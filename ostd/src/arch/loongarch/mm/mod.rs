// SPDX-License-Identifier: MPL-2.0
//! LoongArch paging and the exact model of its existing PTE encoding.
//!
//! # Verified Properties
//! ## Safety
//! PTE access operates on an owned integer word; decoding establishes valid
//! bitflags before unwrapping them. CSR access, TLB operations, and concurrent
//! hardware changes remain outside these proofs.
//! ## Functional Correctness
//! The model proves 4 KiB leaf construction, child-table and absent entries,
//! address extraction, and property updates within the existing encoding's limits.
//! ## Preconditions
//! Leaf construction requires level 1, valid flags with DIRTY set, and Writeback
//! or WriteCombining caching. Updates additionally require a present, non-huge
//! entry with address bit 12 clear and an input without GLOBAL.
//! ## Postconditions
//! Constructors preserve tracked physical addresses and round-trip supported
//! properties. Updates preserve the physical address, presence, and terminal
//! status, and return the requested property. The raw encoder also models the
//! legacy Uncacheable mismatch and forced DIRTY behavior.
use crate::{
    mm::{
        page_prop::{CachePolicy, PageFlags, PageProperty, PrivilegedPageFlags as PrivFlags},
        page_table::PageTableEntryTrait,
        CurrentPagingConstsTrait, Paddr, PagingConstsTrait, PagingLevel, PodOnce, Vaddr, MAX_PADDR,
    },
    Pod,
};
use alloc::fmt;
use core::{arch::asm, ops::Range};
use vstd::{
    arithmetic::power2::{lemma2_to64, lemma2_to64_rest},
    prelude::*,
};
use vstd_extra::prelude::*;
verus! {

/// Size of a base page in LoongArch.
pub const PAGE_SIZE: usize = 4096;

/// Size of a LoongArch page-table entry.
pub const PTE_SIZE: usize = 8;

/// Number of entries in a LoongArch page-table node.
pub const NR_ENTRIES: usize = 512;

/// Number of translation levels in LoongArch.
pub const NR_LEVELS: usize = 4;

/// Width of canonical virtual addresses in LoongArch.
pub const ADDRESS_WIDTH: usize = 48;

/// Exclusive upper bound of physical addresses encodable by a LoongArch PTE.
pub const MAX_ARCH_PADDR: Paddr = 0x1_0000_0000_0000;

/// Highest level at which a LoongArch PTE may directly map a page.
pub const HIGHEST_TRANSLATION_LEVEL: PagingLevel = 1;

/// LoongArch virtual addresses use sign extension from bit 47.
pub const VA_SIGN_EXT: bool = true;

#[verifier::allow(autoderive_clone_without_spec)]
#[derive(Clone, Debug, Default)]
pub struct PagingConsts {}

impl PagingConstsTrait for PagingConsts {
    #[verifier::inline]
    open spec fn BASE_PAGE_SIZE_spec() -> usize {
        PAGE_SIZE
    }

    #[inline(always)]
    fn BASE_PAGE_SIZE() -> usize {
        PAGE_SIZE
    }

    #[verifier::inline]
    open spec fn NR_LEVELS_spec() -> PagingLevel {
        NR_LEVELS as PagingLevel
    }

    #[inline(always)]
    fn NR_LEVELS() -> PagingLevel {
        NR_LEVELS as PagingLevel
    }

    #[verifier::inline]
    open spec fn HIGHEST_TRANSLATION_LEVEL_spec() -> PagingLevel {
        HIGHEST_TRANSLATION_LEVEL
    }

    #[inline(always)]
    fn HIGHEST_TRANSLATION_LEVEL() -> PagingLevel {
        HIGHEST_TRANSLATION_LEVEL
    }

    #[verifier::inline]
    open spec fn PTE_SIZE_spec() -> usize {
        PTE_SIZE
    }

    #[inline(always)]
    fn PTE_SIZE() -> usize {
        PTE_SIZE
    }

    #[verifier::inline]
    open spec fn ADDRESS_WIDTH_spec() -> usize {
        ADDRESS_WIDTH
    }

    #[inline(always)]
    fn ADDRESS_WIDTH() -> usize {
        ADDRESS_WIDTH
    }

    #[verifier::inline]
    open spec fn VA_SIGN_EXT_spec() -> bool {
        VA_SIGN_EXT
    }

    #[inline(always)]
    fn VA_SIGN_EXT() -> bool {
        VA_SIGN_EXT
    }

    proof fn lemma_paging_consts_requirements() {
        assert(Self::BASE_PAGE_SIZE() == PAGE_SIZE) by (compute_only);
        assert(Self::NR_LEVELS() == NR_LEVELS as PagingLevel) by (compute_only);
        assert(Self::PTE_SIZE() == PTE_SIZE) by (compute_only);
        assert(Self::ADDRESS_WIDTH() == ADDRESS_WIDTH) by (compute_only);
        lemma_pow2_is_pow2_to64();
        lemma2_to64();
        lemma2_to64_rest();
        vstd::layout::unsigned_int_max_values();
        lemma_usize_pow2_ilog2(12);
        lemma_usize_pow2_ilog2(9);
    }
}

impl CurrentPagingConstsTrait for PagingConsts {
    proof fn lemma_current_paging_consts_requirements() {
        Self::lemma_paging_consts_requirements();

        assert(Self::BASE_PAGE_SIZE() / Self::PTE_SIZE() == NR_ENTRIES) by (compute_only);
    }
}

/// A next-level page contains one subpage per page-table entry.
///
/// # Preconditions
/// None; the result uses the fixed LoongArch paging constants.
/// # Postconditions
/// The subpage count equals `NR_ENTRIES`; this does not establish huge-page support.
pub proof fn lemma_nr_subpage_per_huge_eq_nr_entries()
    ensures
        crate::mm::nr_subpage_per_huge::<PagingConsts>() == NR_ENTRIES,
{
}

} // verus!
bitflags::bitflags! {
    /// Possible flags for a page table entry.
    pub struct PageTableFlags: usize {
        /// Specifies whether the mapped frame is valid.
        const VALID =           1usize << 0;
        /// Whether the memory area represented by this entry is modified.
        const DIRTY =           1usize << 1;
        /// Privilege level corresponding to the page table entry.
        /// When `RPLV` = 0, the page table entry can be accessed by any program
        /// with a privilege level not lower than `PLV`;
        /// When `RPLV` = 1, this page table entry can only be accessed by programs
        /// with privilege level equal to `PLV`.
        const PLVL =            1usize << 2;
        const PLVH =            1usize << 3;
        /// Controls the memory access type of the memory access operation
        /// falling on the address space of the table page entry.
        const MATL =            1usize << 4;
        const MATH =            1usize << 5;
        /// If this entry is a basic page table entry, it is `GLOBAL`,
        /// which means that the mapping is present in all address spaces,
        /// so it isn't flushed from the TLB on an address space switch.
        /// If this entry is a huge page table entry, it is `HUGE`,
        /// which means that the memory area represented by this entry is
        /// a huge page.
        const GLOBAL_OR_HUGE =  1usize << 6;
        /// Specifies whether the mapped frame or page table is loaded in memory.
        /// This flag does not fill in TLB.
        const PRESENT =         1usize << 7;
        /// Controls whether writes to the mapped frames are allowed.
        /// This flag does not fill in TLB.
        const WRITABLE =        1usize << 8;
        // Whether this entry is a basic page table entry.
        const IS_BASIC =        1usize << 9;
        // First bit ignored by MMU.
        const RSV1 =            1usize << 10;
        // Second bit ignored by MMU.
        const RSV2 =            1usize << 11;
        /// If this entry is a huge page table entry, it is `GLOBAL`.
        const GLOBAL_IN_HUGE =  1usize << 12;
        /// Controls whether reads to the mapped frames are not allowed.
        const NOT_READABLE =    1usize << 61;
        /// Controls whether execution code in the mapped frames are not allowed.
        const NOT_EXECUTABLE =  1usize << 62;
        /// Whether the `PageTableEntry` can only be accessed by the privileged level `PLV` field inferred
        const RPLV =            1usize << 63;
    }
}

