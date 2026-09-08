pub mod model;
pub use model::*;

// Compatibility re-exports for proof modules that still use `specs::arch`.
// The authoritative values live in the executable memory/architecture modules.
pub use crate::{
    arch::mm::{NR_ENTRIES, NR_LEVELS, PAGE_SIZE},
    mm::{MAX_NR_PAGES, MAX_PADDR},
};

pub mod riscv;
#[cfg(target_arch = "x86_64")]
mod x86;
#[cfg(target_arch = "x86_64")]
pub use x86::*;

#[cfg(target_arch = "x86_64")]
pub type CurrentArch = x86::X86Arch;

#[cfg(target_arch = "riscv64")]
pub use riscv::RiscvArch;
#[cfg(target_arch = "riscv64")]
pub type CurrentArch = riscv::RiscvArch;

mod current;
pub use current::*;
