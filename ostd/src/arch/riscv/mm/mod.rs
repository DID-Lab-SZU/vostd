// SPDX-License-Identifier: MPL-2.0
use alloc::fmt;
use core::ops::Range;
use vstd::arithmetic::power2::{lemma2_to64, lemma2_to64_rest};
use vstd::prelude::*;
use vstd_extra::{
    panic::{may_panic, panic_diverge},
    prelude::*,
};

use crate::{
    mm::{
        page_prop::{CachePolicy, PageFlags, PageProperty, PrivilegedPageFlags as PrivFlags},
        page_table::PageTableEntryTrait,
        CurrentPagingConstsTrait, Paddr, PagingConstsTrait, PagingLevel, PodOnce, Vaddr, MAX_PADDR,
    },
    Pod,
};

verus! {

/// Size of a base page in Sv48.
pub const PAGE_SIZE: usize = 4096;

/// Size of an Sv48 page-table entry.
pub const PTE_SIZE: usize = 8;

/// Number of entries in an Sv48 page-table node.
pub const NR_ENTRIES: usize = 512;

/// Number of translation levels in Sv48.
pub const NR_LEVELS: usize = 4;

/// Width of canonical virtual addresses in Sv48.
pub const ADDRESS_WIDTH: usize = 48;

/// Exclusive upper bound of physical addresses encodable by an Sv48 PTE.
pub const MAX_ARCH_PADDR: Paddr = 0x100_0000_0000_0000;

/// Highest level at which an Sv48 PTE may directly map a page.
pub const HIGHEST_TRANSLATION_LEVEL: PagingLevel = 4;

/// Sv48 virtual addresses use sign extension from bit 47.
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
        assert(Self::BASE_PAGE_SIZE() == PAGE_SIZE) by (compute_only);
        assert(Self::NR_LEVELS() == NR_LEVELS as PagingLevel) by (compute_only);
        assert(Self::BASE_PAGE_SIZE() / Self::PTE_SIZE() == NR_ENTRIES) by (compute_only);
    }
}

pub proof fn lemma_nr_subpage_per_huge_eq_nr_entries()
    ensures
        crate::mm::nr_subpage_per_huge::<PagingConsts>() == NR_ENTRIES,
{
    assert(crate::mm::nr_subpage_per_huge::<PagingConsts>() == 4096usize / 8usize);
    assert(NR_ENTRIES == 512usize);
}

} // verus!

bitflags::bitflags! {
    /// Possible flags for a page table entry.
    pub struct PageTableFlags: usize {
        /// Specifies whether the mapped frame or page table is valid.
        const VALID =           1usize << 0;
        /// Controls whether reads to the mapped frames are allowed.
        const READABLE =        1usize << 1;
        /// Controls whether writes to the mapped frames are allowed.
        const WRITABLE =        1usize << 2;
        /// Controls whether execution code in the mapped frames are allowed.
        const EXECUTABLE =      1usize << 3;
        /// Controls whether accesses from userspace (i.e. U-mode) are permitted.
        const USER =            1usize << 4;
        /// Indicates that the mapping is present in all address spaces, so it isn't flushed from
        /// the TLB on an address space switch.
        const GLOBAL =          1usize << 5;
        /// Whether the memory area represented by this entry is accessed.
        const ACCESSED =        1usize << 6;
        /// Whether the memory area represented by this entry is modified.
        const DIRTY =           1usize << 7;

        // First bit ignored by MMU.
        const RSV1 =            1usize << 8;
        // Second bit ignored by MMU.
        const RSV2 =            1usize << 9;

        // PBMT: Non-cacheable, idempotent, weakly-ordered (RVWMO), main memory
        const PBMT_NC =         1usize << 61;
        // PBMT: Non-cacheable, non-idempotent, strongly-ordered (I/O ordering), I/O
        const PBMT_IO =         1usize << 62;
        /// Naturally aligned power-of-2
        const NAPOT =           1usize << 63;
    }
}

pub(crate) fn tlb_flush_addr(vaddr: Vaddr) {
    unsafe {
        riscv::asm::sfence_vma(0, vaddr);
    }
}

pub(crate) fn tlb_flush_addr_range(range: &Range<Vaddr>) {
    for vaddr in range.clone().step_by(PAGE_SIZE) {
        tlb_flush_addr(vaddr);
    }
}

pub(crate) fn tlb_flush_all_excluding_global() {
    // TODO: excluding global?
    riscv::asm::sfence_vma_all()
}

pub(crate) fn tlb_flush_all_including_global() {
    riscv::asm::sfence_vma_all()
}

verus! {

#[derive(Clone, Copy)]
#[repr(C)]
pub struct PageTableEntry(usize);

global layout PageTableEntry is size == 8, align == 8;

#[verus_verify]
unsafe impl Pod for PageTableEntry {}

impl PageTableEntry {
    pub proof fn lemma_layout()
        ensures
            core::mem::size_of::<PageTableEntry>() == 8,
            core::mem::align_of::<PageTableEntry>() == 8,
            core::mem::size_of::<PageTableEntry>() % core::mem::align_of::<PageTableEntry>() == 0,
    {
        broadcast use VERUS_layout_of_PageTableEntry;
    }

    pub closed spec fn default_spec() -> Self {
        Self(0)
    }
}

impl Default for PageTableEntry {
    fn default() -> (ret: Self)
        ensures
            ret.paddr() == 0,
        returns
            Self::default_spec(),
    {
        proof {
            lemma_riscv_flag_constants();
            assert(Self::default_spec().paddr_spec() == 0) by (compute);
        }
        Self(usize::default())
    }
}

} // verus!

/// Activate the given level 4 page table.
///
/// "satp" register doesn't have a field that encodes the cache policy,
/// so `_root_pt_cache` is ignored.
///
/// # Safety
///
/// Changing the level 4 page table is unsafe, because it's possible to violate memory safety by
/// changing the page mapping.
pub unsafe fn activate_page_table(root_paddr: Paddr, _root_pt_cache: CachePolicy) {
    assert!(root_paddr % PagingConsts::BASE_PAGE_SIZE() == 0);
    let ppn = root_paddr >> 12;
    riscv::register::satp::set(riscv::register::satp::Mode::Sv48, 0, ppn);
}

pub fn current_page_table_paddr() -> Paddr {
    riscv::register::satp::read().ppn() << 12
}

/// Parse a bit-flag bits `val` in the representation of `from` to `to` in bits.
macro_rules! parse_flags {
    ($val:expr, $from:expr, $to:expr) => {
        ($val as usize & $from.bits() as usize) >> $from.bits().ilog2() << $to.bits().ilog2()
    };
}