pub(crate) fn tlb_flush_addr(vaddr: Vaddr) {
    unsafe {
        asm!(
            "invtlb 0, $zero, {}",
            in(reg) vaddr
        );
    }
}

pub(crate) fn tlb_flush_addr_range(range: &Range<Vaddr>) {
    for vaddr in range.clone().step_by(PAGE_SIZE) {
        tlb_flush_addr(vaddr);
    }
}

pub(crate) fn tlb_flush_all_excluding_global() {
    unsafe {
        asm!("invtlb 3, $zero, $zero");
    }
}

pub(crate) fn tlb_flush_all_including_global() {
    unsafe {
        asm!("invtlb 0, $zero, $zero");
    }
}

/// Activates the given level 4 page table.
///
/// "pgdl" or "pgdh" register doesn't have a field that encodes the cache policy,
/// so `_root_pt_cache` is ignored.
///
/// # Safety
///
/// Changing the level 4 page table is unsafe, because it's possible to violate memory safety by
/// changing the page mapping.
pub unsafe fn activate_page_table(root_paddr: Paddr, _root_pt_cache: CachePolicy) {
    assert!(root_paddr % PagingConsts::BASE_PAGE_SIZE() == 0);
    loongArch64::register::pgdl::set_base(root_paddr);
    loongArch64::register::pgdh::set_base(root_paddr);
}

pub fn current_page_table_paddr() -> Paddr {
    let pgdl = loongArch64::register::pgdl::read().raw();
    let pgdh = loongArch64::register::pgdh::read().raw();
    assert_eq!(
        pgdl, pgdh,
        "Only support to share the same page table for both user and kernel space"
    );
    pgdl
}

