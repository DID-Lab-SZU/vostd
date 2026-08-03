# VOSTD verification progress

`verification-progress` reports how much of the VOSTD source tree is in the current x86
verification build and how that active source is split between executable code, proofs,
specifications, and trusted boundaries.

## Usage

Run the whole-crate Verus check and then write `progress.json` and `progress.md`:

```console
make progress
```

Run the analyzer directly:

```console
cargo run -p verification-progress -- --target ostd
cargo run -p verification-progress -- --target ostd --static-only
cargo run -p verification-progress -- --target ostd --baseline old-progress.json
cargo run -p verification-progress -- --target ostd --output-dir target/my-progress
```

The default output directory is `target/verification-progress`. These reports are local
artifacts and are not intended to be committed.

## Metric rules

The primary project-wide denominator combines source-declared exec functions and methods with
bodies from the active x86 Cargo dep-info input set with every RISC-V and LoongArch
architecture-specific source body. Because those two architecture builds are not confirmed,
their ordinary and syntactically verification-candidate bodies enter the project denominator as
`unverified`. The report retains a separate current-x86-build metric for day-to-day work. Trait
declarations without a default body, extern declarations, closures, and macro-generated
functions are excluded.

When the x86 whole-crate run succeeds, the project-wide status is `partially_confirmed`: its
checked numerator is confirmed by x86 Verus, while the two architecture-specific backlogs are
already present in the denominator as unverified. The x86-only package table remains `passed`.

- `checked_candidates`: exec bodies in `verus!`, `#[verus_verify]`,
  `#[verifier::verify]`, or equivalent verified regions.
- `checked`: populated from x86 `checked_candidates` only after a successful x86 whole-crate
  Verus run. It is `null` in static mode or after a failed/partial run; unconfirmed architecture
  candidates are folded into project-wide `unverified` instead.
- `trusted`: exec bodies behind `external_body` or another trusted boundary.
- `unverified`: ordinary Rust bodies and bodies excluded with `external`.
- coverage: `checked / (checked_candidates + trusted + unverified)`. It is absent unless the
  whole-crate run succeeds.
- contract coverage: exec bodies with a `requires`, `ensures` (including default ensures),
  `returns`, `recommends`, `verus_spec`, or `dual_spec` contract.

Proof and spec counts describe proof scale; they are never added to exec coverage. Proof
declarations without bodies are shown separately from checked proof bodies. Trust debt separately
counts active `external_body`, `external`, external specification, trusted, axiom, `assume`, and
`admit` markers. Unsafe exec functions are split across the same exec buckets.

Physical source lines use the mutually exclusive display priority
`Trusted > Spec > Proof > Exec`; JSON also retains every raw line-tag combination for audit.

## Source scope

The confirmed build configuration is `x86_64-unknown-none`. Active source files come from the
newest unambiguous Cargo dep-info for every verified package in `ostd`'s local dependency
closure. The report lists `ostd`, `vstd_extra`, `ostd-pod`, `bitflags`, and `align_ext`
separately. The full public+x86 inventory is used for source-inclusion statistics.

RISC-V (`riscv64imac-unknown-none-elf`) and LoongArch
(`loongarch64-unknown-none-softfloat`) architecture-specific source trees are also parsed with
their own `cfg` sets. Their exec/proof/spec counts, contract coverage, trust debt, and line
composition are included in both outputs. Because `make progress` currently confirms only the
x86 whole-crate build, the other architectures are explicitly marked `unconfirmed`; their exec
candidate counts are never presented as checked coverage.

If the analyzer cannot identify a unique usable dep-info input set, it writes an error-bearing
report and exits nonzero instead of guessing. A failed Verus run likewise still produces both
output files, but no verified coverage percentage.

`progress.json` currently uses `schema_version: 1`. Baseline comparison is informational and
does not impose a pass/fail threshold.