verus! {

impl PageTableEntry {
    const PHYS_ADDR_MASK: usize = 0x003F_FFFF_FFFF_FC00;
    pub const LEAF_PERM_MASK: usize = 0xE;
    /// Sv48 bits 60:54, PBMT_NC, and NAPOT are not modeled by this implementation.
    pub const FORBIDDEN_MASK: usize = 0xBFC0_0000_0000_0000;
    /// U, A, D, and PBMT_IO must be clear on a non-leaf PTE.
    pub const NON_LEAF_RESERVED_MASK: usize = 0x4000_0000_0000_00D0;

    /// The page size mapped by a leaf PTE at `level` under Sv48.
    pub open spec fn page_size_at_level_spec(level: PagingLevel) -> usize {
        if level == 1 {
            0x1000
        } else if level == 2 {
            0x20_0000
        } else if level == 3 {
            0x4000_0000
        } else if level == 4 {
            0x80_0000_0000
        } else {
            0
        }
    }

    /// Whether a PTE uses RSV1 as the tracked-frame tag.
    pub open spec fn has_tracked_tag(&self) -> bool {
        self.as_usize() & 0x100usize != 0
    }

    /// Hardware and software well-formedness for an Sv48 PTE at `level`.
    pub open spec fn pte_wf(&self, level: PagingLevel) -> bool {
        let raw = self.as_usize();
        &&& 1 <= level <= NR_LEVELS
        &&& raw & Self::FORBIDDEN_MASK == 0
        &&& self.paddr() < MAX_ARCH_PADDR
        &&& (!self.is_present() || if self.is_last(level) {
            &&& raw & PageTableFlags::READABLE().bits() != 0
            &&& self.paddr() % Self::page_size_at_level_spec(level) == 0
        } else {
            &&& 1 < level
            &&& raw & Self::NON_LEAF_RESERVED_MASK == 0
        })
    }

    /// Specification-only view of an arbitrary hardware PTE word.
    pub closed spec fn from_raw_spec(raw: usize) -> Self {
        Self(raw)
    }

    closed spec fn new_paddr_spec(paddr: Paddr) -> Self {
        Self((paddr >> 12) << 10 | PageTableFlags::VALID().bits())
    }

    fn new_paddr(paddr: Paddr) -> (res: Self)
        returns
            Self::new_paddr_spec(paddr),
    {
        let ppn = paddr >> 12;
        Self(ppn << 10 | PageTableFlags::VALID().bits())
    }

    pub closed spec fn raw_set_prop_spec(old_raw: usize, prop: PageProperty) -> usize {
        old_raw & Self::PHYS_ADDR_MASK | PageProperty::encode_riscv_prop_spec(prop)
    }
}

impl PodOnce for PageTableEntry {}

impl PageTableEntryTrait for PageTableEntry {
    open spec fn new_absent_spec() -> Self {
        Self::default_spec()
    }

    fn new_absent() -> Self {
        proof {
            lemma_riscv_flag_constants();
            Self::lemma_page_table_entry_properties();
        }
        Self::default()
    }

    open spec fn is_present_spec(&self) -> bool {
        self.as_usize() & PageTableFlags::VALID().bits() != 0
    }

    fn is_present(&self) -> bool {
        self.0 & PageTableFlags::VALID().bits() != 0
    }

    closed spec fn new_page_spec(paddr: Paddr, _level: PagingLevel, prop: PageProperty) -> Self {
        Self(Self::raw_set_prop_spec(Self::new_paddr_spec(paddr).0, prop))
    }

    open spec fn new_page_req(_paddr: Paddr, _level: PagingLevel, prop: PageProperty) -> bool {
        &&& prop.inv()
        &&& prop.flags.bits() & PageFlags::R().bits() == PageFlags::R().bits()
        &&& (prop.cache is Writeback || prop.cache is Uncacheable)
    }

    fn new_page(paddr: Paddr, _level: PagingLevel, prop: PageProperty) -> Self {
        let mut pte = Self::new_paddr(paddr);
        proof {
            lemma_riscv_flag_constants();
            lemma_riscv_new_paddr_bits(paddr);
            assert(pte.set_prop_req(prop));
        }
        pte.set_prop(prop);
        proof {
            lemma_riscv_page_align_mask(paddr);
        }
        pte
    }

    closed spec fn new_pt_spec(paddr: Paddr) -> Self {
        Self::new_paddr_spec(paddr)
    }

    fn new_pt(paddr: Paddr) -> Self {
        // In RISC-V, non-leaf PTE should have RWX = 000,
        // and D, A, and U are reserved for future standard use.
        let pte = Self::new_paddr(paddr);
        proof {
            lemma_riscv_flag_constants();
            lemma_riscv_new_paddr_bits(paddr);
            lemma_riscv_page_align_mask(paddr);
        }
        pte
    }

    closed spec fn paddr_spec(&self) -> Paddr {
        ((self.0 & Self::PHYS_ADDR_MASK) >> 10) << 12
    }

    fn paddr(&self) -> Paddr {
        proof {
            self.lemma_paddr_is_page_aligned();
        }
        let ppn = (self.0 & Self::PHYS_ADDR_MASK) >> 10;
        ppn << 12
    }

    closed spec fn prop_spec(&self) -> PageProperty {
        PageProperty {
            flags: PageFlags::from_bits(
                PageProperty::decode_riscv_page_flags_spec(self.0) as u8,
            )->0,
            cache: PageProperty::decode_riscv_cache_spec(self.0),
            priv_flags: PrivFlags::from_bits(
                PageProperty::decode_riscv_priv_flags_spec(self.0) as u8,
            )->0,
        }
    }

    fn prop(&self) -> PageProperty {
        proof {
            lemma_riscv_flag_constants();
        }
        let flags = (parse_flags!(self.0, PageTableFlags::READABLE(), PageFlags::R()))
            | (parse_flags!(self.0, PageTableFlags::WRITABLE(), PageFlags::W()))
            | (parse_flags!(self.0, PageTableFlags::EXECUTABLE(), PageFlags::X()))
            | (parse_flags!(self.0, PageTableFlags::ACCESSED(), PageFlags::ACCESSED()))
            | (parse_flags!(self.0, PageTableFlags::DIRTY(), PageFlags::DIRTY()))
            | (parse_flags!(self.0, PageTableFlags::RSV1(), PageFlags::AVAIL1()))
            | (parse_flags!(self.0, PageTableFlags::RSV2(), PageFlags::AVAIL2()));
        let priv_flags = (parse_flags!(self.0, PageTableFlags::USER(), PrivFlags::USER()))
            | (parse_flags!(self.0, PageTableFlags::GLOBAL(), PrivFlags::GLOBAL()));

        let cache = if self.0 & PageTableFlags::PBMT_IO().bits() != 0 {
            CachePolicy::Uncacheable
        } else {
            CachePolicy::Writeback
        };

        proof {
            lemma_riscv_parse_flags(self.0);
            lemma_riscv_decoded_flags_wf(self.0);
            let spec_prop = self.prop();
            assert(flags == PageProperty::decode_riscv_page_flags_spec(self.0));
            assert(priv_flags == PageProperty::decode_riscv_priv_flags_spec(self.0));
            assert(PageFlags::from_bits(flags as u8)->0 == spec_prop.flags);
            assert(PrivFlags::from_bits(priv_flags as u8)->0 == spec_prop.priv_flags);
            assert(cache == spec_prop.cache);
        }
        PageProperty {
            flags: PageFlags::from_bits(flags as u8).unwrap(),
            cache,
            priv_flags: PrivFlags::from_bits(priv_flags as u8).unwrap(),
        }
    }

    open spec fn set_prop_req(self, prop: PageProperty) -> bool {
        &&& self.is_present()
        &&& prop.inv()
        &&& prop.flags.bits() & PageFlags::R().bits() == PageFlags::R().bits()
        &&& (!(prop.cache is Writeback || prop.cache is Uncacheable) ==> may_panic())
    }

    fn set_prop(&mut self, prop: PageProperty)
        ensures
            final(self).as_usize() == Self::raw_set_prop_spec(old(self).as_usize(), prop),
            forall|level: PagingLevel| final(self).is_last(level),
    {
        proof {
            lemma_riscv_flag_constants();
        }
        let base_flags = PageTableFlags::VALID().bits()
            | parse_flags!(prop.flags.bits(), PageFlags::R(), PageTableFlags::READABLE())
            | parse_flags!(prop.flags.bits(), PageFlags::W(), PageTableFlags::WRITABLE())
            | parse_flags!(prop.flags.bits(), PageFlags::X(), PageTableFlags::EXECUTABLE())
            | parse_flags!(prop.flags.bits(), PageFlags::ACCESSED(), PageTableFlags::ACCESSED())
            | parse_flags!(prop.flags.bits(), PageFlags::DIRTY(), PageTableFlags::DIRTY())
            | parse_flags!(prop.flags.bits(), PageFlags::AVAIL1(), PageTableFlags::RSV1())
            | parse_flags!(prop.flags.bits(), PageFlags::AVAIL2(), PageTableFlags::RSV2())
            | parse_flags!(
                prop.priv_flags.bits(),
                PrivFlags::USER(),
                PageTableFlags::USER()
            )
            | parse_flags!(
                prop.priv_flags.bits(),
                PrivFlags::GLOBAL(),
                PageTableFlags::GLOBAL()
            );

        let flags = match prop.cache {
            CachePolicy::Writeback => {
                proof {
                    lemma_riscv_encode_page_property_matches(prop);
                    lemma_riscv_encode_prop_matches(prop.flags.bits(), prop.priv_flags.bits(), false);
                    let encoded_bits = encode_riscv_prop_bits_spec(
                        prop.flags.bits() as usize,
                        prop.priv_flags.bits() as usize,
                        false,
                    );
                    lemma_riscv_or_zero(base_flags);
                    assert(!(prop.cache is Uncacheable));
                    assert(PageProperty::encode_riscv_prop_spec(prop) == encoded_bits);
                    assert(base_flags == encoded_bits);
                    assert(base_flags == PageProperty::encode_riscv_prop_spec(prop));
                }
                base_flags
            }
            // Currently, Asterinas uses `Uncacheable` for I/O memory.
            CachePolicy::Uncacheable => {
                let flags = base_flags | PageTableFlags::PBMT_IO().bits();
                proof {
                    lemma_riscv_encode_page_property_matches(prop);
                    lemma_riscv_encode_prop_matches(prop.flags.bits(), prop.priv_flags.bits(), true);
                    assert(flags == PageProperty::encode_riscv_prop_spec(prop));
                }
                flags
            }
            _ => panic_diverge(),
        };

        proof {
            assert(flags == PageProperty::encode_riscv_prop_spec(prop));
        }
        self.0 = (self.0 & Self::PHYS_ADDR_MASK) | flags;
        proof {
            lemma_riscv_set_prop_roundtrip(old(self).0, prop);
        }
    }

    open spec fn is_last_spec(&self, level: PagingLevel) -> bool {
        level == 1 || self.as_usize() & Self::LEAF_PERM_MASK != 0
    }

    fn is_last(&self, level: PagingLevel) -> bool {
        let rwx = PageTableFlags::READABLE().bits()
            | PageTableFlags::WRITABLE().bits()
            | PageTableFlags::EXECUTABLE().bits();
        proof {
            lemma_riscv_flag_constants();
            assert(rwx == Self::LEAF_PERM_MASK) by (bit_vector)
                requires
                    rwx == 0x2usize | 0x4usize | 0x8usize,
                    Self::LEAF_PERM_MASK == 0xEusize,
            ;
        }
        level == 1 || (self.0 & rwx) != 0
    }

    closed spec fn as_usize_spec(self) -> usize {
        self.0
    }

    fn as_usize(self) -> usize {
        self.0
    }

    proof fn lemma_page_table_entry_properties() {
        lemma_riscv_flag_constants();
        PagingConsts::lemma_paging_consts_requirements();
        Self::lemma_layout();

        assert(Self::new_absent().as_usize() == 0);
        assert(Self::new_absent().paddr() == 0) by (compute);
        assert(crate::specs::arch::valid_frame_paddr(Self::new_absent().paddr()));
        assert(!Self::new_absent().is_present()) by (compute);

        assert forall|level: PagingLevel| 1 < level implies !(#[trigger] Self::new_absent()
            .is_last(level)) by {
            assert(level != 1);
            assert(0usize & Self::LEAF_PERM_MASK == 0) by (compute_only);
            assert(Self::new_absent().is_last(level) == (level == 1
                || Self::new_absent().as_usize() & Self::LEAF_PERM_MASK != 0));
        }

        assert forall|paddr: Paddr, level: PagingLevel, prop: PageProperty|
            #![trigger Self::new_page(paddr, level, prop)]
            Self::new_page_req(paddr, level, prop) && (prop.cache is Writeback
                || prop.cache is Writethrough || prop.cache is Uncacheable) implies {
                &&& Self::new_page(paddr, level, prop).is_present()
                &&& (paddr < MAX_PADDR ==> Self::new_page(paddr, level, prop).paddr() == paddr
                    & !((PAGE_SIZE - 1) as usize))
                &&& (paddr < MAX_PADDR && paddr % PAGE_SIZE == 0 ==> Self::new_page(
                    paddr,
                    level,
                    prop,
                ).paddr() == paddr)
                &&& Self::new_page(paddr, level, prop).prop() == prop
                &&& Self::new_page(paddr, level, prop).is_last(level)
            }
        by {
            lemma_riscv_new_page_properties(paddr, level, prop);
        }

        assert forall|paddr: Paddr| #![trigger Self::new_pt(paddr)] {
                &&& Self::new_pt(paddr).is_present()
                &&& (paddr < MAX_PADDR ==> Self::new_pt(paddr).paddr() == paddr & !((PAGE_SIZE
                    - 1) as usize))
                &&& (paddr < MAX_PADDR && paddr % PAGE_SIZE == 0 ==> Self::new_pt(paddr).paddr()
                    == paddr)
                &&& forall|level: PagingLevel| 1 < level ==> !Self::new_pt(paddr).is_last(level)
            }
        by {
            lemma_riscv_new_pt_properties(paddr);
        }
    }

    proof fn lemma_paddr_is_page_aligned(self) {
        lemma_riscv_raw_paddr_aligned(self.0);
    }
}