verus! {

#[derive(Clone, Copy)]
#[repr(C)]
/// Native PTE word; verified operations follow this module's encoding restrictions.
pub struct PageTableEntry(usize);

global layout PageTableEntry is size == 8, align == 8;

#[verus_verify]
unsafe impl Pod for PageTableEntry {

}

impl PodOnce for PageTableEntry {

}

impl Default for PageTableEntry {
    fn default() -> Self
        returns
            Self::default_spec(),
    {
        Self(0)
    }
}

/// Exact decoding of the huge marker, including its overlap with GLOBAL.
pub open spec fn raw_is_huge(raw: usize) -> bool {
    raw & 0x200usize == 0 && raw & 0x40usize != 0
}

closed spec fn raw_is_global(raw: usize) -> bool {
    if raw & 0x200usize != 0 {
        raw & 0x40usize != 0
    } else {
        raw & 0x1000usize != 0
    }
}

closed spec fn raw_paddr(raw: usize) -> usize {
    if raw_is_huge(raw) {
        raw & 0x0000_FFFF_FFFF_E000usize
    } else {
        raw & 0x0000_FFFF_FFFF_F000usize
    }
}

closed spec fn decode_flags(raw: usize) -> u8 {
    (((!raw & 0x2000_0000_0000_0000usize) >> 61) | ((raw & 0x100usize) >> 7) | ((!raw
        & 0x4000_0000_0000_0000usize) >> 60) | ((raw & 0x80usize) >> 4) | ((raw & 0x2usize) << 3)
        | ((raw & 0xC00usize) >> 4)) as u8
}

closed spec fn decode_priv(raw: usize) -> u8 {
    (if raw & 0xCusize == 0xCusize {
        1u8
    } else {
        0u8
    }) | (if raw_is_global(raw) {
        2u8
    } else {
        0u8
    })
}

closed spec fn decode_cache(raw: usize) -> CachePolicy {
    if raw & 0x10usize != 0 {
        CachePolicy::Writeback
    } else if raw & 0x20usize != 0 {
        CachePolicy::WriteCombining
    } else {
        CachePolicy::Writeback
    }
}

closed spec fn raw_set_bits(raw: usize, p: u8, v: u8, cache: usize) -> usize {
    let flags = 1usize | 2usize | (((!p as usize) & 1usize) >> 0 << 61) | (((p as usize) & 2usize)
        >> 1 << 8) | (((!p as usize) & 4usize) >> 2 << 62) | (((p as usize) & 0x10usize) >> 4 << 1)
        | (((p as usize) & 8usize) >> 3 << 7) | (((p as usize) & 0x40usize) >> 6 << 10) | (((
    p as usize) & 0x80usize) >> 7 << 11);
    let flags = if v & 1u8 == 1u8 {
        flags | 4usize | 8usize
    } else {
        flags
    };
    let flags = if v & 2u8 == 2u8 {
        flags | if raw_is_huge(raw) {
            0x1000usize
        } else {
            0x40usize
        }
    } else {
        flags
    };
    (raw & 0x0000_FFFF_FFFF_F000usize) | (flags | cache)
}

closed spec fn cache_bits(cache: CachePolicy) -> usize {
    if cache is Writeback {
        0x10
    } else if cache is WriteCombining {
        0x20
    } else {
        0
    }
}

/// Inputs accepted by the raw property encoder without an unsupported-cache panic.
pub open spec fn supported_prop(prop: PageProperty) -> bool {
    &&& prop.inv()
    &&& prop.cache is Writeback || prop.cache is WriteCombining || prop.cache is Uncacheable
}

/// Exact round trips require DIRTY and exclude the legacy MAT=0 decoding mismatch.
pub open spec fn roundtrip_prop(prop: PageProperty) -> bool {
    &&& supported_prop(prop)
    &&& prop.flags.bits() & 0x10u8 != 0
    &&& prop.cache is Writeback || prop.cache is WriteCombining
}

impl PageTableEntry {
    pub closed spec fn default_spec() -> Self {
        Self(0)
    }

    /// The native PTE word has the layout required by the shared page-table code.
    ///
    /// # Preconditions
    /// None.
    /// # Postconditions
    /// Size and alignment are both eight bytes, so arrays preserve alignment.
    pub proof fn lemma_layout()
        ensures
            core::mem::size_of::<PageTableEntry>() == 8,
            core::mem::align_of::<PageTableEntry>() == 8,
            core::mem::size_of::<PageTableEntry>() % core::mem::align_of::<PageTableEntry>() == 0,
    {
        broadcast use VERUS_layout_of_PageTableEntry;

    }

    pub closed spec fn raw_set_prop_spec(raw: usize, prop: PageProperty) -> usize {
        raw_set_bits(raw, prop.flags.bits(), prop.priv_flags.bits(), cache_bits(prop.cache))
    }

    const PHYS_ADDR_MASK: usize = 0x0000_FFFF_FFFF_F000;

    closed spec fn is_user_spec(&self) -> bool {
        self.0 & 0xCusize == 0xCusize
    }

    #[verifier::when_used_as_spec(is_user_spec)]
    fn is_user(&self) -> bool
        returns
            self.is_user_spec(),
    {
        proof {
            lemma_flag_constants();
            lemma_raw_helpers(self.0);
        }
        self.0 & PageTableFlags::PLVL().bits() != 0 && self.0 & PageTableFlags::PLVH().bits() != 0
    }

    closed spec fn is_huge_spec(&self) -> bool {
        raw_is_huge(self.0)
    }

    #[verifier::when_used_as_spec(is_huge_spec)]
    fn is_huge(&self) -> bool
        returns
            self.is_huge_spec(),
    {
        proof {
            lemma_flag_constants();
            lemma_raw_helpers(self.0);
        }
        if self.0 & PageTableFlags::IS_BASIC().bits() != 0 {
            return false;
        } else {
            return self.0 & PageTableFlags::GLOBAL_OR_HUGE().bits() != 0;
        }
    }

    closed spec fn is_global_spec(&self) -> bool {
        raw_is_global(self.0)
    }

    #[verifier::when_used_as_spec(is_global_spec)]
    fn is_global(&self) -> bool
        returns
            self.is_global_spec(),
    {
        proof {
            lemma_flag_constants();
            lemma_raw_helpers(self.0);
        }
        if self.0 & PageTableFlags::IS_BASIC().bits() != 0 {
            return self.0 & PageTableFlags::GLOBAL_OR_HUGE().bits() != 0;
        } else {
            return self.0 & PageTableFlags::GLOBAL_IN_HUGE().bits() != 0;
        }
    }
}

} // verus!
/// Parse a bit-flag bits `val` in the representation of `from` to `to` in bits.
macro_rules! parse_flags {
    ($val:expr, $from:expr, $to:expr) => {
        ($val as usize & $from.bits() as usize) >> $from.bits().ilog2() << $to.bits().ilog2()
    };
}

