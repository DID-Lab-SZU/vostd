use crate::mm::{MAX_PADDR, Paddr, PagingConstsTrait, Vaddr};
use vstd::prelude::*;

verus! {

// Every architecture currently supported by OSTD uses 64-bit pointers.
global size_of usize == 8;

global size_of isize == 8;

/// The paging-related part of an architecture contract.
///
/// The associated paging constants are still supplied by the existing
/// `PagingConstsTrait`; this trait only adds the architecture-wide encodable
/// physical-address bound and the proof that the contracts are compatible.
pub trait ArchPagingModel {
    type C: PagingConstsTrait;

    /// The exclusive upper bound for physical addresses encodable by a PTE.
    spec fn max_arch_paddr_spec() -> Paddr;

    proof fn lemma_paging_model_requirements()
        ensures
            0 < Self::max_arch_paddr_spec(),
            Self::C::BASE_PAGE_SIZE() <= MAX_PADDR <= Self::max_arch_paddr_spec(),
            MAX_PADDR % Self::C::BASE_PAGE_SIZE() == 0,
            Self::max_arch_paddr_spec() % Self::C::BASE_PAGE_SIZE() == 0,
    ;
}

/// A base-page-aligned physical address encodable by architecture `A`.
pub open spec fn valid_arch_paddr_for<A: ArchPagingModel>(pa: Paddr) -> bool {
    pa % A::C::BASE_PAGE_SIZE() == 0 && pa < A::max_arch_paddr_spec()
}

/// A physical frame address represented by the tracked frame-metadata model.
///
/// This bound is intentionally narrower than the address range that a hardware
/// PTE can encode.
pub open spec fn valid_tracked_frame_paddr_for<A: ArchPagingModel>(pa: Paddr) -> bool {
    pa % A::C::BASE_PAGE_SIZE() == 0 && pa < MAX_PADDR
}

pub proof fn lemma_tracked_frame_paddr_is_arch_paddr<A: ArchPagingModel>(pa: Paddr)
    requires
        valid_tracked_frame_paddr_for::<A>(pa),
    ensures
        valid_arch_paddr_for::<A>(pa),
{
    A::lemma_paging_model_requirements();
}

/// Whether `va` is a canonical virtual address for paging configuration `C`.
pub open spec fn is_canonical_vaddr_for<C: PagingConstsTrait>(va: Vaddr) -> bool {
    if C::VA_SIGN_EXT() {
        (va as int) < vstd::arithmetic::power2::pow2((C::ADDRESS_WIDTH() - 1) as nat) || (
        usize::MAX as int) - vstd::arithmetic::power2::pow2((C::ADDRESS_WIDTH() - 1) as nat) < va
    } else {
        (va as int) < vstd::arithmetic::power2::pow2(C::ADDRESS_WIDTH() as nat)
    }
}

/// Every address in the upper canonical half is valid for a sign-extended
/// 48-bit virtual-address configuration.
pub proof fn lemma_sv48_upper_half_is_canonical<C: PagingConstsTrait>(va: Vaddr)
    requires
        C::VA_SIGN_EXT(),
        C::ADDRESS_WIDTH() == 48,
        0xffff_8000_0000_0000 <= va,
    ensures
        is_canonical_vaddr_for::<C>(va),
{
    vstd::arithmetic::power2::lemma2_to64_rest();
    assert(vstd::arithmetic::power2::pow2(47) == 0x8000_0000_0000int);
    assert((usize::MAX as int) == 0xffff_ffff_ffff_ffffint) by (compute_only);
}

/// The address-space part of an architecture contract.
pub trait ArchAddressSpaceModel: ArchPagingModel {
    /// The base of the kernel's physical-to-virtual linear mapping.
    spec fn linear_mapping_base_vaddr_spec() -> Vaddr;

    /// The first virtual address reserved for vmalloc mappings.
    spec fn vmalloc_base_vaddr_spec() -> Vaddr;

    proof fn lemma_address_space_model_requirements()
        ensures
            Self::linear_mapping_base_vaddr_spec() % Self::C::BASE_PAGE_SIZE() == 0,
            Self::linear_mapping_base_vaddr_spec() < Self::vmalloc_base_vaddr_spec(),
            MAX_PADDR < Self::vmalloc_base_vaddr_spec() - Self::linear_mapping_base_vaddr_spec(),
            MAX_PADDR + Self::linear_mapping_base_vaddr_spec() < usize::MAX,
            is_canonical_vaddr_for::<Self::C>(Self::linear_mapping_base_vaddr_spec()),
            is_canonical_vaddr_for::<Self::C>(Self::vmalloc_base_vaddr_spec()),
            is_canonical_vaddr_for::<Self::C>(
                (Self::linear_mapping_base_vaddr_spec() + MAX_PADDR) as usize,
            ),
    ;
}

/// Convert a physical address through architecture `A`'s linear mapping.
pub open spec fn paddr_to_vaddr_for<A: ArchAddressSpaceModel>(pa: Paddr) -> Vaddr {
    (pa + A::linear_mapping_base_vaddr_spec()) as usize
}

/// Convert a linear-mapped virtual address back to a physical address.
pub open spec fn vaddr_to_paddr_for<A: ArchAddressSpaceModel>(va: Vaddr) -> Paddr {
    (va - A::linear_mapping_base_vaddr_spec()) as usize
}

} // verus!
