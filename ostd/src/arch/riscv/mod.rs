// SPDX-License-Identifier: MPL-2.0
//! Platform-specific code for the RISC-V platform.
#[cfg(not(feature = "riscv_paging_verification"))]
pub mod boot;
#[cfg(not(feature = "riscv_paging_verification"))]
pub(crate) mod cpu;
#[cfg(not(feature = "riscv_paging_verification"))]
pub mod device;
#[cfg(not(feature = "riscv_paging_verification"))]
pub(crate) mod iommu;
#[cfg(not(feature = "riscv_paging_verification"))]
pub(crate) mod irq;
#[cfg(feature = "riscv_paging_verification")]
#[path = "verification/irq.rs"]
pub(crate) mod irq;
pub(crate) mod mm;
#[cfg(not(feature = "riscv_paging_verification"))]
pub(crate) mod pci;
#[cfg(not(feature = "riscv_paging_verification"))]
pub mod qemu;
#[cfg(not(feature = "riscv_paging_verification"))]
pub(crate) mod serial;
#[cfg(not(feature = "riscv_paging_verification"))]
pub(crate) mod task;
#[cfg(not(feature = "riscv_paging_verification"))]
pub mod timer;
#[cfg(feature = "riscv_paging_verification")]
#[path = "verification/timer.rs"]
pub mod timer;
#[cfg(not(feature = "riscv_paging_verification"))]
pub mod trap;

#[cfg(not(feature = "riscv_paging_verification"))]
#[cfg(feature = "cvm_guest")]
pub(crate) fn init_cvm_guest() {
    // Unimplemented, no-op
}

#[cfg(not(feature = "riscv_paging_verification"))]
pub(crate) unsafe fn late_init_on_bsp() {
    // SAFETY: This function is called in the boot context of the BSP.
    unsafe { trap::init() };

    // SAFETY: We're on the BSP and we're ready to boot all APs.
    unsafe { crate::boot::smp::boot_all_aps() };

    // SAFETY: This function is called once and at most once at a proper timing
    // in the boot context of the BSP, with no timer-related operations having
    // been performed.
    unsafe { timer::init() };
    let _ = pci::init();
}

#[cfg(not(feature = "riscv_paging_verification"))]
pub(crate) unsafe fn init_on_ap() {
    unimplemented!()
}

#[cfg(not(feature = "riscv_paging_verification"))]
pub(crate) fn interrupts_ack(irq_number: usize) {
    unimplemented!()
}

/// Return the frequency of TSC. The unit is Hz.
#[cfg(not(feature = "riscv_paging_verification"))]
pub fn tsc_freq() -> u64 {
    timer::get_timebase_freq()
}

/// Reads the current value of the processor’s time-stamp counter (TSC).
#[cfg(not(feature = "riscv_paging_verification"))]
pub fn read_tsc() -> u64 {
    riscv::register::time::read64()
}

/// Reads a hardware generated 64-bit random value.
///
/// Returns None if no random value was generated.
#[cfg(not(feature = "riscv_paging_verification"))]
pub fn read_random() -> Option<u64> {
    // FIXME: Implement a hardware random number generator on RISC-V platforms.
    None
}

#[cfg(not(feature = "riscv_paging_verification"))]
pub(crate) fn enable_cpu_features() {
    cpu::extension::init();
    unsafe {
        // We adopt a lazy approach to enable the floating-point unit; it's not
        // enabled before the first FPU trap.
        riscv::register::sstatus::set_fs(riscv::register::sstatus::FS::Off);
    }
}
