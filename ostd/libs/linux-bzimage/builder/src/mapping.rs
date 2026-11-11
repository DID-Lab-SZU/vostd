// SPDX-License-Identifier: MPL-2.0
//! In the setup, VA - SETUP32_LMA == FileOffset - LEGACY_SETUP_SEC_SIZE.
//! And the addresses are specified in the ELF file.
//!
//! This module centralizes the conversion between VA and FileOffset.
//!
//! Verification covers conversions between each address wrapper and `usize`.
//! Arithmetic and conversions between address types remain unverified.
use vstd::{prelude::*, std_specs::convert::FromSpecImpl};

use std::{
    cmp::PartialOrd,
    convert::From,
    ops::{Add, Sub},
};

// We chose the legacy setup sections to be 7 so that the setup header
// is page-aligned and the legacy setup section size would be 0x1000.
#[verus_verify]
pub const LEGACY_SETUP_SECS: usize = 7;
#[verus_verify]
pub const LEGACY_SETUP_SEC_SIZE: usize = 0x200 * (LEGACY_SETUP_SECS + 1);

#[verus_verify]
pub const SETUP32_LMA: usize = 0x100000;

#[verus_verify]
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Clone, Copy)]
pub struct SetupVA {
    addr: usize,
}

#[verus_verify]
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Clone, Copy)]
pub struct SetupFileOffset {
    offset: usize,
}

#[verus_verify]
impl From<usize> for SetupVA {
    #[verus_spec(result => ensures result.value() == addr)]
    fn from(addr: usize) -> Self {
        Self { addr }
    }
}

#[verus_verify]
impl From<SetupVA> for usize {
    #[verus_spec(returns va.value())]
    fn from(va: SetupVA) -> Self {
        va.addr
    }
}

impl Sub for SetupVA {
    type Output = usize;

    fn sub(self, rhs: Self) -> Self::Output {
        self.addr - rhs.addr
    }
}

impl Add<usize> for SetupVA {
    type Output = Self;

    fn add(self, rhs: usize) -> Self::Output {
        Self {
            addr: self.addr + rhs,
        }
    }
}

#[verus_verify]
impl From<usize> for SetupFileOffset {
    #[verus_spec(result => ensures result.value() == offset)]
    fn from(offset: usize) -> Self {
        Self { offset }
    }
}

#[verus_verify]
impl From<SetupFileOffset> for usize {
    #[verus_spec(returns offset.value())]
    fn from(offset: SetupFileOffset) -> Self {
        offset.offset
    }
}

impl Sub for SetupFileOffset {
    type Output = usize;

    fn sub(self, rhs: Self) -> Self::Output {
        self.offset - rhs.offset
    }
}

impl Add<usize> for SetupFileOffset {
    type Output = Self;

    fn add(self, rhs: usize) -> Self::Output {
        Self {
            offset: self.offset + rhs,
        }
    }
}

impl From<SetupVA> for SetupFileOffset {
    fn from(va: SetupVA) -> Self {
        Self {
            offset: va.addr + LEGACY_SETUP_SEC_SIZE - SETUP32_LMA,
        }
    }
}

impl From<SetupFileOffset> for SetupVA {
    fn from(offset: SetupFileOffset) -> Self {
        Self {
            addr: offset.offset + SETUP32_LMA - LEGACY_SETUP_SEC_SIZE,
        }
    }
}

verus! {

impl FromSpecImpl<usize> for SetupVA {
    open spec fn obeys_from_spec() -> bool {
        true
    }

    closed spec fn from_spec(v: usize) -> Self {
        Self { addr: v }
    }
}

impl FromSpecImpl<SetupVA> for usize {
    open spec fn obeys_from_spec() -> bool {
        true
    }

    closed spec fn from_spec(v: SetupVA) -> Self {
        v.addr
    }
}

impl FromSpecImpl<usize> for SetupFileOffset {
    open spec fn obeys_from_spec() -> bool {
        true
    }

    closed spec fn from_spec(v: usize) -> Self {
        Self { offset: v }
    }
}

impl FromSpecImpl<SetupFileOffset> for usize {
    open spec fn obeys_from_spec() -> bool {
        true
    }

    closed spec fn from_spec(v: SetupFileOffset) -> Self {
        v.offset
    }
}

impl SetupVA {
    /// The represented setup virtual address.
    pub closed spec fn value(self) -> usize {
        self.addr
    }
}

impl SetupFileOffset {
    /// The represented byte offset in the setup file.
    pub closed spec fn value(self) -> usize {
        self.offset
    }
}

} // verus!