impl PageProperty {
    closed spec fn encode_riscv_prop_spec(prop: Self) -> usize {
        PageTableFlags::VALID().bits()
            | parse_flags!(prop.flags.bits(), PageFlags::R(), PageTableFlags::READABLE())
            | parse_flags!(prop.flags.bits(), PageFlags::W(), PageTableFlags::WRITABLE())
            | parse_flags!(prop.flags.bits(), PageFlags::X(), PageTableFlags::EXECUTABLE())
            | parse_flags!(prop.flags.bits(), PageFlags::ACCESSED(), PageTableFlags::ACCESSED())
            | parse_flags!(prop.flags.bits(), PageFlags::DIRTY(), PageTableFlags::DIRTY())
            | parse_flags!(prop.flags.bits(), PageFlags::AVAIL1(), PageTableFlags::RSV1())
            | parse_flags!(prop.flags.bits(), PageFlags::AVAIL2(), PageTableFlags::RSV2())
            | parse_flags!(prop.priv_flags.bits(), PrivFlags::USER(), PageTableFlags::USER())
            | parse_flags!(prop.priv_flags.bits(), PrivFlags::GLOBAL(), PageTableFlags::GLOBAL())
            | if prop.cache is Uncacheable {
                PageTableFlags::PBMT_IO().bits()
            } else {
                0usize
            }
    }

