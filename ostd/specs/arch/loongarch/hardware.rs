//! Trusted PGDL/PGDH reads used by the existing VM-space model.
//!
//! Source: `loongArch64` 0.2.6, `register/mmu/pgdl.rs`, `pgdh.rs`, and
//! `register/macros.rs`: `csrrd` reads CSRs 0x19/0x1a, and `raw()` returns
//! the stored word. The getter returns PGDL only after asserting that PGDL equals PGDH. This
//! contract requires equal roots before the read, excluding that assertion's panic.
//! It does not claim `no_unwind` for the unverified hardware access.
//! Register values are assumed stable during a reader/writer operation, as in
//! the existing x86/RISC-V model. Privilege and CSR access remain unverified.
use crate::mm::Paddr;
use vstd::prelude::*;
verus! {

pub uninterp spec fn current_page_table_paddr_spec() -> Paddr;

pub uninterp spec fn current_high_page_table_paddr_spec() -> Paddr;

#[verifier::when_used_as_spec(current_page_table_paddr_spec)]
pub assume_specification[ crate::arch::mm::current_page_table_paddr ]() -> Paddr
    requires
        current_page_table_paddr_spec() == current_high_page_table_paddr_spec(),
    returns
        current_page_table_paddr_spec(),
;

} // verus!