verus! {

impl PageTableEntry {
    fn set_prop_inner(&mut self, prop: PageProperty)
        requires
            supported_prop(prop),
        ensures
            final(self).0 == Self::raw_set_prop_spec(old(self).0, prop),
    {
        proof {
            lemma_flag_constants();
        }

        let mut flags =
            PageTableFlags::VALID().bits()
        // FIXME: To avoid the PageModifyFault exception,
        // we set the DIRTY bit to 1 all the time.
         | PageTableFlags::DIRTY().bits()
            | parse_flags!(
                !prop.flags.bits(),
                PageFlags::R(),
                PageTableFlags::NOT_READABLE()
            )
            | parse_flags!(prop.flags.bits(), PageFlags::W(), PageTableFlags::WRITABLE())
            | parse_flags!(
                !prop.flags.bits(),
                PageFlags::X(),
                PageTableFlags::NOT_EXECUTABLE()
            )
            | parse_flags!(prop.flags.bits(), PageFlags::DIRTY(), PageTableFlags::DIRTY())
        // TODO: How to get the accessed bit in loongarch?
         | parse_flags!(prop.flags.bits(), PageFlags::ACCESSED(), PageTableFlags::PRESENT())
            | parse_flags!(prop.flags.bits(), PageFlags::AVAIL1(), PageTableFlags::RSV1())
            | parse_flags!(prop.flags.bits(), PageFlags::AVAIL2(), PageTableFlags::RSV2());
        if prop.priv_flags.contains(PrivFlags::USER()) {
            flags |= PageTableFlags::PLVL().bits();
            flags |= PageTableFlags::PLVH().bits();
        }
        if prop.priv_flags.contains(PrivFlags::GLOBAL()) {
            if self.is_huge() {
                flags |= PageTableFlags::GLOBAL_IN_HUGE().bits();
            } else {
                flags |= PageTableFlags::GLOBAL_OR_HUGE().bits();
            }
        }
        match prop.cache {
            CachePolicy::Writeback => {
                flags |= PageTableFlags::MATL().bits();
            },
            CachePolicy::Uncacheable => (),
            CachePolicy::WriteCombining => {
                flags |= PageTableFlags::MATH().bits();
            },
            _ => panic!("unsupported cache policy"),
        }

        proof {
            lemma_encode_matches(
                self.0,
                prop.flags.bits(),
                prop.priv_flags.bits(),
                cache_bits(prop.cache),
            );
        }
        proof {
            assert(flags | 0usize == flags) by (bit_vector);
        }
        self.0 = (self.0 & Self::PHYS_ADDR_MASK) | flags;
    }
}

impl PageTableEntryTrait for PageTableEntry {
    closed spec fn new_absent_spec() -> Self {
        Self(0)
    }

    fn new_absent() -> Self {
        proof {
            lemma_absent();
        }
        Self(0)
    }

    closed spec fn is_present_spec(&self) -> bool {
        self.0 & 1usize != 0
    }

    fn is_present(&self) -> bool {
        proof {
            lemma_flag_constants();
        }
        self.0 & PageTableFlags::VALID().bits() != 0
    }

    closed spec fn new_page_spec(paddr: Paddr, level: PagingLevel, prop: PageProperty) -> Self {
        Self(
            Self::raw_set_prop_spec(paddr & Self::PHYS_ADDR_MASK, prop) | if level == 1 {
                0x200usize
            } else {
                0x40usize
            },
        )
    }

    open spec fn new_page_req(paddr: Paddr, level: PagingLevel, prop: PageProperty) -> bool {
        level == 1 && roundtrip_prop(prop)
    }

    /// Constructs a supported 4 KiB leaf.
    ///
    /// # Verified Properties
    /// ## Safety
    /// Encodes an integer PTE without accessing mapped memory.
    /// ## Functional Correctness
    /// The leaf round-trips its supported properties and aligned physical address.
    /// ## Preconditions
    /// `paddr < MAX_PADDR`, level 1, valid flags including DIRTY, and Writeback
    /// or WriteCombining caching. USER and GLOBAL are both supported.
    /// ## Postconditions
    /// The entry is present and terminal; its address is `paddr` rounded down
    /// to a base page. These preconditions exclude the unsupported-cache panic.
    fn new_page(paddr: Paddr, level: PagingLevel, prop: PageProperty) -> Self {
        proof {
            lemma_flag_constants();
            lemma_new_page(paddr, level, prop);
        }
        let flags = if level == 1 {
            PageTableFlags::IS_BASIC().bits()
        } else {
            PageTableFlags::GLOBAL_OR_HUGE().bits()
        };
        let mut pte = Self(paddr & Self::PHYS_ADDR_MASK);
        pte.set_prop_inner(prop);
        let pte = pte.0 | flags;
        Self(pte)
    }

    closed spec fn new_pt_spec(paddr: Paddr) -> Self {
        Self(paddr & Self::PHYS_ADDR_MASK | 1usize)
    }

    fn new_pt(paddr: Paddr) -> Self {
        proof {
            lemma_flag_constants();
            lemma_new_pt(paddr);
        }
        Self(paddr & Self::PHYS_ADDR_MASK | PageTableFlags::VALID().bits())
    }

    closed spec fn paddr_spec(&self) -> Paddr {
        raw_paddr(self.0)
    }

    fn paddr(&self) -> Paddr {
        proof {
            lemma_flag_constants();
            lemma_raw_helpers(self.0);
        }
        if self.is_huge() {
            let paddr = (self.0 & Self::PHYS_ADDR_MASK & !PageTableFlags::GLOBAL_IN_HUGE().bits())
                >> 12;
            paddr << 12
        } else {
            let ppn = (self.0 & Self::PHYS_ADDR_MASK) >> 12;
            ppn << 12
        }
    }

    closed spec fn prop_spec(&self) -> PageProperty {
        PageProperty {
            flags: PageFlags::from_bits(decode_flags(self.0))->0,
            cache: decode_cache(self.0),
            priv_flags: PrivFlags::from_bits(decode_priv(self.0))->0,
        }
    }

    /// Decodes the existing encoding, including MAT=0 reading as Writeback.
    ///
    /// # Verified Properties
    /// ## Safety
    /// Decoded bits are valid before either `from_bits` result is unwrapped.
    /// ## Functional Correctness
    /// Returns the exact permission, cache, and privilege decoding of the raw word.
    /// ## Preconditions
    /// None; arbitrary encoded words are supported.
    /// ## Postconditions
    /// The result equals `prop_spec`; this does not promise an unrestricted
    /// round trip through `set_prop`.
    fn prop(&self) -> PageProperty {
        proof {
            lemma_flag_constants();
            lemma_decode_matches(self.0);
            lemma_raw_helpers(self.0);
        }
        let flags = (parse_flags!(!(self.0), PageTableFlags::NOT_READABLE(), PageFlags::R())) | (
        parse_flags!(self.0, PageTableFlags::WRITABLE(), PageFlags::W())) | (
        parse_flags!(!(self.0), PageTableFlags::NOT_EXECUTABLE(), PageFlags::X()))
        // TODO: How to get the accessed bit in loongarch?
         | (parse_flags!(self.0, PageTableFlags::PRESENT(), PageFlags::ACCESSED())) | (
        parse_flags!(self.0, PageTableFlags::DIRTY(), PageFlags::DIRTY())) | (
        parse_flags!(self.0, PageTableFlags::RSV1(), PageFlags::AVAIL1())) | (
        parse_flags!(self.0, PageTableFlags::RSV2(), PageFlags::AVAIL2()));
        let mut priv_flags = PrivFlags::empty().bits();
        if self.is_user() {
            priv_flags |= PrivFlags::USER().bits();
        }
        if self.is_global() {
            priv_flags |= PrivFlags::GLOBAL().bits();
        }
        proof {
            let raw = self.0;
            assert(priv_flags == decode_priv(raw)) by (bit_vector)
                requires
                    priv_flags == (if raw_is_global(raw) {
                        (if raw & 0xCusize == 0xCusize {
                            0u8 | 1u8
                        } else {
                            0u8
                        }) | 2u8
                    } else {
                        if raw & 0xCusize == 0xCusize {
                            0u8 | 1u8
                        } else {
                            0u8
                        }
                    }),
            ;
        }
        let cache = if self.0 & PageTableFlags::MATL().bits() != 0 {
            CachePolicy::Writeback
        } else if self.0 & PageTableFlags::MATH().bits() != 0 {
            CachePolicy::WriteCombining
        } else {
            CachePolicy::Writeback
        };

        PageProperty {
            flags: PageFlags::from_bits(flags as u8).unwrap(),
            cache,
            priv_flags: PrivFlags::from_bits(priv_flags as u8).unwrap(),
        }
    }

    open spec fn set_prop_req(self, prop: PageProperty) -> bool {
        &&& self.is_present()
        &&& !raw_is_huge(self.as_usize())
        &&& self.as_usize() & 0x1000usize == 0
        &&& prop.priv_flags.bits() & 2u8 == 0
        &&& roundtrip_prop(prop)
    }

    /// Updates properties within the legacy setter's address-preserving domain.
    ///
    /// # Verified Properties
    /// ## Safety
    /// Mutates only the owned PTE word; TLB synchronization is outside this proof.
    /// ## Functional Correctness
    /// The updated entry has exactly the requested property.
    /// ## Preconditions
    /// The old entry is present and non-huge, with address bit 12 clear.
    /// Input flags are valid, include DIRTY, exclude GLOBAL, and select
    /// Writeback or WriteCombining caching. Absent-entry updates are excluded.
    /// ## Postconditions
    /// Physical address, presence, and terminal status are preserved. The
    /// preconditions exclude the unsupported-cache panic.
    fn set_prop(&mut self, prop: PageProperty) {
        proof {
            lemma_set_prop(old(self).0, prop);
        }
        self.set_prop_inner(prop);
    }

    closed spec fn is_last_spec(&self, level: PagingLevel) -> bool {
        level == 1 || raw_is_huge(self.0)
    }

    fn is_last(&self, level: PagingLevel) -> bool {
        level == 1 || self.is_huge()
    }

    closed spec fn as_usize_spec(self) -> usize {
        self.0
    }

    fn as_usize(self) -> usize {
        self.0
    }

    proof fn lemma_paddr_is_page_aligned(self) {
        lemma_raw_helpers(self.0);
    }

    proof fn lemma_page_table_entry_properties() {
        broadcast use VERUS_layout_of_PageTableEntry;

        lemma_absent();
        assert forall|paddr: Paddr, level: PagingLevel, prop: PageProperty|
            #![trigger Self::new_page(paddr, level, prop)]
            Self::new_page_req(paddr, level, prop) implies {
            &&& Self::new_page(paddr, level, prop).is_present()
            &&& paddr < MAX_PADDR ==> Self::new_page(paddr, level, prop).paddr() == paddr
                & !0xFFFusize
            &&& paddr < MAX_PADDR && paddr % PAGE_SIZE == 0 ==> Self::new_page(
                paddr,
                level,
                prop,
            ).paddr() == paddr
            &&& Self::new_page(paddr, level, prop).prop() == prop
            &&& Self::new_page(paddr, level, prop).is_last(level)
        } by {
            lemma_new_page(paddr, level, prop);
        }
        assert forall|paddr: Paddr|
            #![trigger Self::new_pt(paddr)]
            {
                &&& Self::new_pt(paddr).is_present()
                &&& paddr < MAX_PADDR ==> Self::new_pt(paddr).paddr() == paddr & !0xFFFusize
                &&& paddr < MAX_PADDR && paddr % PAGE_SIZE == 0 ==> Self::new_pt(paddr).paddr()
                    == paddr
                &&& forall|level: PagingLevel| 1 < level ==> !Self::new_pt(paddr).is_last(level)
            } by {
            lemma_new_pt(paddr);
        }
    }
}

proof fn lemma_flag_constants()
    ensures
        PageTableFlags::VALID().bits() == 0x1usize,
        PageTableFlags::VALID().bits().ilog2() == 0,
        PageTableFlags::DIRTY().bits() == 0x2usize,
        PageTableFlags::DIRTY().bits().ilog2() == 1,
        PageTableFlags::PLVL().bits() == 0x4usize,
        PageTableFlags::PLVL().bits().ilog2() == 2,
        PageTableFlags::PLVH().bits() == 0x8usize,
        PageTableFlags::PLVH().bits().ilog2() == 3,
        PageTableFlags::MATL().bits() == 0x10usize,
        PageTableFlags::MATL().bits().ilog2() == 4,
        PageTableFlags::MATH().bits() == 0x20usize,
        PageTableFlags::MATH().bits().ilog2() == 5,
        PageTableFlags::GLOBAL_OR_HUGE().bits() == 0x40usize,
        PageTableFlags::GLOBAL_OR_HUGE().bits().ilog2() == 6,
        PageTableFlags::PRESENT().bits() == 0x80usize,
        PageTableFlags::PRESENT().bits().ilog2() == 7,
        PageTableFlags::WRITABLE().bits() == 0x100usize,
        PageTableFlags::WRITABLE().bits().ilog2() == 8,
        PageTableFlags::IS_BASIC().bits() == 0x200usize,
        PageTableFlags::IS_BASIC().bits().ilog2() == 9,
        PageTableFlags::RSV1().bits() == 0x400usize,
        PageTableFlags::RSV1().bits().ilog2() == 10,
        PageTableFlags::RSV2().bits() == 0x800usize,
        PageTableFlags::RSV2().bits().ilog2() == 11,
        PageTableFlags::GLOBAL_IN_HUGE().bits() == 0x1000usize,
        PageTableFlags::GLOBAL_IN_HUGE().bits().ilog2() == 12,
        PageTableFlags::NOT_READABLE().bits() == 0x2000000000000000usize,
        PageTableFlags::NOT_READABLE().bits().ilog2() == 61,
        PageTableFlags::NOT_EXECUTABLE().bits() == 0x4000000000000000usize,
        PageTableFlags::NOT_EXECUTABLE().bits().ilog2() == 62,
        PageTableFlags::RPLV().bits() == 0x8000000000000000usize,
        PageTableFlags::RPLV().bits().ilog2() == 63,
        PageFlags::R().bits() == 0x1u8,
        PageFlags::R().bits().ilog2() == 0,
        PageFlags::W().bits() == 0x2u8,
        PageFlags::W().bits().ilog2() == 1,
        PageFlags::X().bits() == 0x4u8,
        PageFlags::X().bits().ilog2() == 2,
        PageFlags::ACCESSED().bits() == 0x8u8,
        PageFlags::ACCESSED().bits().ilog2() == 3,
        PageFlags::DIRTY().bits() == 0x10u8,
        PageFlags::DIRTY().bits().ilog2() == 4,
        PageFlags::AVAIL1().bits() == 0x40u8,
        PageFlags::AVAIL1().bits().ilog2() == 6,
        PageFlags::AVAIL2().bits() == 0x80u8,
        PageFlags::AVAIL2().bits().ilog2() == 7,
        PrivFlags::USER().bits() == 0x1u8,
        PrivFlags::USER().bits().ilog2() == 0,
        PrivFlags::GLOBAL().bits() == 0x2u8,
        PrivFlags::GLOBAL().bits().ilog2() == 1,
        PageFlags::all().bits() == 0xDFu8,
        PrivFlags::all().bits() == 3u8,
{
    lemma_usize_ilog2_to32();
    lemma_u8_ilog2_to8();
    lemma_u64_ilog2_to64();
    broadcast use PageTableFlags::lemma_consts;

    assert(PageTableFlags::VALID().bits() == 0x1usize) by (compute);
    assert(PageTableFlags::DIRTY().bits() == 0x2usize) by (compute);
    assert(PageTableFlags::PLVL().bits() == 0x4usize) by (compute);
    assert(PageTableFlags::PLVH().bits() == 0x8usize) by (compute);
    assert(PageTableFlags::MATL().bits() == 0x10usize) by (compute);
    assert(PageTableFlags::MATH().bits() == 0x20usize) by (compute);
    assert(PageTableFlags::GLOBAL_OR_HUGE().bits() == 0x40usize) by (compute);
    assert(PageTableFlags::PRESENT().bits() == 0x80usize) by (compute);
    assert(PageTableFlags::WRITABLE().bits() == 0x100usize) by (compute);
    assert(PageTableFlags::IS_BASIC().bits() == 0x200usize) by (compute);
    assert(PageTableFlags::RSV1().bits() == 0x400usize) by (compute);
    assert(PageTableFlags::RSV2().bits() == 0x800usize) by (compute);
    assert(PageTableFlags::GLOBAL_IN_HUGE().bits() == 0x1000usize) by (compute);
    assert(PageTableFlags::NOT_READABLE().bits() == 0x2000000000000000usize) by (compute);
    assert(PageTableFlags::NOT_EXECUTABLE().bits() == 0x4000000000000000usize) by (compute);
    assert(PageTableFlags::RPLV().bits() == 0x8000000000000000usize) by (compute);
    broadcast use PageFlags::lemma_consts;
    broadcast use PrivFlags::lemma_consts;

    PageFlags::lemma_all_constant();
    PrivFlags::lemma_all_constant();
    assert((0u8 | 1u8 | 2u8 | 4u8 | 3u8 | 5u8 | 7u8 | 8u8 | 0x10u8 | 0x40u8 | 0x80u8) == 0xDFu8)
        by (compute_only);
    assert((0u8 | 1u8 | 2u8 | 0u8) == 3u8) by (compute_only);
}

#[verifier::bit_vector]
proof fn lemma_raw_helpers(raw: usize)
    ensures
        (raw & 4usize != 0 && raw & 8usize != 0) == (raw & 0xCusize == 0xCusize),
        raw_paddr(raw) % 4096usize == 0,
        raw_paddr(raw) < 0x1_0000_0000_0000usize,
        ((raw & 0x0000_FFFF_FFFF_F000usize) >> 12) << 12 == raw & 0x0000_FFFF_FFFF_F000usize,
        ((raw & 0x0000_FFFF_FFFF_F000usize & !0x1000usize) >> 12) << 12 == raw
            & 0x0000_FFFF_FFFF_E000usize,
        decode_flags(raw) & 0xDFu8 == decode_flags(raw),
        decode_priv(raw) & 3u8 == decode_priv(raw),
{
}

#[verifier::bit_vector]
proof fn lemma_decode_bits(raw: usize)
    ensures
        (((!raw & 0x2000_0000_0000_0000usize) >> 61 << 0) | ((raw & 0x100usize) >> 8 << 1) | ((!raw
            & 0x4000_0000_0000_0000usize) >> 62 << 2) | ((raw & 0x80usize) >> 7 << 3) | ((raw
            & 2usize) >> 1 << 4) | ((raw & 0x400usize) >> 10 << 6) | ((raw & 0x800usize) >> 11
            << 7)) == decode_flags(raw) as usize,
{
}

proof fn lemma_decode_matches(raw: usize)
    ensures
        (parse_flags!(!raw, PageTableFlags::NOT_READABLE(), PageFlags::R())
            | parse_flags!(raw, PageTableFlags::WRITABLE(), PageFlags::W())
            | parse_flags!(!raw, PageTableFlags::NOT_EXECUTABLE(), PageFlags::X())
            | parse_flags!(raw, PageTableFlags::PRESENT(), PageFlags::ACCESSED())
            | parse_flags!(raw, PageTableFlags::DIRTY(), PageFlags::DIRTY())
            | parse_flags!(raw, PageTableFlags::RSV1(), PageFlags::AVAIL1())
            | parse_flags!(raw, PageTableFlags::RSV2(), PageFlags::AVAIL2())) == decode_flags(
            raw,
        ) as usize,
{
    lemma_flag_constants();
    lemma_raw_helpers(raw);
    lemma_decode_bits(raw);
}

proof fn lemma_encode_matches(raw: usize, p: u8, v: u8, cache: usize)
    ensures
        raw_set_bits(raw, p, v, cache) == {
            let flags = PageTableFlags::VALID().bits() | PageTableFlags::DIRTY().bits()
                | parse_flags!(!p, PageFlags::R(), PageTableFlags::NOT_READABLE())
                | parse_flags!(p, PageFlags::W(), PageTableFlags::WRITABLE())
                | parse_flags!(!p, PageFlags::X(), PageTableFlags::NOT_EXECUTABLE())
                | parse_flags!(p, PageFlags::DIRTY(), PageTableFlags::DIRTY())
                | parse_flags!(p, PageFlags::ACCESSED(), PageTableFlags::PRESENT())
                | parse_flags!(p, PageFlags::AVAIL1(), PageTableFlags::RSV1())
                | parse_flags!(p, PageFlags::AVAIL2(), PageTableFlags::RSV2());
            let flags = if v & 1u8 == 1u8 {
                flags | PageTableFlags::PLVL().bits() | PageTableFlags::PLVH().bits()
            } else {
                flags
            };
            let flags = if v & 2u8 == 2u8 {
                flags | if raw_is_huge(raw) {
                    PageTableFlags::GLOBAL_IN_HUGE().bits()
                } else {
                    PageTableFlags::GLOBAL_OR_HUGE().bits()
                }
            } else {
                flags
            };
            (raw & PageTableEntry::PHYS_ADDR_MASK) | (flags | cache)
        },
{
    lemma_flag_constants();
}

#[verifier::bit_vector]
proof fn lemma_new_page_bits(pa: usize, p: u8, v: u8, cache: usize)
    requires
        p & 0xDFu8 == p,
        p & 0x10u8 != 0,
        v & 3u8 == v,
        cache == 0x10usize || cache == 0x20usize,
    ensures
        decode_flags(raw_set_bits(pa & 0x0000_FFFF_FFFF_F000usize, p, v, cache) | 0x200usize) == p,
        decode_priv(raw_set_bits(pa & 0x0000_FFFF_FFFF_F000usize, p, v, cache) | 0x200usize) == v,
        raw_paddr(raw_set_bits(pa & 0x0000_FFFF_FFFF_F000usize, p, v, cache) | 0x200usize) == pa
            & 0x0000_FFFF_FFFF_F000usize,
        (raw_set_bits(pa & 0x0000_FFFF_FFFF_F000usize, p, v, cache) | 0x200usize) & 1usize != 0,
        (raw_set_bits(pa & 0x0000_FFFF_FFFF_F000usize, p, v, cache) | 0x200usize) & 0x30usize
            == cache,
        pa < 0x8000_0000usize ==> pa & 0x0000_FFFF_FFFF_F000usize == pa & !0xFFFusize,
        pa < 0x8000_0000usize ==> pa & 0x0000_FFFF_FFFF_F000usize < 0x8000_0000usize,
        pa % 4096usize == 0 ==> pa & !0xFFFusize == pa,
{
}

#[verifier::bit_vector]
proof fn lemma_new_pt_bits(pa: usize)
    ensures
        (pa & 0x0000_FFFF_FFFF_F000usize | 1usize) & 1usize != 0,
        !raw_is_huge(pa & 0x0000_FFFF_FFFF_F000usize | 1usize),
        raw_paddr(pa & 0x0000_FFFF_FFFF_F000usize | 1usize) == pa & 0x0000_FFFF_FFFF_F000usize,
        pa < 0x8000_0000usize ==> raw_paddr(pa & 0x0000_FFFF_FFFF_F000usize | 1usize)
            < 0x8000_0000usize,
        pa < 0x8000_0000usize ==> raw_paddr(pa & 0x0000_FFFF_FFFF_F000usize | 1usize) == pa
            & !0xFFFusize,
        pa % 4096usize == 0 ==> pa & !0xFFFusize == pa,
{
}

#[verifier::bit_vector]
proof fn lemma_set_prop_bits(raw: usize, p: u8, v: u8, cache: usize)
    requires
        !raw_is_huge(raw),
        raw & 0x1000usize == 0,
        p & 0xDFu8 == p,
        p & 0x10u8 != 0,
        v & 3u8 == v,
        v & 2u8 == 0,
        cache == 0x10usize || cache == 0x20usize,
    ensures
        decode_flags(raw_set_bits(raw, p, v, cache)) == p,
        decode_priv(raw_set_bits(raw, p, v, cache)) == v,
        raw_paddr(raw_set_bits(raw, p, v, cache)) == raw_paddr(raw),
        raw_set_bits(raw, p, v, cache) & 1usize != 0,
        raw_set_bits(raw, p, v, cache) & 0x30usize == cache,
        !raw_is_huge(raw_set_bits(raw, p, v, cache)),
{
}

/// The shared constructor/update domain contains user mappings and remains
/// nonempty after an update. These are actual PTEs, not unconstrained words.
proof fn lemma_supported_mapping_witness()
    ensures
        ({
            let prop = PageProperty {
                flags: PageFlags::DIRTY(),
                cache: CachePolicy::Writeback,
                priv_flags: PrivFlags::USER(),
            };
            let pte = PageTableEntry::new_page(0x2000, 1, prop);
            let updated = PageTableEntry(PageTableEntry::raw_set_prop_spec(pte.0, prop));
            &&& PageTableEntry::new_page_req(0x2000, 1, prop)
            &&& forall|pa: Paddr, level: PagingLevel|
                1 <= level <= HIGHEST_TRANSLATION_LEVEL ==> #[trigger] PageTableEntry::new_page_req(
                    pa,
                    level,
                    prop,
                )
            &&& pte.set_prop_req(prop)
            &&& updated.set_prop_req(prop)
            &&& pte.paddr() == 0x2000
            &&& updated.paddr() == 0x2000
            &&& pte.prop() == prop
            &&& updated.prop() == prop
        }),
{
    lemma_flag_constants();
    let prop = PageProperty {
        flags: PageFlags::DIRTY(),
        cache: CachePolicy::Writeback,
        priv_flags: PrivFlags::USER(),
    };
    assert(0x10u8 & 0xDFu8 == 0x10u8 && 1u8 & 3u8 == 1u8 && 0x10u8 & 0x10u8 != 0 && 1u8 & 2u8 == 0)
        by (compute_only);
    lemma_new_page(0x2000, 1, prop);
    assert(0x2000usize & 0x0000_FFFF_FFFF_F000usize == 0x2000usize) by (compute_only);
    let raw = raw_set_bits(0x2000, 0x10, 1, 0x10) | 0x200usize;
    assert({
        let raw = raw_set_bits(0x2000, 0x10, 1, 0x10) | 0x200usize;
        !raw_is_huge(raw) && raw & 0x1000usize == 0
    }) by (compute_only);
    lemma_set_prop(raw, prop);
    lemma_set_prop_bits(raw, 0x10, 1, 0x10);
    assert(raw_set_bits(raw_set_bits(0x2000, 0x10, 1, 0x10) | 0x200usize, 0x10, 1, 0x10)
        & 0x1000usize == 0) by (compute_only);
}

