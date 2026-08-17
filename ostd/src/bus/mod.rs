// SPDX-License-Identifier: MPL-2.0
//! Bus operations
use vstd::prelude::*;

#[cfg(not(verus_keep_ghost))]
pub mod pci;

verus! {

/// Models the partition between claimed and still-unclaimed devices.
pub open spec fn probe_partition(total: nat, claimed: nat) -> nat
    recommends
        claimed <= total,
{
    (total - claimed) as nat
}

/// Claiming devices preserves the total number of discovered devices.
pub proof fn lemma_probe_partition_preserves_total(total: nat, claimed: nat)
    requires
        claimed <= total,
    ensures
        probe_partition(total, claimed) + claimed == total,
{
}

/// Whether a device/function pair is representable in a PCI configuration address.
pub open spec fn valid_location(device: u8, function: u8) -> bool {
    device < 32 && function < 8
}

/// Models the x86 PCI configuration address encoding without performing port I/O.
pub open spec fn config_address(bus: u8, device: u8, function: u8) -> u32 {
    (1u32 << 31) | ((bus as u32) << 16) | (((device as u32) & 0x1f) << 11) | (((function as u32)
        & 0x7) << 8)
}

/// The configuration address keeps each location component in its assigned field.
pub proof fn lemma_config_address_layout(bus: u8, device: u8, function: u8)
    requires
        valid_location(device, function),
    ensures
        config_address(bus, device, function) & 0x8000_0000 == 0x8000_0000,
        config_address(bus, device, function) & 0x00ff_0000 == (bus as u32) << 16,
        config_address(bus, device, function) & 0x0000_f800 == ((device as u32) & 0x1f) << 11,
        config_address(bus, device, function) & 0x0000_0700 == ((function as u32) & 0x7) << 8,
{
    assert(config_address(bus, device, function) & 0x8000_0000 == 0x8000_0000) by (bit_vector);
    assert(config_address(bus, device, function) & 0x00ff_0000 == (bus as u32) << 16)
        by (bit_vector);
    assert(config_address(bus, device, function) & 0x0000_f800 == ((device as u32) & 0x1f) << 11)
        by (bit_vector);
    assert(config_address(bus, device, function) & 0x0000_0700 == ((function as u32) & 0x7) << 8)
        by (bit_vector);
}

/// Returns the byte selected from a configuration-space dword.
pub fn model_read8(word: u32, byte_offset: u8) -> (value: u8)
    requires
        byte_offset < 4,
    ensures
        value == ((word >> ((byte_offset as u32) * 8)) & 0xff) as u8,
{
    ((word >> ((byte_offset as u32) * 8)) & 0xff) as u8
}

/// Returns the aligned half-word selected from a configuration-space dword.
pub fn model_read16(word: u32, byte_offset: u8) -> (value: u16)
    requires
        byte_offset == 0 || byte_offset == 2,
    ensures
        value == ((word >> ((byte_offset as u32) * 8)) & 0xffff) as u16,
{
    ((word >> ((byte_offset as u32) * 8)) & 0xffff) as u16
}

/// Models the read-modify-write operation used for an 8-bit config-space write.
pub fn model_write8(word: u32, byte_offset: u8, value: u8) -> (updated: u32)
    requires
        byte_offset < 4,
    ensures
        updated == ((value as u32) << ((byte_offset as u32) * 8)) | (word & !(0xffu32 << ((
        byte_offset as u32) * 8))),
{
    let shift = (byte_offset as u32) * 8;
    let mask = 0xffu32 << shift;
    ((value as u32) << shift) | (word & !mask)
}

/// Models the read-modify-write operation used for a 16-bit config-space write.
pub fn model_write16(word: u32, byte_offset: u8, value: u16) -> (updated: u32)
    requires
        byte_offset == 0 || byte_offset == 2,
    ensures
        updated == ((value as u32) << ((byte_offset as u32) * 8)) | (word & !(0xffffu32 << ((
        byte_offset as u32) * 8))),
{
    let shift = (byte_offset as u32) * 8;
    let mask = 0xffffu32 << shift;
    ((value as u32) << shift) | (word & !mask)
}

/// Models updating one bit while preserving all other bits.
pub fn model_set_bit(origin: u16, offset: u8, set: bool) -> (updated: u16)
    requires
        offset < 16,
    ensures
        updated == (origin & !(1u16 << offset)) | ((set as u16) << offset),
{
    (origin & !(1u16 << offset)) | ((set as u16) << offset)
}

/// Computes the common-configuration-space offset of a BAR.
pub fn model_bar_offset(index: u8) -> (offset: u16)
    requires
        index < 6,
    ensures
        offset == 0x10 + 4 * index,
        0x10 <= offset <= 0x24,
        offset % 4 == 0,
{
    0x10 + 4 * index as u16
}

/// Extracts the number of MSI-X table entries from the message-control register.
pub fn model_msix_table_size(message_control: u16) -> (size: u16)
    ensures
        size == (message_control & 0x03ff) + 1,
        1 <= size <= 1024,
{
    assert((message_control & 0x03ff) <= 0x03ff) by (bit_vector);
    let size = (message_control & 0x03ff) + 1;
    size
}

/// A capability access is local only when its complete value fits in the capability.
pub open spec fn capability_access_in_bounds(offset: nat, width: nat, length: nat) -> bool {
    0 < width && offset + width <= length
}

/// A valid capability access cannot start beyond the capability.
pub proof fn lemma_capability_access_start_in_bounds(offset: nat, width: nat, length: nat)
    requires
        capability_access_in_bounds(offset, width, length),
    ensures
        offset < length,
{
}

/// A forward capability pointer has a non-empty span and remains in config space.
pub proof fn lemma_capability_span(current: u16, next: u16)
    requires
        current < next,
        next <= 0xfc,
    ensures
        0 < next - current,
        next - current <= 0xfc,
{
}

/// Models the range check shared by PIO and MMIO BAR accesses.
pub open spec fn bar_access_in_bounds(offset: nat, width: nat, size: nat) -> bool {
    0 < width && offset + width <= size
}

/// An in-bounds BAR access has both a valid start and a sufficient region size.
pub proof fn lemma_bar_access_bounds(offset: nat, width: nat, size: nat)
    requires
        bar_access_in_bounds(offset, width, size),
    ensures
        offset < size,
        width <= size,
{
}

} // verus!
/// An error that occurs during bus probing.
#[cfg(not(verus_keep_ghost))]
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BusProbeError {
    /// The device does not match the expected criteria.
    DeviceNotMatch,
    /// An error in accessing the configuration space of the device.
    ConfigurationSpaceError,
}

/// Initializes the bus
#[cfg(not(verus_keep_ghost))]
pub(crate) fn init() {
    pci::init();
}
