// SPDX-License-Identifier: MPL-2.0
//! Platform-specific code for the LoongArch platform.
#[cfg(not(feature = "loongarch_paging_verification"))]
pub mod boot;
#[cfg(not(feature = "loongarch_paging_verification"))]
pub(crate) mod cpu;
#[cfg(not(feature = "loongarch_paging_verification"))]
pub mod device;
#[cfg(not(feature = "loongarch_paging_verification"))]
mod io;
#[cfg(not(feature = "loongarch_paging_verification"))]
pub(crate) mod iommu;
#[cfg(not(feature = "loongarch_paging_verification"))]
pub(crate) mod irq;
#[cfg(feature = "loongarch_paging_verification")]
#[path = "verification/irq.rs"]
pub(crate) mod irq;
#[cfg(not(feature = "loongarch_paging_verification"))]
pub mod kernel;
pub(crate) mod mm;
#[cfg(not(feature = "loongarch_paging_verification"))]
pub(crate) mod pci;
#[cfg(not(feature = "loongarch_paging_verification"))]
pub mod qemu;
#[cfg(not(feature = "loongarch_paging_verification"))]
pub(crate) mod serial;
#[cfg(not(feature = "loongarch_paging_verification"))]
pub(crate) mod task;
pub mod timer;
#[cfg(not(feature = "loongarch_paging_verification"))]
pub mod trap;

#[cfg(feature = "cvm_guest")]
#[cfg(not(feature = "loongarch_paging_verification"))]
pub(crate) fn init_cvm_guest() {
    // Unimplemented, no-op
}

#[cfg(not(feature = "loongarch_paging_verification"))]
pub(crate) unsafe fn late_init_on_bsp() {
    // SAFETY: This function is called in the boot context of the BSP.
    unsafe { trap::init() };

    let io_mem_builder = io::construct_io_mem_allocator_builder();

    kernel::irq::init();

    // SAFETY: We're on the BSP and we're ready to boot all APs.
    unsafe { crate::boot::smp::boot_all_aps() };

    // SAFETY:
    // 1. All the system device memory have been removed from the builder.
    // 2. LoongArch platforms does not have port I/O.
    unsafe { crate::io::init(io_mem_builder) };

    let _ = pci::init();
}

#[cfg(not(feature = "loongarch_paging_verification"))]
pub(crate) unsafe fn init_on_ap() {
    unimplemented!()
}

#[cfg(not(feature = "loongarch_paging_verification"))]
pub(crate) fn interrupts_ack(irq_number: usize) {
    kernel::irq::complete(irq_number as _);
}

/// Returns the frequency of TSC. The unit is Hz.
#[cfg(not(feature = "loongarch_paging_verification"))]
pub fn tsc_freq() -> u64 {
    loongArch64::time::get_timer_freq() as _
}

/// Reads the current value of the processor’s time-stamp counter (TSC).
#[cfg(not(feature = "loongarch_paging_verification"))]
pub fn read_tsc() -> u64 {
    loongArch64::time::Time::read() as _
}

/// Reads a hardware generated 64-bit random value.
///
/// Returns None if no random value was generated.
#[cfg(not(feature = "loongarch_paging_verification"))]
pub fn read_random() -> Option<u64> {
    // FIXME: Implement a hardware random number generator on LoongArch platforms.
    None
}

#[cfg(not(feature = "loongarch_paging_verification"))]
pub(crate) fn enable_cpu_features() {}