    closed spec fn decode_riscv_page_flags_spec(raw: usize) -> usize {
        parse_flags!(raw, PageTableFlags::READABLE(), PageFlags::R())
            | parse_flags!(raw, PageTableFlags::WRITABLE(), PageFlags::W())
            | parse_flags!(raw, PageTableFlags::EXECUTABLE(), PageFlags::X())
            | parse_flags!(raw, PageTableFlags::ACCESSED(), PageFlags::ACCESSED())
            | parse_flags!(raw, PageTableFlags::DIRTY(), PageFlags::DIRTY())
            | parse_flags!(raw, PageTableFlags::RSV1(), PageFlags::AVAIL1())
            | parse_flags!(raw, PageTableFlags::RSV2(), PageFlags::AVAIL2())
    }

    closed spec fn decode_riscv_priv_flags_spec(raw: usize) -> usize {
        parse_flags!(raw, PageTableFlags::USER(), PrivFlags::USER())
            | parse_flags!(raw, PageTableFlags::GLOBAL(), PrivFlags::GLOBAL())
    }

    closed spec fn decode_riscv_cache_spec(raw: usize) -> CachePolicy {
        if raw & PageTableFlags::PBMT_IO().bits() != 0 {
            CachePolicy::Uncacheable
        } else {
            CachePolicy::Writeback
        }
    }
}

closed spec fn encode_riscv_prop_bits_spec(
    pbits: usize,
    priv_bits: usize,
    uncacheable: bool,
) -> usize {
    0x1usize
        | if pbits & 0x1usize != 0 { 0x2usize } else { 0 }
        | if pbits & 0x2usize != 0 { 0x4usize } else { 0 }
        | if pbits & 0x4usize != 0 { 0x8usize } else { 0 }
        | if pbits & 0x8usize != 0 { 0x40usize } else { 0 }
        | if pbits & 0x10usize != 0 { 0x80usize } else { 0 }
        | if pbits & 0x40usize != 0 { 0x100usize } else { 0 }
        | if pbits & 0x80usize != 0 { 0x200usize } else { 0 }
        | if priv_bits & 0x1usize != 0 { 0x10usize } else { 0 }
        | if priv_bits & 0x2usize != 0 { 0x20usize } else { 0 }
        | if uncacheable { 0x4000_0000_0000_0000usize } else { 0 }
}

closed spec fn decode_riscv_page_bits_spec(raw: usize) -> usize {
    (if raw & 0x2usize != 0 { 0x1usize } else { 0 })
        | (if raw & 0x4usize != 0 { 0x2usize } else { 0 })
        | (if raw & 0x8usize != 0 { 0x4usize } else { 0 })
        | (if raw & 0x40usize != 0 { 0x8usize } else { 0 })
        | (if raw & 0x80usize != 0 { 0x10usize } else { 0 })
        | (if raw & 0x100usize != 0 { 0x40usize } else { 0 })
        | (if raw & 0x200usize != 0 { 0x80usize } else { 0 })
}

closed spec fn decode_riscv_priv_bits_spec(raw: usize) -> usize {
    (if raw & 0x10usize != 0 { 0x1usize } else { 0 })
        | (if raw & 0x20usize != 0 { 0x2usize } else { 0 })
}

#[verifier::bit_vector]
proof fn lemma_riscv_shifted_flags_match(raw: usize)
    ensures
        (((raw & 0x2usize) >> 1 << 0)
            | ((raw & 0x4usize) >> 2 << 1)
            | ((raw & 0x8usize) >> 3 << 2)
            | ((raw & 0x40usize) >> 6 << 3)
            | ((raw & 0x80usize) >> 7 << 4)
            | ((raw & 0x100usize) >> 8 << 6)
            | ((raw & 0x200usize) >> 9 << 7)) == decode_riscv_page_bits_spec(raw),
        (((raw & 0x10usize) >> 4 << 0) | ((raw & 0x20usize) >> 5 << 1))
            == decode_riscv_priv_bits_spec(raw),
{
}

#[verifier::bit_vector]
proof fn lemma_riscv_decoded_bits_wf(raw: usize)
    ensures
        decode_riscv_page_bits_spec(raw) <= 0xDFusize,
        decode_riscv_page_bits_spec(raw) & 0xDFusize == decode_riscv_page_bits_spec(raw),
        decode_riscv_priv_bits_spec(raw) <= 0x3usize,
        decode_riscv_priv_bits_spec(raw) & 0x3usize == decode_riscv_priv_bits_spec(raw),
{
}

#[verifier::bit_vector]
proof fn lemma_riscv_encode_bits_expanded(pbits: u8, priv_bits: u8, uncacheable: bool)
    ensures
        (0x1usize
            | ((pbits as usize & 0x1usize) >> 0 << 1)
            | ((pbits as usize & 0x2usize) >> 1 << 2)
            | ((pbits as usize & 0x4usize) >> 2 << 3)
            | ((pbits as usize & 0x8usize) >> 3 << 6)
            | ((pbits as usize & 0x10usize) >> 4 << 7)
            | ((pbits as usize & 0x40usize) >> 6 << 8)
            | ((pbits as usize & 0x80usize) >> 7 << 9)
            | ((priv_bits as usize & 0x1usize) >> 0 << 4)
            | ((priv_bits as usize & 0x2usize) >> 1 << 5)
            | if uncacheable { 0x4000_0000_0000_0000usize } else { 0usize })
            == encode_riscv_prop_bits_spec(pbits as usize, priv_bits as usize, uncacheable),
{
}

#[verifier::bit_vector]
proof fn lemma_riscv_page_align_mask(paddr: Paddr)
    ensures
        paddr % 4096usize == 0 ==> paddr & !0xFFFusize == paddr,
{
}

#[verifier::bit_vector]
proof fn lemma_riscv_raw_paddr_aligned(raw: usize)
    ensures
        (((raw & PageTableEntry::PHYS_ADDR_MASK) >> 10) << 12) % 4096usize == 0,
{
}

