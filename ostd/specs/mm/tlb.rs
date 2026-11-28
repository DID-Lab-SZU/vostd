//! The abstract TLB model.
//!
//! [`TlbModel`] is a tracked resource that holds one [`CpuTlb`] per CPU. Each
//! [`CpuTlb`] bounds what that CPU's TLB may cache: an entry is either backed by
//! the current page table of a resident address space, or it is in `stale`.
//! Hardware fills and evictions keep this bound, so they are not steps.
//!
//! The model keeps its state private, so a proof cannot forge it or drop stale
//! entries. Removing stale entries is reserved for the contracts of the
//! hardware flush primitives.
//!
//! The model describes the least that each flush invalidates. The trusted arch
//! flushes may invalidate more (`invlpg` and LoongArch's `invtlb 0` also drop
//! global entries, and RISC-V implements `TlbFlushOp::All` as a full
//! `sfence.vma`), which keeps the model a sound upper bound on what a TLB may
//! cache.
//!
//! The request bookkeeping of `OpsStack` and `TlbFlusher::issue_tlb_flush[_with]`
//! is verified against [`FlushSet`]. `OpsStack::new`, the hardware flushes, and
//! dispatching and synchronizing flush requests remain trusted. Which CPU runs
//! the code is not modelled yet.
//!
//! TODO: Page-table updates do not add entries to `stale` yet, and
//! `TlbFlusher::dispatch_tlb_flush`, `sync_tlb_flush`, and non-x86 arch flushes
//! do not update [`TlbModel`] yet, so [`TlbModel::coherent`] is not yet tied to
//! the page table or maintained by `CursorMut::map` and `CursorMut::unmap`.
//!
//! Frames are not kept alive until their TLB entries are flushed: the
//! `page_keeper` of `OpsStack` is commented out, so the frame passed to
//! `TlbFlusher::issue_tlb_flush_with` is dropped once the request is recorded.
use vstd::prelude::*;
use vstd_extra::ownership::*;

use crate::specs::mm::page_table::*;

use core::ops::Range;

use crate::{
    cpu::cpu_count,
    mm::{PAGE_SIZE, Paddr, Vaddr},
};