/// Checked counterexamples explain why the legacy setter cannot satisfy the
/// unrestricted shared contract. The scalar encoder is the exact contract of
/// `set_prop_inner`; these facts require no hardware assumptions.
proof fn lemma_legacy_encoding_limits()
    ensures
// Losing IS_BASIC reinterprets basic GLOBAL as HUGE and drops PA bit 12.

        ({
            let before = raw_set_bits(0x1000, 0x11, 2, 0x10) | 0x200usize;
            let after = raw_set_bits(before, 0x11, 2, 0x10);
            &&& raw_paddr(before) == 0x1000
            &&& raw_paddr(after) == 0
            &&& !raw_is_huge(before)
            &&& raw_is_huge(after)
        }),
        // Without GLOBAL, losing IS_BASIC still decodes PA bit 12 as GLOBAL.
        ({
            let before = raw_set_bits(0x1000, 0x11, 0, 0x10) | 0x200usize;
            let after = raw_set_bits(before, 0x11, 0, 0x10);
            &&& decode_priv(before) == 0
            &&& decode_priv(after) == 2
        }),
        // UC is written as MAT=0 but read back as WB.
        cache_bits(CachePolicy::Uncacheable) == 0,
        decode_cache(raw_set_bits(0x2000, 0x11, 0, 0)) is Writeback,
        // The PageModifyFault workaround forces DIRTY even when it is absent.
        decode_flags(raw_set_bits(0x2000, 1, 0, 0x10)) == 0x11,
{
    assert({
        let before = raw_set_bits(0x1000, 0x11, 2, 0x10) | 0x200usize;
        let after = raw_set_bits(before, 0x11, 2, 0x10);
        raw_paddr(before) == 0x1000 && raw_paddr(after) == 0 && !raw_is_huge(before) && raw_is_huge(
            after,
        )
    }) by (compute_only);
    assert({
        let before = raw_set_bits(0x1000, 0x11, 0, 0x10) | 0x200usize;
        let after = raw_set_bits(before, 0x11, 0, 0x10);
        decode_priv(before) == 0 && decode_priv(after) == 2
    }) by (compute_only);
    assert(decode_cache(raw_set_bits(0x2000, 0x11, 0, 0)) is Writeback) by (compute_only);
    assert(decode_flags(raw_set_bits(0x2000, 1, 0, 0x10)) == 0x11) by (compute_only);
}

