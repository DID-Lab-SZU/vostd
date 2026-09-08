// SPDX-License-Identifier: MPL-2.0
//! Timer constants needed by paging-focused verification builds.
use vstd::prelude::*;

verus! {

/// The timer frequency in hertz.
pub const TIMER_FREQ: u64 = 1000;

} // verus!