#[verifier::bit_vector]
proof fn lemma_riscv_or_zero(bits: usize)
    ensures
        bits | 0usize == bits,
{
}

#[verifier::bit_vector]
proof fn lemma_riscv_single_bit_nonzero(bits: u8)
    ensures
        (bits & 0x40u8 != 0) <==> (bits & 0x40u8 == 0x40u8),
{
}

proof fn lemma_riscv_flag_constants()
    ensures
        PageTableFlags::VALID().bits() == 0x1usize,
        PageTableFlags::READABLE().bits() == 0x2usize,
        PageTableFlags::WRITABLE().bits() == 0x4usize,
        PageTableFlags::EXECUTABLE().bits() == 0x8usize,
        PageTableFlags::USER().bits() == 0x10usize,
        PageTableFlags::GLOBAL().bits() == 0x20usize,
        PageTableFlags::ACCESSED().bits() == 0x40usize,
        PageTableFlags::DIRTY().bits() == 0x80usize,
        PageTableFlags::RSV1().bits() == 0x100usize,
        PageTableFlags::RSV2().bits() == 0x200usize,
        PageTableFlags::PBMT_NC().bits() == 0x2000_0000_0000_0000usize,
        PageTableFlags::PBMT_IO().bits() == 0x4000_0000_0000_0000usize,
        PageTableFlags::NAPOT().bits() == 0x8000_0000_0000_0000usize,
        PageTableFlags::VALID().bits().ilog2() == 0,
        PageTableFlags::READABLE().bits().ilog2() == 1,
        PageTableFlags::WRITABLE().bits().ilog2() == 2,
        PageTableFlags::EXECUTABLE().bits().ilog2() == 3,
        PageTableFlags::USER().bits().ilog2() == 4,
        PageTableFlags::GLOBAL().bits().ilog2() == 5,
        PageTableFlags::ACCESSED().bits().ilog2() == 6,
        PageTableFlags::DIRTY().bits().ilog2() == 7,
        PageTableFlags::RSV1().bits().ilog2() == 8,
        PageTableFlags::RSV2().bits().ilog2() == 9,
        PageTableFlags::PBMT_IO().bits().ilog2() == 62,
        PageFlags::R().bits() == 0x1u8,
        PageFlags::W().bits() == 0x2u8,
        PageFlags::X().bits() == 0x4u8,
        PageFlags::ACCESSED().bits() == 0x8u8,
        PageFlags::DIRTY().bits() == 0x10u8,
        PageFlags::AVAIL1().bits() == 0x40u8,
        PageFlags::AVAIL2().bits() == 0x80u8,
        PageFlags::all().bits() == 0xDFu8,
        PageFlags::R().bits().ilog2() == 0,
        PageFlags::W().bits().ilog2() == 1,
        PageFlags::X().bits().ilog2() == 2,
        PageFlags::ACCESSED().bits().ilog2() == 3,
        PageFlags::DIRTY().bits().ilog2() == 4,
        PageFlags::AVAIL1().bits().ilog2() == 6,
        PageFlags::AVAIL2().bits().ilog2() == 7,
        PrivFlags::USER().bits() == 0x1u8,
        PrivFlags::GLOBAL().bits() == 0x2u8,
        #[cfg(not(all(target_arch = "x86_64", feature = "cvm_guest")))]
        PrivFlags::all().bits() == 0x3u8,
        PrivFlags::USER().bits().ilog2() == 0,
        PrivFlags::GLOBAL().bits().ilog2() == 1,
        PAGE_SIZE == 4096usize,
        MAX_PADDR == 0x8000_0000usize,
{
    lemma_usize_ilog2_to32();
    lemma_u8_ilog2_to8();
    lemma_u64_ilog2_to64();
    broadcast use PageTableFlags::lemma_consts;
    broadcast use PageFlags::lemma_consts;
    broadcast use PrivFlags::lemma_consts;
    PageFlags::lemma_all_constant();
    assert(PageTableFlags::VALID().bits() == 0x1usize) by (compute);
    assert(PageTableFlags::READABLE().bits() == 0x2usize) by (compute);
    assert(PageTableFlags::WRITABLE().bits() == 0x4usize) by (compute);
    assert(PageTableFlags::EXECUTABLE().bits() == 0x8usize) by (compute);
    assert(PageTableFlags::USER().bits() == 0x10usize) by (compute);
    assert(PageTableFlags::GLOBAL().bits() == 0x20usize) by (compute);
    assert(PageTableFlags::ACCESSED().bits() == 0x40usize) by (compute);
    assert(PageTableFlags::DIRTY().bits() == 0x80usize) by (compute);
    assert(PageTableFlags::RSV1().bits() == 0x100usize) by (compute);
    assert(PageTableFlags::RSV2().bits() == 0x200usize) by (compute);
    assert(PageTableFlags::PBMT_NC().bits() == 0x2000_0000_0000_0000usize) by (compute);
    assert(PageTableFlags::PBMT_IO().bits() == 0x4000_0000_0000_0000usize) by (compute);
    assert(PageTableFlags::NAPOT().bits() == 0x8000_0000_0000_0000usize) by (compute);
    assert((0u8 | 0x1u8 | 0x2u8 | 0x4u8 | 0x3u8 | 0x5u8 | 0x7u8 | 0x8u8 | 0x10u8
        | 0x40u8 | 0x80u8) == 0xDFu8) by (compute_only);
    #[cfg(not(all(target_arch = "x86_64", feature = "cvm_guest")))]
    {
        PrivFlags::lemma_all_constant();
        assert((0u8 | 0x1u8 | 0x2u8 | 0u8) == 0x3u8) by (compute_only);
    }
}