proof fn lemma_absent()
    ensures
        PageTableEntry::new_absent().paddr() == 0,
        !PageTableEntry::new_absent().is_present(),
        forall|level: PagingLevel| #[trigger]
            PageTableEntry::new_absent().is_last(level) == (level == 1),
{
    assert(raw_paddr(0) == 0 && !raw_is_huge(0) && 0usize & 1usize == 0) by (compute_only);
}

proof fn lemma_cache_decode(raw: usize, cache: CachePolicy)
    requires
        cache is Writeback || cache is WriteCombining,
        raw & 0x30usize == cache_bits(cache),
    ensures
        decode_cache(raw) == cache,
{
    assert(raw & 0x30usize == 0x10usize ==> raw & 0x10usize != 0) by (bit_vector);
    assert(raw & 0x30usize == 0x20usize ==> raw & 0x10usize == 0 && raw & 0x20usize != 0)
        by (bit_vector);
}

proof fn lemma_new_page(pa: Paddr, level: PagingLevel, prop: PageProperty)
    requires
        PageTableEntry::new_page_req(pa, level, prop),
    ensures
        PageTableEntry::new_page(pa, level, prop).is_present(),
        PageTableEntry::new_page(pa, level, prop).prop() == prop,
        PageTableEntry::new_page(pa, level, prop).is_last(level),
        pa < MAX_PADDR ==> PageTableEntry::new_page(pa, level, prop).paddr() == pa & !0xFFFusize,
        pa < MAX_PADDR && pa % PAGE_SIZE == 0 ==> PageTableEntry::new_page(pa, level, prop).paddr()
            == pa,
        pa < MAX_PADDR ==> crate::specs::arch::valid_frame_paddr(
            PageTableEntry::new_page(pa, level, prop).paddr(),
        ),
{
    lemma_flag_constants();
    lemma_new_page_bits(pa, prop.flags.bits(), prop.priv_flags.bits(), cache_bits(prop.cache));
    let raw = PageTableEntry::new_page(pa, level, prop).0;
    lemma_decode_matches(raw);
    lemma_raw_helpers(raw);
    lemma_cache_decode(raw, prop.cache);
    PageFlags::lemma_from_bits_bits(prop.flags.bits());
    PrivFlags::lemma_from_bits_bits(prop.priv_flags.bits());
    PageFlags::lemma_eq_from_bits(PageTableEntry(raw).prop().flags, prop.flags);
    PrivFlags::lemma_eq_from_bits(PageTableEntry(raw).prop().priv_flags, prop.priv_flags);
}

