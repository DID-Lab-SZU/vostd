// SPDX-License-Identifier: MPL-2.0
//! Minimal local-interrupt interface for paging-focused verification builds.
pub(crate) fn enable_local() {
    // SAFETY: This preserves the existing RISC-V implementation's contract.
    unsafe { riscv::interrupt::enable() }
}

pub(crate) fn enable_local_and_halt() {
    riscv::asm::wfi();
    // SAFETY: This preserves the existing RISC-V implementation's contract.
    unsafe { riscv::interrupt::enable() }
}

pub(crate) fn disable_local() {
    riscv::interrupt::disable();
}

pub(crate) fn is_local_enabled() -> bool {
    riscv::register::sstatus::read().sie()
}