proof fn lemma_riscv_parse_flags(raw: usize)
    ensures
        (parse_flags!(raw, PageTableFlags::READABLE(), PageFlags::R()))
            | (parse_flags!(raw, PageTableFlags::WRITABLE(), PageFlags::W()))
            | (parse_flags!(raw, PageTableFlags::EXECUTABLE(), PageFlags::X()))
            | (parse_flags!(raw, PageTableFlags::ACCESSED(), PageFlags::ACCESSED()))
            | (parse_flags!(raw, PageTableFlags::DIRTY(), PageFlags::DIRTY()))
            | (parse_flags!(raw, PageTableFlags::RSV1(), PageFlags::AVAIL1()))
            | (parse_flags!(raw, PageTableFlags::RSV2(), PageFlags::AVAIL2()))
                == PageProperty::decode_riscv_page_flags_spec(raw),
        (parse_flags!(raw, PageTableFlags::USER(), PrivFlags::USER()))
            | (parse_flags!(raw, PageTableFlags::GLOBAL(), PrivFlags::GLOBAL()))
                == PageProperty::decode_riscv_priv_flags_spec(raw),
        PageProperty::decode_riscv_page_flags_spec(raw) == decode_riscv_page_bits_spec(raw),
        PageProperty::decode_riscv_priv_flags_spec(raw) == decode_riscv_priv_bits_spec(raw),
{
    lemma_riscv_flag_constants();
    assert(parse_flags!(raw, PageTableFlags::READABLE(), PageFlags::R())
        == ((raw & 0x2usize) >> 1 << 0)) by (compute);
    assert(parse_flags!(raw, PageTableFlags::WRITABLE(), PageFlags::W())
        == ((raw & 0x4usize) >> 2 << 1)) by (compute);
    assert(parse_flags!(raw, PageTableFlags::EXECUTABLE(), PageFlags::X())
        == ((raw & 0x8usize) >> 3 << 2)) by (compute);
    assert(parse_flags!(raw, PageTableFlags::ACCESSED(), PageFlags::ACCESSED())
        == ((raw & 0x40usize) >> 6 << 3)) by (compute);
    assert(parse_flags!(raw, PageTableFlags::DIRTY(), PageFlags::DIRTY())
        == ((raw & 0x80usize) >> 7 << 4)) by (compute);
    assert(parse_flags!(raw, PageTableFlags::RSV1(), PageFlags::AVAIL1())
        == ((raw & 0x100usize) >> 8 << 6)) by (compute);
    assert(parse_flags!(raw, PageTableFlags::RSV2(), PageFlags::AVAIL2())
        == ((raw & 0x200usize) >> 9 << 7)) by (compute);
    assert(parse_flags!(raw, PageTableFlags::USER(), PrivFlags::USER())
        == ((raw & 0x10usize) >> 4 << 0)) by (compute);
    assert(parse_flags!(raw, PageTableFlags::GLOBAL(), PrivFlags::GLOBAL())
        == ((raw & 0x20usize) >> 5 << 1)) by (compute);

    lemma_riscv_shifted_flags_match(raw);
}

proof fn lemma_riscv_decoded_flags_wf(raw: usize)
    ensures
        PageProperty::decode_riscv_page_flags_spec(raw) <= u8::MAX,
        PageProperty::decode_riscv_page_flags_spec(raw) & 0xDFusize
            == PageProperty::decode_riscv_page_flags_spec(raw),
        PageProperty::decode_riscv_priv_flags_spec(raw) <= u8::MAX,
        PageProperty::decode_riscv_priv_flags_spec(raw) & 0x3usize
            == PageProperty::decode_riscv_priv_flags_spec(raw),
{
    lemma_riscv_flag_constants();
    lemma_riscv_parse_flags(raw);
    let page_bits = decode_riscv_page_bits_spec(raw);
    let priv_bits = decode_riscv_priv_bits_spec(raw);
    lemma_riscv_decoded_bits_wf(raw);
}

proof fn lemma_riscv_encode_prop_matches(
    pbits: u8,
    priv_bits: u8,
    uncacheable: bool,
)
    ensures
        (PageTableFlags::VALID().bits()
            | parse_flags!(pbits, PageFlags::R(), PageTableFlags::READABLE())
            | parse_flags!(pbits, PageFlags::W(), PageTableFlags::WRITABLE())
            | parse_flags!(pbits, PageFlags::X(), PageTableFlags::EXECUTABLE())
            | parse_flags!(pbits, PageFlags::ACCESSED(), PageTableFlags::ACCESSED())
            | parse_flags!(pbits, PageFlags::DIRTY(), PageTableFlags::DIRTY())
            | parse_flags!(pbits, PageFlags::AVAIL1(), PageTableFlags::RSV1())
            | parse_flags!(pbits, PageFlags::AVAIL2(), PageTableFlags::RSV2())
            | parse_flags!(priv_bits, PrivFlags::USER(), PageTableFlags::USER())
            | parse_flags!(priv_bits, PrivFlags::GLOBAL(), PageTableFlags::GLOBAL())
            | if uncacheable { PageTableFlags::PBMT_IO().bits() } else { 0usize })
            == encode_riscv_prop_bits_spec(pbits as usize, priv_bits as usize, uncacheable),
{
    lemma_riscv_flag_constants();
    lemma_riscv_encode_bits_expanded(pbits, priv_bits, uncacheable);
}

proof fn lemma_riscv_encode_page_property_matches(prop: PageProperty)
    requires
        prop.cache is Writeback || prop.cache is Uncacheable,
    ensures
        PageProperty::encode_riscv_prop_spec(prop) == encode_riscv_prop_bits_spec(
            prop.flags.bits() as usize,
            prop.priv_flags.bits() as usize,
            prop.cache is Uncacheable,
        ),
{
    lemma_riscv_encode_prop_matches(
        prop.flags.bits(),
        prop.priv_flags.bits(),
        prop.cache is Uncacheable,
    );
}

#[verifier::bit_vector]
proof fn lemma_riscv_set_prop_bits(
    old_raw: usize,
    pbits: usize,
    priv_bits: usize,
    uncacheable: bool,
)
    requires
        pbits & 0xDFusize == pbits,
        priv_bits & 0x3usize == priv_bits,
        pbits & 0x1usize == 0x1usize,
    ensures
        {
            let flags = encode_riscv_prop_bits_spec(pbits, priv_bits, uncacheable);
            let new_raw = old_raw & PageTableEntry::PHYS_ADDR_MASK | flags;
            decode_riscv_page_bits_spec(new_raw) == pbits
        },
        {
            let flags = encode_riscv_prop_bits_spec(pbits, priv_bits, uncacheable);
            let new_raw = old_raw & PageTableEntry::PHYS_ADDR_MASK | flags;
            decode_riscv_priv_bits_spec(new_raw) == priv_bits
        },
        {
            let flags = encode_riscv_prop_bits_spec(pbits, priv_bits, uncacheable);
            let new_raw = old_raw & PageTableEntry::PHYS_ADDR_MASK | flags;
            new_raw & 0x4000_0000_0000_0000usize != 0 <==> uncacheable
        },
        {
            let flags = encode_riscv_prop_bits_spec(pbits, priv_bits, uncacheable);
            flags & PageTableEntry::PHYS_ADDR_MASK == 0
        },
        {
            let flags = encode_riscv_prop_bits_spec(pbits, priv_bits, uncacheable);
            let new_raw = old_raw & PageTableEntry::PHYS_ADDR_MASK | flags;
            new_raw & PageTableEntry::PHYS_ADDR_MASK
                == old_raw & PageTableEntry::PHYS_ADDR_MASK
        },
        {
            let flags = encode_riscv_prop_bits_spec(pbits, priv_bits, uncacheable);
            let new_raw = old_raw & PageTableEntry::PHYS_ADDR_MASK | flags;
            new_raw & 0x1usize != 0
        },
        {
            let flags = encode_riscv_prop_bits_spec(pbits, priv_bits, uncacheable);
            let new_raw = old_raw & PageTableEntry::PHYS_ADDR_MASK | flags;
            new_raw & 0xEusize != 0
        },
        {
            let flags = encode_riscv_prop_bits_spec(pbits, priv_bits, uncacheable);
            let new_raw = old_raw & PageTableEntry::PHYS_ADDR_MASK | flags;
            new_raw & 0x2usize != 0
        },
        {
            let flags = encode_riscv_prop_bits_spec(pbits, priv_bits, uncacheable);
            let new_raw = old_raw & PageTableEntry::PHYS_ADDR_MASK | flags;
            new_raw & PageTableEntry::FORBIDDEN_MASK == 0
        },
        {
            let flags = encode_riscv_prop_bits_spec(pbits, priv_bits, uncacheable);
            let new_raw = old_raw & PageTableEntry::PHYS_ADDR_MASK | flags;
            (((new_raw & PageTableEntry::PHYS_ADDR_MASK) >> 10) << 12) < MAX_ARCH_PADDR
        },
        {
            let flags = encode_riscv_prop_bits_spec(pbits, priv_bits, uncacheable);
            let new_raw = old_raw & PageTableEntry::PHYS_ADDR_MASK | flags;
            (new_raw & 0x100usize != 0) <==> (pbits & 0x40usize != 0)
        },
{
}

