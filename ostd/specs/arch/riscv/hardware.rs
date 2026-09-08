//! Trusted interface to the active address space, matching the existing x86 model.
//!
//! `riscv` 0.11.1 reads CSR 0x180 and `Satp::ppn` masks its low 44 bits;
//! OSTD shifts that page number left by 12. The current VM-space model assumes
//! SATP remains stable during each reader/writer operation. CSR access, privilege
//! checks, and address-space switching are outside the paging proofs.
use vstd::prelude::*;

use crate::mm::Paddr;

verus! {

pub uninterp spec fn current_page_table_paddr_spec() -> Paddr;

#[verifier::when_used_as_spec(current_page_table_paddr_spec)]
pub assume_specification[ crate::arch::mm::current_page_table_paddr ]() -> (paddr: Paddr)
    returns
        current_page_table_paddr_spec(),
;

} // verus!