verus! {

/// The TLB entries that a group of flush operations invalidates, by their ISA
/// meaning.
pub ghost struct FlushSet {
    /// At least every non-global entry is invalidated, as by `TlbFlushOp::All`.
    pub all_user: bool,
    /// At least the non-global entries translating any of these addresses are
    /// invalidated, as by `invlpg` or `sfence.vma`.
    pub vas: Set<Vaddr>,
}

impl FlushSet {
    pub open spec fn empty() -> Self {
        FlushSet { all_user: false, vas: Set::empty() }
    }

    /// The meaning of flushing all non-global entries, which drops any
    /// address-specific operations collected before.
    pub open spec fn all_user() -> Self {
        FlushSet { all_user: true, vas: Set::empty() }
    }

    /// The meaning of flushing entries translating a single virtual address.
    pub open spec fn addr(va: Vaddr) -> Self {
        FlushSet { all_user: false, vas: Set::empty().insert(va) }
    }

    /// The meaning of flushing one address per base page across `range`,
    /// starting from `range.start`.
    pub open spec fn range(range: Range<Vaddr>) -> Self {
        FlushSet {
            all_user: false,
            vas: Set::from_finite_type(
                |va: Vaddr|
                    range.start <= va < range.end && (va - range.start) % PAGE_SIZE as int == 0,
            ),
        }
    }

    pub open spec fn join(self, other: Self) -> Self {
        FlushSet { all_user: self.all_user || other.all_user, vas: self.vas + other.vas }
    }

    /// Whether this flush set is guaranteed to invalidate `entry`.
    ///
    /// Global (`AddrSpaceId::Kernel`) entries are never invalidated by the
    /// model. Non-global entries are invalidated when `all_user` holds or when
    /// any flushed virtual address lies in `entry.map.va_range`.
    pub open spec fn invalidates(self, entry: TlbEntry) -> bool {
        entry.asid is User && (self.all_user || exists|va: Vaddr| #[trigger]
            self.vas.contains(va) && entry.map.va_range.start <= va < entry.map.va_range.end)
    }
}

/// The address space that a TLB entry is tagged with.
pub ghost enum AddrSpaceId {
    /// Global entries. The model never invalidates them, although some
    /// architectures do.
    Kernel,
    /// Non-global entries of the user page table with this root.
    User(Paddr),
}

pub ghost struct TlbEntry {
    pub asid: AddrSpaceId,
    pub map: Mapping,
}

/// A CPU index: the view of a [`CpuId`], which lies in `0..cpu_count()`.
///
/// [`CpuId`]: crate::cpu::CpuId
pub type CpuIdx = int;

/// The state of one CPU's TLB, as an upper bound on what it may cache.
pub ghost struct CpuTlbState {
    /// The root of the user page table installed in CR3 or `satp`, if any.
    pub active: Option<Paddr>,
    /// The address spaces that may have entries cached on this CPU.
    pub resident: Set<AddrSpaceId>,
    /// The entries that may be cached although their page tables no longer
    /// contain them.
    pub stale: Set<TlbEntry>,
}

impl Inv for CpuTlbState {
    open spec fn inv(self) -> bool {
        &&& self.resident.contains(AddrSpaceId::Kernel)
        &&& self.active matches Some(root) ==> self.resident.contains(AddrSpaceId::User(root))
        &&& forall|e: TlbEntry| #[trigger]
            self.stale.contains(e) ==> e.map.inv() && self.resident.contains(e.asid)
    }
}

impl CpuTlbState {
    /// The TLB state after flushing `flush` on this CPU.
    pub open spec fn apply_flush(self, flush: FlushSet) -> Self {
        let resident = if flush.all_user {
            match self.active {
                Some(root) => set![AddrSpaceId::Kernel, AddrSpaceId::User(root)],
                None => set![AddrSpaceId::Kernel],
            }
        } else {
            self.resident
        };
        CpuTlbState {
            active: self.active,
            resident,
            stale: self.stale.filter(|e: TlbEntry| !flush.invalidates(e)),
        }
    }

    /// The TLB state after installing `root` as the active user page table on
    /// this CPU, without invalidating existing TLB entries.
    pub open spec fn activate(self, root: Paddr) -> Self {
        CpuTlbState {
            active: Some(root),
            resident: self.resident.insert(AddrSpaceId::User(root)),
            stale: self.stale,
        }
    }

    /// Applying an empty flush leaves the CPU TLB state unchanged.
    pub proof fn lemma_apply_flush_empty(self)
        ensures
            self.apply_flush(FlushSet::empty()) == self,
    {
        assert(self.apply_flush(FlushSet::empty()).stale =~= self.stale);
    }

    /// Sequential flushes compose via [`FlushSet::join`].
    pub proof fn lemma_apply_flush_join(self, first: FlushSet, second: FlushSet)
        ensures
            self.apply_flush(first).apply_flush(second) == self.apply_flush(first.join(second)),
    {
        let lhs = self.apply_flush(first).apply_flush(second);
        let rhs = self.apply_flush(first.join(second));
        assert(lhs.resident =~= rhs.resident);
        assert(lhs.stale =~= rhs.stale) by {
            assert forall|e: TlbEntry| #[trigger]
                lhs.stale.contains(e) <==> rhs.stale.contains(e) by {
                if first.join(second).invalidates(e) {
                    if !first.all_user && !second.all_user {
                        let va = choose|va: Vaddr|
                            first.join(second).vas.contains(va) && e.map.va_range.start <= va
                                < e.map.va_range.end;
                        if first.vas.contains(va) {
                            assert(first.invalidates(e));
                        } else {
                            assert(second.vas.contains(va));
                            assert(second.invalidates(e));
                        }
                    }
                }
                if first.invalidates(e) {
                    if !first.all_user {
                        let va = choose|va: Vaddr|
                            first.vas.contains(va) && e.map.va_range.start <= va
                                < e.map.va_range.end;
                        assert(first.join(second).vas.contains(va));
                    }
                    assert(first.join(second).invalidates(e));
                }
                if second.invalidates(e) {
                    if !second.all_user {
                        let va = choose|va: Vaddr|
                            second.vas.contains(va) && e.map.va_range.start <= va
                                < e.map.va_range.end;
                        assert(first.join(second).vas.contains(va));
                    }
                    assert(first.join(second).invalidates(e));
                }
            };
        }
    }
}

/// The TLB of one CPU.
pub ghost struct CpuTlb {
    cpu: CpuIdx,
    state: CpuTlbState,
}

impl View for CpuTlb {
    type V = CpuTlbState;

    closed spec fn view(&self) -> CpuTlbState {
        self.state
    }
}

impl CpuTlb {
    /// The CPU whose TLB this is.
    pub closed spec fn cpu(self) -> CpuIdx {
        self.cpu
    }
}

/// The tracked TLBs of all CPUs, one for each `CpuId` in `0..cpu_count()`.
pub tracked struct TlbModel {
    ghost cpus: Map<CpuIdx, CpuTlb>,
}

impl Inv for TlbModel {
    closed spec fn inv(self) -> bool {
        &&& forall|c: CpuIdx| #[trigger] self.cpus.contains_key(c) <==> 0 <= c < cpu_count()
        &&& forall|c: CpuIdx|
            #![trigger self.cpus[c]]
            0 <= c < cpu_count() ==> self.cpus[c].cpu() == c && self.cpus[c]@.inv()
    }
}

impl TlbModel {
    /// The TLB state of CPU `c`, the view of a `CpuId`.
    pub closed spec fn cpu_state(self, c: CpuIdx) -> CpuTlbState {
        self.cpus[c]@
    }

    /// `post` matches `self` on every CPU except `cpu`, whose TLB state is `next`.
    pub open spec fn cpu_updated(self, post: Self, cpu: CpuIdx, next: CpuTlbState) -> bool {
        &&& post.cpu_state(cpu) == next
        &&& forall|c: CpuIdx|
            0 <= c < cpu_count() && c != cpu ==> #[trigger] post.cpu_state(c) == self.cpu_state(c)
    }

    /// No CPU caches an entry that its page table no longer backs.
    pub open spec fn coherent(self) -> bool {
        forall|c: CpuIdx, e: TlbEntry|
            0 <= c < cpu_count() ==> !#[trigger] self.cpu_state(c).stale.contains(e)
    }
}

} // verus!