#[verifier::bit_vector]
proof fn lemma_riscv_new_paddr_bits(paddr: Paddr)
    ensures
        {
            let raw = (paddr >> 12) << 10 | 0x1usize;
            raw & 0x1usize != 0
        },
        {
            let raw = (paddr >> 12) << 10 | 0x1usize;
            raw & 0xEusize == 0
        },
        paddr < 0x8000_0000usize ==> {
            let raw = (paddr >> 12) << 10 | 0x1usize;
            raw & PageTableEntry::FORBIDDEN_MASK == 0
        },
        paddr < 0x8000_0000usize ==> {
            let raw = (paddr >> 12) << 10 | 0x1usize;
            raw & PageTableEntry::NON_LEAF_RESERVED_MASK == 0
        },
        {
            let raw = (paddr >> 12) << 10 | 0x1usize;
            (((raw & PageTableEntry::PHYS_ADDR_MASK) >> 10) << 12) % 4096usize == 0
        },
        paddr < 0x8000_0000usize ==> {
            let raw = (paddr >> 12) << 10 | 0x1usize;
            ((raw & PageTableEntry::PHYS_ADDR_MASK) >> 10) << 12
                == paddr & !0xFFFusize
        },
        {
            let raw = (paddr >> 12) << 10 | 0x1usize;
            (((raw & PageTableEntry::PHYS_ADDR_MASK) >> 10) << 12) < MAX_ARCH_PADDR
        },
        paddr < 0x8000_0000usize ==> {
            let raw = (paddr >> 12) << 10 | 0x1usize;
            (((raw & PageTableEntry::PHYS_ADDR_MASK) >> 10) << 12) < 0x8000_0000usize
        },
{
}

proof fn lemma_riscv_set_prop_roundtrip(old_raw: usize, prop: PageProperty)
    requires
        old_raw & PageTableFlags::VALID().bits() != 0,
        prop.inv(),
        prop.flags.bits() & PageFlags::R().bits() == PageFlags::R().bits(),
        prop.cache is Writeback || prop.cache is Uncacheable,
    ensures
        PageTableEntry(PageTableEntry::raw_set_prop_spec(old_raw, prop)).prop() == prop,
        PageTableEntry(PageTableEntry::raw_set_prop_spec(old_raw, prop)).paddr()
            == PageTableEntry(old_raw).paddr(),
        PageTableEntry(PageTableEntry::raw_set_prop_spec(old_raw, prop)).is_present(),
        forall|level: PagingLevel|
            PageTableEntry(PageTableEntry::raw_set_prop_spec(old_raw, prop)).is_last(level),
        forall|level: PagingLevel|
            #[trigger] PageTableEntry(old_raw).is_last(level) ==>
                PageTableEntry(PageTableEntry::raw_set_prop_spec(old_raw, prop)).is_last(level),
{
    lemma_riscv_flag_constants();
    let pbits_u8 = prop.flags.bits();
    let priv_bits_u8 = prop.priv_flags.bits();
    lemma_riscv_encode_prop_matches(pbits_u8, priv_bits_u8, prop.cache is Uncacheable);
    let pbits = pbits_u8 as usize;
    let priv_bits = priv_bits_u8 as usize;
    let uncacheable = prop.cache is Uncacheable;
    let flags = encode_riscv_prop_bits_spec(pbits, priv_bits, uncacheable);
    let new_raw = old_raw & PageTableEntry::PHYS_ADDR_MASK | flags;
    lemma_riscv_set_prop_bits(old_raw, pbits, priv_bits, uncacheable);
    assert(PageTableEntry::raw_set_prop_spec(old_raw, prop) == new_raw);
    lemma_riscv_parse_flags(new_raw);
    PageFlags::lemma_from_bits_bits(prop.flags.bits());
    PrivFlags::lemma_from_bits_bits(prop.priv_flags.bits());
    PageFlags::lemma_eq_from_bits(PageTableEntry(new_raw).prop().flags, prop.flags);
    PrivFlags::lemma_eq_from_bits(PageTableEntry(new_raw).prop().priv_flags, prop.priv_flags);
    assert forall|level: PagingLevel| PageTableEntry(new_raw).is_last(level) by {
        assert(new_raw & PageTableEntry::LEAF_PERM_MASK != 0);
    }
}

proof fn lemma_riscv_new_page_properties(
    paddr: Paddr,
    level: PagingLevel,
    prop: PageProperty,
)
    requires
        PageTableEntry::new_page_req(paddr, level, prop),
    ensures
        PageTableEntry::new_page(paddr, level, prop).is_present(),
        paddr < MAX_PADDR ==> PageTableEntry::new_page(paddr, level, prop).paddr()
            == paddr & !((PAGE_SIZE - 1) as usize),
        paddr < MAX_PADDR && paddr % PAGE_SIZE == 0 ==>
            PageTableEntry::new_page(paddr, level, prop).paddr() == paddr,
        PageTableEntry::new_page(paddr, level, prop).prop() == prop,
        forall|level: PagingLevel| PageTableEntry::new_page(paddr, level, prop).is_last(level),
{
    lemma_riscv_flag_constants();
    lemma_riscv_new_paddr_bits(paddr);
    let old_raw = PageTableEntry::new_paddr_spec(paddr).0;
    lemma_riscv_set_prop_roundtrip(old_raw, prop);
    lemma_riscv_page_align_mask(paddr);
}

proof fn lemma_riscv_new_pt_properties(paddr: Paddr)
    ensures
        PageTableEntry::new_pt(paddr).is_present(),
        paddr < MAX_PADDR ==> PageTableEntry::new_pt(paddr).paddr()
            == paddr & !((PAGE_SIZE - 1) as usize),
        paddr < MAX_PADDR && paddr % PAGE_SIZE == 0 ==> PageTableEntry::new_pt(paddr).paddr()
            == paddr,
        forall|level: PagingLevel| 1 < level ==> !PageTableEntry::new_pt(paddr).is_last(level),
{
    lemma_riscv_flag_constants();
    lemma_riscv_new_paddr_bits(paddr);
    lemma_riscv_page_align_mask(paddr);
}