proof fn lemma_new_pt(pa: Paddr)
    ensures
        PageTableEntry::new_pt(pa).is_present(),
        forall|level: PagingLevel| 1 < level ==> !PageTableEntry::new_pt(pa).is_last(level),
        pa < MAX_PADDR ==> PageTableEntry::new_pt(pa).paddr() == pa & !0xFFFusize,
        pa < MAX_PADDR && pa % PAGE_SIZE == 0 ==> PageTableEntry::new_pt(pa).paddr() == pa,
        pa < MAX_PADDR ==> crate::specs::arch::valid_frame_paddr(
            PageTableEntry::new_pt(pa).paddr(),
        ),
{
    lemma_new_pt_bits(pa);
    lemma_raw_helpers(PageTableEntry::new_pt(pa).0);
}

proof fn lemma_set_prop(raw: usize, prop: PageProperty)
    requires
        PageTableEntry(raw).set_prop_req(prop),
    ensures
        PageTableEntry(PageTableEntry::raw_set_prop_spec(raw, prop)).prop() == prop,
        PageTableEntry(PageTableEntry::raw_set_prop_spec(raw, prop)).paddr() == PageTableEntry(
            raw,
        ).paddr(),
        PageTableEntry(PageTableEntry::raw_set_prop_spec(raw, prop)).is_present(),
        forall|level: PagingLevel| #[trigger]
            PageTableEntry(raw).is_last(level) ==> PageTableEntry(
                PageTableEntry::raw_set_prop_spec(raw, prop),
            ).is_last(level),
{
    lemma_flag_constants();
    lemma_set_prop_bits(raw, prop.flags.bits(), prop.priv_flags.bits(), cache_bits(prop.cache));
    let new_raw = PageTableEntry::raw_set_prop_spec(raw, prop);
    lemma_decode_matches(new_raw);
    lemma_cache_decode(new_raw, prop.cache);
    PageFlags::lemma_from_bits_bits(prop.flags.bits());
    PrivFlags::lemma_from_bits_bits(prop.priv_flags.bits());
    PageFlags::lemma_eq_from_bits(PageTableEntry(new_raw).prop().flags, prop.flags);
    PrivFlags::lemma_eq_from_bits(PageTableEntry(new_raw).prop().priv_flags, prop.priv_flags);
}

} // verus!
impl fmt::Debug for PageTableEntry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut f = f.debug_struct("PageTableEntry");
        f.field("raw", &format_args!("{:#x}", self.0))
            .field("paddr", &format_args!("{:#x}", self.paddr()))
            .field("present", &self.is_present())
            .field(
                "flags",
                &PageTableFlags::from_bits_truncate(self.0 & !Self::PHYS_ADDR_MASK),
            )
            .field("prop", &self.prop())
            .finish()
    }
}

pub(in crate::arch) fn paddr_to_daddr(pa: Paddr) -> usize {
    const DEVICE_LINEAR_MAPPING_BASE_VADDR: usize = 0x8000_0000_0000_0000;
    pa + DEVICE_LINEAR_MAPPING_BASE_VADDR
}

pub(crate) unsafe fn __memcpy_fallible(dst: *mut u8, src: *const u8, size: usize) -> usize {
    // TODO: implement fallible
    unsafe { core::ptr::copy(src, dst, size) };
    0
}

pub(crate) unsafe fn __memset_fallible(dst: *mut u8, value: u8, size: usize) -> usize {
    // TODO: implement fallible
    unsafe { core::ptr::write_bytes(dst, value, size) };
    0
}
