#[cfg(target_arch = "loongarch64")]
mod hardware;

#[cfg(target_arch = "loongarch64")]
pub use hardware::{current_high_page_table_paddr_spec, current_page_table_paddr_spec};

use vstd::{
    arithmetic::power2::{lemma2_to64, lemma2_to64_rest},
    prelude::*,
};
use vstd_extra::prelude::*;

use super::model::{self, ArchAddressSpaceModel, ArchPagingModel};

use crate::mm::{
    kspace::{LINEAR_MAPPING_BASE_VADDR, VMALLOC_BASE_VADDR},
    Paddr, PagingConstsTrait, PagingLevel, Vaddr, MAX_PADDR,
};

verus! {

/// Base-page size of the LoongArch paging scheme.
pub const PAGE_SIZE: usize = 4096;

/// Size of a LoongArch page-table entry.
pub const PTE_SIZE: usize = 8;

/// Number of entries in a LoongArch page-table node.
pub const NR_ENTRIES: usize = 512;

/// Number of translation levels used by LoongArch.
pub const NR_LEVELS: usize = 4;

/// Width of canonical virtual addresses in LoongArch.
pub const ADDRESS_WIDTH: usize = 48;

/// Exclusive upper bound of physical addresses encodable by a LoongArch PTE.
pub const MAX_ARCH_PADDR: Paddr = 0x1_0000_0000_0000;

/// Highest level at which a LoongArch PTE may directly map a page.
pub const HIGHEST_TRANSLATION_LEVEL: PagingLevel = 1;

/// LoongArch virtual addresses use sign extension from bit 47.
pub const VA_SIGN_EXT: bool = true;

/// Paging constants used to verify the target-independent LoongArch model.
#[verifier::allow(autoderive_clone_without_spec)]
#[derive(Clone, Debug, Default)]
pub struct LoongArchPagingConsts;

#[cfg(target_arch = "loongarch64")]
type LoongArchModelPagingConsts = crate::arch::mm::PagingConsts;

#[cfg(not(target_arch = "loongarch64"))]
type LoongArchModelPagingConsts = LoongArchPagingConsts;

impl PagingConstsTrait for LoongArchPagingConsts {
    #[verifier::inline]
    open spec fn BASE_PAGE_SIZE_spec() -> usize {
        PAGE_SIZE
    }

    #[inline(always)]
    fn BASE_PAGE_SIZE() -> usize {
        PAGE_SIZE
    }

    #[verifier::inline]
    open spec fn NR_LEVELS_spec() -> PagingLevel {
        NR_LEVELS as PagingLevel
    }

    #[inline(always)]
    fn NR_LEVELS() -> PagingLevel {
        NR_LEVELS as PagingLevel
    }

    #[verifier::inline]
    open spec fn HIGHEST_TRANSLATION_LEVEL_spec() -> PagingLevel {
        HIGHEST_TRANSLATION_LEVEL
    }

    #[inline(always)]
    fn HIGHEST_TRANSLATION_LEVEL() -> PagingLevel {
        HIGHEST_TRANSLATION_LEVEL
    }

    #[verifier::inline]
    open spec fn PTE_SIZE_spec() -> usize {
        PTE_SIZE
    }

    #[inline(always)]
    fn PTE_SIZE() -> usize {
        PTE_SIZE
    }

    #[verifier::inline]
    open spec fn ADDRESS_WIDTH_spec() -> usize {
        ADDRESS_WIDTH
    }

    #[inline(always)]
    fn ADDRESS_WIDTH() -> usize {
        ADDRESS_WIDTH
    }

    #[verifier::inline]
    open spec fn VA_SIGN_EXT_spec() -> bool {
        VA_SIGN_EXT
    }

    #[inline(always)]
    fn VA_SIGN_EXT() -> bool {
        VA_SIGN_EXT
    }

    proof fn lemma_paging_consts_requirements() {
        assert(Self::BASE_PAGE_SIZE() == PAGE_SIZE) by (compute_only);
        assert(Self::NR_LEVELS() == NR_LEVELS as PagingLevel) by (compute_only);
        assert(Self::PTE_SIZE() == PTE_SIZE) by (compute_only);
        assert(Self::ADDRESS_WIDTH() == ADDRESS_WIDTH) by (compute_only);
        lemma_pow2_is_pow2_to64();
        lemma2_to64();
        lemma2_to64_rest();
        vstd::layout::unsigned_int_max_values();
        lemma_usize_pow2_ilog2(12);
        lemma_usize_pow2_ilog2(9);
    }
}

/// The LoongArch instance of the architecture-wide paging model.
pub ghost struct LoongArch;

impl ArchPagingModel for LoongArch {
    type C = LoongArchModelPagingConsts;

    open spec fn max_arch_paddr_spec() -> Paddr {
        MAX_ARCH_PADDR
    }

    proof fn lemma_paging_model_requirements() {
        Self::C::lemma_paging_consts_requirements();
        assert(Self::C::BASE_PAGE_SIZE() <= MAX_PADDR <= Self::max_arch_paddr_spec())
            by (compute_only);
    }
}

impl ArchAddressSpaceModel for LoongArch {
    open spec fn linear_mapping_base_vaddr_spec() -> Vaddr {
        LINEAR_MAPPING_BASE_VADDR
    }

    open spec fn vmalloc_base_vaddr_spec() -> Vaddr {
        VMALLOC_BASE_VADDR
    }

    proof fn lemma_address_space_model_requirements() {
        Self::C::lemma_paging_consts_requirements();
        Self::lemma_paging_model_requirements();
        assert(Self::linear_mapping_base_vaddr_spec() % Self::C::BASE_PAGE_SIZE() == 0)
            by (compute_only);
        assert(MAX_PADDR < Self::vmalloc_base_vaddr_spec() - Self::linear_mapping_base_vaddr_spec())
            by (compute_only);
        assert(Self::C::VA_SIGN_EXT()) by (compute_only);
        assert(Self::C::ADDRESS_WIDTH() == 48) by (compute_only);
        assert(0xffff_8000_0000_0000 <= Self::linear_mapping_base_vaddr_spec()) by (compute_only);
        assert(0xffff_8000_0000_0000 <= Self::vmalloc_base_vaddr_spec()) by (compute_only);
        assert(0xffff_8000_0000_0000 <= (Self::linear_mapping_base_vaddr_spec()
            + MAX_PADDR) as usize) by (compute_only);
        model::lemma_sv48_upper_half_is_canonical::<Self::C>(
            Self::linear_mapping_base_vaddr_spec(),
        );
        model::lemma_sv48_upper_half_is_canonical::<Self::C>(Self::vmalloc_base_vaddr_spec());
        model::lemma_sv48_upper_half_is_canonical::<Self::C>(
            (Self::linear_mapping_base_vaddr_spec() + MAX_PADDR) as usize,
        );
    }
}

} // verus!
