---
id: "0329"
product: monorepo
title: "vxn-core-matrix: crate skeleton, the roster/mechanism seam, and the null-test harness"
priority: medium
created: 2026-08-30
epic: E049
depends: []
---

## Summary

First ticket of [E049](../../epics/closed/E049-shared-matrix-routing.md). Two
deliverables, both prerequisites for everything after:

1. `crates/vxn-core-matrix` and the seam [ADR 0003](../../adrs/0003-vxn-core-matrix.md)
   sets, with **no consumers yet**.
2. The **null-test harness** every later ticket is verified against. The epic's
   bar is a difference peak ≤ −100 dBFS, and the repo has no way to measure that
   today — only a render hash, which is binary. Without this, ticket one of the
   extraction has no way to show it changed nothing.

The seam is between the **roster** (what a synth can route) and the
**mechanism** (how a routing is evaluated). This ticket writes down the roster
side as a trait, so later tickets have something to be generic over.

## Design

```rust
pub trait MatrixRoster: Copy {
    const N_SOURCES: usize;
    const N_DESTS: usize;
    const N_SLOTS: usize;

    fn source_is_bipolar(src: u8) -> bool;
    fn dest_gain(dest: u8) -> f32;
    fn cook_depth(dest: u8, depth: f32) -> f32;
    fn dest_tier(dest: u8) -> Tier;
    fn source_tier(src: u8) -> Tier;
    fn dest_smoothing(dest: u8) -> Smoothing;

    fn source_names() -> &'static [&'static str];
    fn dest_names() -> &'static [&'static str];
    // …labels likewise
}
```

Opaque `u8` indices rather than associated enum types: the engine never needs to
know what `dest 7` *is*, and associated types would force every shared function
to carry two more generic parameters. The synth's enums stay its own and convert
at the boundary, exactly as they already do at the wire boundary.

`Tier` and `Smoothing` are shared enums defined here — see
[0332](0332-roster-row-declares-everything.md) for what fills them in and
[0335](0335-declared-target-smoothing.md) for what consumes `Smoothing`.

### Sizing: plain const generics, no unstable features, no wasted space

Each synth gets storage sized exactly to its own roster — vxn-2's 51
destinations and vxn-1b's 16 — with the sizes known at compile time. The trick
is that the size is **its own const generic parameter, inferred from the
argument**, rather than derived from an associated const:

```rust
pub fn eval<R: Roster, const NS: usize, const ND: usize, const L: usize>(
    src: &[[f32; L]; NS],
    out: &mut [[f32; L]; ND],
) {
    const { assert!(NS == R::N_SOURCES && ND == R::N_DESTS) };
    ...
}

eval::<Vxn2Roster,  _, _, _>(&src, &mut out);  // [[f32; 8]; 51]
eval::<Vxn1bRoster, _, _, _>(&src, &mut out);  // [[f32; 8]; 16]
```

Verified to compile on the pinned stable toolchain (edition 2024). Writing
`[f32; R::N_DESTS]` — deriving the length from the associated const — is what
needs `generic_const_exprs`; taking the length as a parameter does not. The two
read almost the same and behave completely differently.

The `const {}` block is not decoration: it is checked, and it fires. Handing
`eval::<Vxn1bRoster, _, _, _>` a 51-wide accumulator fails to compile with
`evaluation of eval::<Vxn1b, 12, 51, 8>::{constant#4} failed`. So the roster and
the storage cannot silently disagree — which is the failure this sizing scheme
would otherwise invite, since `ND` is inferred from whatever array the caller
happens to pass.

That removes the concern an earlier draft of this ticket raised about a shared
`MAX_DESTS` buffer taxing vxn-1b with 3× the accumulator clears per block. There
is no shared buffer and no runtime bound: each instantiation is monomorphised to
its own exact size, exactly as the per-crate constants do today.

Two things still worth measuring rather than assuming:

- **Monomorphisation cost.** Two rosters × the lane counts each uses means
  several copies of the evaluator. That is what you want for speed and what you
  don't want for compile time and instruction cache. Check the binary size delta.
- **Whether `L` wants to be generic at all.** vxn-1b already has
  `eval_dests_bank<const L: usize>`; vxn-2 has a single `STACK_LANES`. Both are
  in fact 8 today (`RenderBank::LANES = CHANNELS_PER_LAYER = 8`,
  `STACK_LANES = 8`) — if only the tests exercise a second value of `L`, the
  genericity is paying for test convenience.

## The null-test harness

Site the comparator in **`vxn_core_dsp::test_util`**, not in the new crate. That
module already holds this exact genre — `assert_bit_exact_passthrough`,
`worst_d4`, `sine_rms` — and a render-difference helper has nothing to do with
matrix routing. Putting it in `vxn-core-matrix` would be filing it by *when it
was written* rather than *what it is*.

```rust
/// Peak difference between two renders, in dBFS. `-inf` for identical buffers.
pub fn null_test_peak_dbfs(a: &[f32], b: &[f32]) -> f64;

/// Assert two renders differ by no more than `limit_dbfs`, reporting the
/// measured peak and the sample index where it occurred on failure.
pub fn assert_null_test(a: &[f32], b: &[f32], limit_dbfs: f64);
```

The *comparator* is shared; the *reference render* stays per-synth beside each
existing baseline test, because it needs that synth's engine and patch. The
failure message must carry the measured peak and its sample index — "exceeded
−100 dBFS" alone tells you nothing about whether you are at −99 or −12, and that
difference is the whole judgement.

