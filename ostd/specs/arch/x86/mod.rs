use vstd::prelude::*;

use super::model::{self, ArchAddressSpaceModel, ArchPagingModel};

use crate::arch::mm::MAX_ARCH_PADDR;

use crate::mm::{
    MAX_PADDR, Paddr, PagingConstsTrait, Vaddr,
    kspace::{LINEAR_MAPPING_BASE_VADDR, VMALLOC_BASE_VADDR},
};

verus! {

/// The x86 instance of the architecture-wide specification contract.
pub struct X86Arch;

impl ArchPagingModel for X86Arch {
    type C = crate::arch::mm::PagingConsts;

    open spec fn max_arch_paddr_spec() -> Paddr {
        MAX_ARCH_PADDR
    }

    proof fn lemma_paging_model_requirements() {
        Self::C::lemma_paging_consts_requirements();

    }
}

impl ArchAddressSpaceModel for X86Arch {
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

        assert(0xffff_8000_0000_0000 <= Self::linear_mapping_base_vaddr_spec()) by (compute_only);

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
