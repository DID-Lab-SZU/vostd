# LoongArch paging verification

The LoongArch port connects the existing PTE implementation to the shared
page-table, cursor, frame, and VM-space proofs, using the RISC-V migration's
architecture model and cross-target verification entry points.

## Running verification

```sh
make verify-loongarch
cargo dv focus --targets ostd --target loongarch64-unknown-none-softfloat \
  --features loongarch_paging_verification -- --verify-only-module arch::mm
cargo dv build --targets ostd --target loongarch64-unknown-none-softfloat \
  --features loongarch_paging_verification -- --no-verify
make
make verify-riscv
```

Run complete verification before the separate `--no-verify` compilation command.
The configured Rust toolchain includes `loongarch64-unknown-none-softfloat`.
DV's existing `--target` support is required. The
`loongarch_paging_verification` feature selects the paging verification subset,
omitting unverified boot, CPU, trap, and device modules. Its interrupt helpers
retain the original CSR operations. This configuration is not a bootable kernel.

## Verified properties and limits

- The current implementation uses four page-table levels, 4 KiB pages, 512
  entries per node, 48-bit canonical virtual addresses, and a 48-bit PTE
  physical-address mask. The shared tracked-frame model retains `MAX_PADDR`.
- Only level-1 leaves are supported, matching the existing
  `HIGHEST_TRANSLATION_LEVEL = 1`. Huge-page translation is not established.
- Absent entries and child-table constructors satisfy the shared contracts.
  Address extraction is page-aligned for arbitrary encoded words. Child-table
  entries are nonterminal above level 1.
- Page constructors preserve addresses and round-trip permissions, software
  flags, USER/GLOBAL, and Writeback/WriteCombining policies when DIRTY is set in
  the input. The implementation always sets DIRTY, so inputs without it cannot
  satisfy the shared exact-property round trip.
- Property updates preserve addresses, presence, terminal status, and exact
  properties under a narrower domain: the old entry is present and not huge,
  address bit 12 is clear, the new GLOBAL flag is clear, DIRTY is set, and the
  cache policy is Writeback or WriteCombining. These restrictions are explicit
  in `set_prop_req`. `lemma_supported_mapping_witness` proves that a real user
  mapping at `0x2000` satisfies the constructor and update contracts, including
  another update after the first.
- The private setter's raw-encoding contract covers every valid flag
  combination with Writeback, WriteCombining, or Uncacheable, including cases
  outside the shared round-trip domain. Unsupported cache policies retain the
  existing panic path and are excluded by the verified precondition.

The common page-table proofs remain conditional on their existing
architecture-specific constructor/update preconditions. Passing verification
does not establish that every desired mapping meets these LoongArch restrictions.
The untracked kernel mapping API now requires constructor compatibility only
at supported leaf levels. Requiring it at every `u8` level would make that API's
precondition impossible on LoongArch. The witness also proves this corrected
architecture-specific requirement for its example property.

The shared physical-to-virtual lemma now proves that the linear mapping lies
above the target's kernel lower bound, instead of relying on those constants
being equal. The kernel-range lemma is restricted to vmalloc addresses: the
LoongArch kernel lower bound `0x9000_0000_0000_0000` includes direct-map windows
outside the page table's sign-extended high half. Runtime address constants
are unchanged.

## Existing behavior exposed by the proofs

`lemma_legacy_encoding_limits` contains checked counterexamples:

1. `set_prop` clears IS_BASIC. Updating a global basic mapping at `0x1000`
   makes GLOBAL decode as HUGE, changing extracted physical address to zero.
2. Updating a nonglobal basic mapping at `0x1000` also clears IS_BASIC; address
   bit 12 then decodes as GLOBAL.
3. Uncacheable writes MAT=0, but `prop()` reads MAT=0 as Writeback.
4. An input containing R without DIRTY reads back as R | DIRTY, due to the
   existing PageModifyFault workaround.

This migration models those behaviors as they stand. The restrictions above
are consequences of the implementation, not missing solver hints.

## Executable adaptations

Paging constants implement the current method-based traits. Flags use the
verified bitflags macro, and `Pod`, zero-valued `Default`, `new_absent`, and
`as_usize` have explicit implementations.

The original property setter is factored into `set_prop_inner`, with an exact
raw-encoding contract. Both `new_page` and the trait setter call it. This keeps
the constructor's initially absent word separate from the shared setter's
absent-entry contract, preserving the encoded result and panic behavior.
Hardware operation bodies are retained. The obsolete admitted
`set_prop_properties` placeholder is removed; no new `external_body`, `assume()`,
or `admit()` is introduced.

## Trusted hardware boundary

`ostd/specs/arch/loongarch/hardware.rs` supplies an explicit external contract
for `current_page_table_paddr`, following the existing VM reader/writer model.
The audited dependency is `loongArch64` 0.2.6: `register/mmu/pgdl.rs`, `pgdh.rs`,
and `register/macros.rs` read CSRs 0x19 and 0x1a with `csrrd`, and `raw()` returns
the stored word. OSTD asserts PGDL equals PGDH before returning PGDL.

The contract connects the returned root to uninterpreted register values and
requires PGDL to equal PGDH before the read. `VmSpace::reader` and `writer`
propagate this caller obligation through `current_page_table_read_req`, which
is trivially true on x86 and RISC-V. Mismatched roots retain their runtime panic
and are outside the verified calling domain; no unconditional register-equality
assumption is added.
Register values are assumed stable during a VM reader/writer operation, as in
the existing x86/RISC-V model. Privilege, CSR access, page-table activation,
TLB invalidation, interrupts, and concurrent address-space switching remain
outside the proofs.

The retained allocation axiom is narrowed to successful page-aligned requests
and named `axiom_kvirt_alloc_range_bounds`. It records size, alignment, and
vmalloc-region bounds, following the intended
`RangeAllocator::new(VMALLOC_VADDR_RANGE)` configuration. The allocator itself
is still an unverified placeholder; its behavior is a trusted model.

Offset arithmetic is now proved by `lemma_kvirt_alloc_range_bounds`, requiring
`map_offset <= area_size`, rather than granted for arbitrary offsets by an
axiom. The tracked mapping API explicitly requires its documented bounds,
matching the untracked API; a `may_panic` allowance cannot substitute for those
facts. The existing `assume(range.end > 0)` was removed, since positivity now
follows from the allocation bounds.

The Linux and macOS CI workflows include `make verify-loongarch`.

## Local validation

Validation on macOS used the configured Rust 1.98.0 / Verus toolchain:

- `make verify-loongarch`: 1553 OSTD verification items passed. Dependency
  proofs also passed during the complete verification runs.
- `make`: 1565 OSTD verification items passed for x86.
- `make verify-riscv`: 1554 OSTD verification items passed.
- The LoongArch release library compiled with `--no-verify` after complete
  verification, using the command above.
- All modified Rust/Verus files passed `verusfmt --check`.
- Bounded cleanup removed an unused spec helper. The text-mode assertion
  simplifier found no eligible standalone assertions in the witness proof;
  its explicit computation and bit-vector proofs were retained.
- Static review reported no errors, and `git diff --check` passed. The comment
  warnings were reviewed; new files remain untracked for the user's review.
  The axiom warning reflects the renamed and narrowed allocation contract;
  it replaces the previous axiom rather than adding a second trusted fact.
  Dynamic GitHub review rules were unavailable after the earlier rate-limit
  failure, so the final checker used static rules.

Remote CI jobs and boot/runtime hardware tests were not run. The pre-existing
staged DV submodule changes were preserved; no commits were created.
