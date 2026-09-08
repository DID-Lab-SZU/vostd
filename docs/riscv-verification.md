# RISC-V paging verification

The Sv48 proofs are adapted from
[`prove/arch-riscv` at `a3065369c`](https://github.com/DID-Lab-SZU/vostd/commit/a3065369c).
They verify the RISC-V PTE implementation and integrate it with the current
architecture-independent page-table, cursor, frame, and VM-space proofs.

## Running verification

```sh
make                       # Default x86 configuration
make verify-riscv           # RISC-V configuration, including dependency proofs
cargo dv focus --targets ostd --target riscv64imac-unknown-none-elf \
  --features riscv_paging_verification -- --verify-only-module arch::mm
cargo dv build --targets ostd --target riscv64imac-unknown-none-elf \
  --features riscv_paging_verification
```

`rust-toolchain.toml` includes the RISC-V standard-library target. The DV
submodule changes add `--target` to verify, focus, and build; x86 remains the
default. The `riscv_paging_verification` feature omits the unverified RISC-V boot,
CPU, trap, and device modules and exposes the interrupt and timer interfaces
needed by the existing verification subset. It is not a bootable-kernel target.

## Verified properties and limits

- Sv48 has four levels, 4 KiB base pages, 512 entries per node, 48-bit canonical
  virtual addresses, and a 56-bit PTE physical-address encoding. The tracked
  frame model retains its existing, smaller `MAX_PADDR` bound.
- PTE constructors and property updates satisfy the common PTE contracts.
  Encoding proofs cover permissions, software tags, cache bits, reserved bits,
  address extraction, and physical-address preservation.
- The additional `pte_wf(level)` lemmas establish the Sv48 encoding and alignment
  rules for absent, child-table, and aligned leaf entries. They are local lemmas;
  the generic page-table invariant does not yet include this predicate.
- The non-leaf guarantee for `new_pt` applies above level 1. RISC-V treats level 1
  as terminal regardless of the RWX bits.
- Verified property updates require readable mappings and either Writeback or
  Uncacheable caching. These are restrictions inherited from the source proofs;
  execute-only mappings are not covered.
- This migration preserves the current setter's clearing of ACCESSED/DIRTY.
  Its verified round trip additionally requires both input flags to be clear.
  The source branch's change to write these flags is not included. The original
  unsupported-cache panic and message are retained; that path is excluded by
  the verified precondition.

## Executable adaptations

The RISC-V paging constants implement the current method-based
`PagingConstsTrait` API. Flags use the repository's verified bitflags macro,
which exposes flag constructors as methods. `Pod` and `Default` use explicit
implementations so their layout and zero-value properties can be checked.
`new_absent` and `as_usize` implement the current shared trait.

The private `new_paddr` helper sets VALID, allowing the checked `new_page` to
call `set_prop`; `new_pt` returns that helper's result. Both public constructors
produce the same encoded values as before. Property encoding uses an immutable
base word and a cache-selection expression, preserving the existing output
bits and panic path. Hardware operations retain their original implementations.

## Trusted hardware boundary

`ostd/specs/arch/riscv/hardware.rs` supplies an external contract for
`arch::mm::current_page_table_paddr`, needed by the existing VM reader/writer
specifications. Its source basis is `riscv` 0.11.1's `register/satp.rs`:
`read` reads CSR 0x180, `ppn` extracts bits 0 through 43, and OSTD shifts that
number left by 12. The contract connects the getter to an uninterpreted active
root, following the existing x86 model. It assumes the active root remains
stable during a VM reader/writer operation.

This is an explicit trusted contract, not a proof of hardware behavior.
Supervisor privilege, Svpbmt extension availability, CSR side effects, page-table
activation, TLB invalidation,
interrupts, and concurrency of address-space switching remain outside these
proofs. No new `external_body`, `assume()`, or `admit()` is added.

## Migration validation

Local validation on macOS used the configured Rust 1.98.0 / Verus toolchain:

- `make`: 1546 OSTD verification items passed for x86.
- `make verify-riscv`: 1535 OSTD verification items passed, with dependency
  proofs also verified.
- The RISC-V release library compiled in build mode with Verus `--no-verify`,
  following the separate complete verification run.
- All 13 DV library and binary unit tests passed. Its legacy integration suite expects
  an absent fixture workspace; it was not a passing validation gate.
- Cleanup retained 19 individually or jointly verified assertion removals.
  Further cleanup was stopped after ten minutes, restoring the active candidate.
- All new Rust files passed `verusfmt --check`; the pre-existing block-style
  postconditions in `ostd/src/arch/x86/mm/mod.rs` cannot be parsed by verusfmt
  0.7.2, including on the unchanged baseline.
- Static review and `git diff --check` passed. Dynamic GitHub review rules could
  not be refreshed because the API rate limit was exceeded.

The Linux and macOS workflows now include `make verify-riscv`; those remote jobs
were not run as part of this local migration. DV is a Git submodule, so its
changes must be committed separately before updating the parent gitlink.