Capturing the reference: the pragmatic route is to render to a file checked in
beside the baseline test, in the same spirit as the golden hash. Keep it small —
the reference patch is already deterministic and a second or two of stereo at
48 kHz is enough to catch drift. Keep it *short* for a second reason: E049
§"The bar" — ULP-level pitch perturbations integrate into phase drift, and a
long render lets an inaudible reorder walk past −100 dBFS.

Two more gaps this ticket closes, because every later ticket's criteria assume
they exist:

- **vxn-1b has no render-hash baseline today** — only vxn-2 does
  (`vxn2-engine/tests/baseline.rs`, gated on `VXN_RENDER_HASH=1`, CI-only).
  Add the vxn-1b equivalent with the same gating; until it exists, every
  "both baselines" criterion in this epic names a thing with one instance.
- **vxn-1b has no matrix bench** — `route_profile.rs` is a profiling example,
  not a criterion bench. 0334 and the 0337 close-out table need before/after
  numbers for vxn-1b; add a bench mirroring vxn2-osc-bench's
  `matrix_eval_full` / `matrix_eval_scaled`.

## Acceptance criteria

- [x] `crates/vxn-core-matrix` exists, in the workspace, `cargo check` clean.
- [x] `MatrixRoster`, `Tier`, `Smoothing` defined and documented.
- [x] A synthetic test roster (`TestRoster`: ~4 sources, ~4 dests, all gains
      1.0, no taper) lives behind `#[cfg(test)]` or a `testing` feature —
      [0331](0331-matrix-golden-vector-harness.md) builds on it.
- [x] `null_test_peak_dbfs` / `assert_null_test` in `vxn_core_dsp::test_util`,
      with a reference render captured for **both** synths and a test that
      compares against it.
- [x] The harness is proved to work by making it fail: perturb one sample of a
      reference by a known amount and check the reported peak matches. A null
      test that silently passes on everything is worse than no null test, and
      that is the failure mode nobody notices.
- [x] vxn-1b render-hash baseline test exists, gated like vxn-2's
      (`VXN_RENDER_HASH=1`, CI-only), hash captured.
- [x] vxn-1b criterion matrix bench exists and produces stable numbers to quote
      in later close-outs.
- [x] The routing crate itself has **no consumers** — no vxn-1b or vxn-2 file
      changes to use `vxn-core-matrix`. (The harness necessarily touches both
      synths' test dirs; that is the one deliberate exception, and it is why
      this criterion is scoped to the crate rather than to the ticket.)
- [x] The buffer-sizing decision above is recorded in the crate's module docs
      with whatever measurement backed it.

## Notes

- Follow `vxn-core-dsp`'s crate conventions ([ADR 0002](../../adrs/0002-vxn-core-dsp.md)):
  workspace-inherited version/edition/license, no synth deps.
- The routing half is deliberately inert. The value of a skeleton ticket is that
  the seam gets reviewed before anything is ported through it — if the trait is
  wrong, this is the cheap moment to find out.
- The harness half is not inert, and is the more urgent of the two: every
  subsequent ticket's acceptance criteria reference a measurement that cannot be
  taken until it exists — including [0328](0328-matrix-dest-major-lane-accumulators.md),
  which now depends on this ticket for exactly that reason. If this ticket gets
  split, split it that way round — harness first.

## Close-out (2026-09-01)

**Implemented in `a2e025d` + `3d14b0a` on 2026-08-30; the ticket was left open.**
Closed here as part of [0337](0337-retire-duplicated-matrix-code.md)'s epic
close-out, since E049 cannot close with a member ticket open. This is a
verification pass on today's `main`, not a record of fresh work.

- `crates/vxn-core-matrix` is in the workspace and clean. `MatrixRoster`, `Tier`
  and `Smoothing` are defined and documented in
  [roster.rs](../../crates/vxn-core-matrix/src/roster.rs); `TestRoster` sits
  behind `#[cfg(any(test, feature = "testing"))]`, which is how both synths' test
  bridges reach it.
- `null_test_peak_dbfs` / `assert_null_test` are in `vxn_core_dsp::test_util`,
  with reference renders for **both** synths and a `baseline.rs` comparing
  against each.
- **The harness is proved by making it fail**, on all three levels the ticket
  asked for: `a_known_perturbation_reads_back_at_its_known_level` in
  `test_util`, and `a_perturbed_reference_is_caught_at_the_right_level_and_sample`
  in each synth's `baseline.rs` — the latter also checks the reported *sample
  index*, so a harness that found a difference but pointed at the wrong place
  would fail too.
- vxn-1b's render-hash baseline exists and is gated like vxn-2's
  (`VXN_RENDER_HASH=1`, CI-only, dev machines skip); its criterion matrix bench
  produces the four cases quoted throughout this epic's close-outs.
- The buffer-sizing decision and its measurement (~2.4 kB per roster
  instantiation, ~1.7 kB per extra lane count, and the observation that the two
  rosters never link into the same binary) are in
  [storage.rs](../../crates/vxn-core-matrix/src/storage.rs)'s module docs.

**The "no consumers" criterion was satisfied when this landed and is
deliberately no longer true** — 0330 onward is the whole point. The seam got its
review before anything was ported through it, which is what the criterion bought.

**One thing this ticket did better than it knew.** Capturing both reference
renders here, before any arithmetic moved, is what made 0337's end-to-end null
test possible: neither file was ever re-captured, so every null test across nine
tickets has compared against the pre-epic render rather than the previous
ticket's. The epic's §"The bar" workflow — per-ticket passes do not compose into
an end-to-end one — was never actually needed, because the references never
drifted. Worth copying the next time an epic sets a null-test bar.