/// A zero/absent PTE satisfies the Sv48 encoding rules at every valid level.
pub proof fn lemma_riscv_absent_pte_wf(level: PagingLevel)
    requires
        1 <= level <= NR_LEVELS,
    ensures
        PageTableEntry::new_absent().pte_wf(level),
{
    lemma_riscv_flag_constants();
    PagingConsts::lemma_paging_consts_requirements();
    assert(PageTableEntry::new_absent().as_usize() == 0);
    assert(PageTableEntry::new_absent().paddr() == 0) by (compute);
    assert(!PageTableEntry::new_absent().is_present()) by (compute);
    assert(PageTableEntry::new_absent().as_usize() & PageTableEntry::FORBIDDEN_MASK == 0)
        by (compute);
    assert(PageTableEntry::new_absent().paddr() < MAX_ARCH_PADDR) by (compute);
}

/// A page constructor is well formed when its PPN is aligned for the selected leaf level.
pub proof fn lemma_riscv_new_page_pte_wf(
    paddr: Paddr,
    level: PagingLevel,
    prop: PageProperty,
)
    requires
        paddr < MAX_PADDR,
        paddr % PAGE_SIZE == 0,
        1 <= level <= NR_LEVELS,
        paddr % PageTableEntry::page_size_at_level_spec(level) == 0,
        PageTableEntry::new_page_req(paddr, level, prop),
    ensures
        PageTableEntry::new_page(paddr, level, prop).pte_wf(level),
{
    lemma_riscv_flag_constants();
    lemma_riscv_new_page_properties(paddr, level, prop);
    let pbits = prop.flags.bits() as usize;
    let priv_bits = prop.priv_flags.bits() as usize;
    let uncacheable = prop.cache is Uncacheable;
    let old_raw = PageTableEntry::new_paddr_spec(paddr).0;
    let flags = encode_riscv_prop_bits_spec(pbits, priv_bits, uncacheable);
    let new_raw = old_raw & PageTableEntry::PHYS_ADDR_MASK | flags;
    lemma_riscv_encode_page_property_matches(prop);
    lemma_riscv_new_paddr_bits(paddr);
    lemma_riscv_set_prop_bits(old_raw, pbits, priv_bits, uncacheable);
    assert(PageTableEntry::new_page(paddr, level, prop).as_usize() == new_raw);
    assert(PageTableEntry::new_page(paddr, level, prop).paddr() == paddr);
    assert(PageTableEntry::new_page(paddr, level, prop).is_present());
    assert(PageTableEntry::new_page(paddr, level, prop).is_last(level));
    assert(new_raw & PageTableEntry::FORBIDDEN_MASK == 0);
    assert(new_raw & PageTableFlags::READABLE().bits() != 0);
    assert(PageTableEntry::new_page(paddr, level, prop).paddr() < MAX_ARCH_PADDR);
}

/// A child-page-table PTE has no leaf-only or extension bits set.
pub proof fn lemma_riscv_new_pt_pte_wf(paddr: Paddr, level: PagingLevel)
    requires
        paddr < MAX_PADDR,
        2 <= level <= NR_LEVELS,
    ensures
        PageTableEntry::new_pt(paddr).pte_wf(level),
{
    lemma_riscv_flag_constants();
    lemma_riscv_new_pt_properties(paddr);
    lemma_riscv_new_paddr_bits(paddr);
    assert(PageTableEntry::new_pt(paddr).as_usize() == PageTableEntry::new_paddr_spec(paddr).0);
    assert(PageTableEntry::new_pt(paddr).is_present());
    assert(!PageTableEntry::new_pt(paddr).is_last(level));
    assert(PageTableEntry::new_pt(paddr).as_usize() & PageTableEntry::FORBIDDEN_MASK == 0);
    assert(PageTableEntry::new_pt(paddr).as_usize() & PageTableEntry::NON_LEAF_RESERVED_MASK == 0);
    assert(PageTableEntry::new_pt(paddr).paddr() < MAX_ARCH_PADDR);
}

/// Setting properties on an existing leaf preserves its PPN and Sv48 well-formedness.
pub proof fn lemma_riscv_set_prop_preserves_pte_wf(
    old_raw: usize,
    level: PagingLevel,
    prop: PageProperty,
)
    requires
        PageTableEntry::from_raw_spec(old_raw).pte_wf(level),
        PageTableEntry::from_raw_spec(old_raw).is_present(),
        PageTableEntry::from_raw_spec(old_raw).is_last(level),
        prop.inv(),
        prop.flags.bits() & PageFlags::R().bits() == PageFlags::R().bits(),
        prop.cache is Writeback || prop.cache is Uncacheable,
    ensures
        PageTableEntry::from_raw_spec(
            PageTableEntry::raw_set_prop_spec(old_raw, prop),
        ).pte_wf(level),
        PageTableEntry::from_raw_spec(
            PageTableEntry::raw_set_prop_spec(old_raw, prop),
        ).has_tracked_tag()
            == prop.flags.contains(PageFlags::AVAIL1()),
        (prop.flags.contains(PageFlags::AVAIL1())
            == PageTableEntry::from_raw_spec(old_raw).has_tracked_tag()) ==>
            PageTableEntry::from_raw_spec(
                PageTableEntry::raw_set_prop_spec(old_raw, prop),
            ).has_tracked_tag() == PageTableEntry::from_raw_spec(old_raw).has_tracked_tag(),
{
    lemma_riscv_flag_constants();
    lemma_riscv_set_prop_roundtrip(old_raw, prop);
    lemma_riscv_encode_page_property_matches(prop);
    let pbits = prop.flags.bits() as usize;
    let priv_bits = prop.priv_flags.bits() as usize;
    let uncacheable = prop.cache is Uncacheable;
    let flags = encode_riscv_prop_bits_spec(pbits, priv_bits, uncacheable);
    let new_raw = old_raw & PageTableEntry::PHYS_ADDR_MASK | flags;
    lemma_riscv_set_prop_bits(old_raw, pbits, priv_bits, uncacheable);
    lemma_riscv_single_bit_nonzero(prop.flags.bits());
    assert(PageTableEntry::raw_set_prop_spec(old_raw, prop) == new_raw);
    assert(PageTableEntry(new_raw).is_present());
    assert(PageTableEntry(new_raw).is_last(level));
    assert(PageTableEntry(new_raw).paddr() == PageTableEntry(old_raw).paddr());
    assert(new_raw & PageTableEntry::FORBIDDEN_MASK == 0);
    assert(new_raw & PageTableFlags::READABLE().bits() != 0);
    assert(PageTableEntry(new_raw).paddr() < MAX_ARCH_PADDR);
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

pub(crate) fn __memcpy_fallible(dst: *mut u8, src: *const u8, size: usize) -> usize {
    // TODO: implement fallible
    unsafe {
        riscv::register::sstatus::set_sum();
    }
    unsafe { core::ptr::copy(src, dst, size) };
    0
}

pub(crate) fn __memset_fallible(dst: *mut u8, value: u8, size: usize) -> usize {
    // TODO: implement fallible
    unsafe {
        riscv::register::sstatus::set_sum();
    }
    unsafe { core::ptr::write_bytes(dst, value, size) };
    0
}
