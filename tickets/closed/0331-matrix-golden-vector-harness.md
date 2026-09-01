---
id: "0331"
product: monorepo
title: "Golden-vector matrix tests: routes + source values in, modulation amounts out"
priority: medium
created: 2026-08-30
epic: E049
depends: ["0329"]
---

## Summary

Build the declarative test surface [ADR 0003](../../adrs/0003-vxn-core-matrix.md)
§4 calls for, in `vxn-core-matrix`, against the synthetic roster from
[0329](0329-vxn-core-matrix-crate-skeleton.md).

A case is: *put these routes in at these depths, feed these source values, get
these modulation amounts.* Nothing else.

## Design

Today's mechanism tests bake roster constants into their expected numbers.
[vxn-1b eval.rs](../../vxn-1b/crates/vxn1b-engine/src/eval.rs) asserts
`out[Cutoff] == 24.0`, which is three claims at once — the evaluator multiplies
correctly, `DEST_GAIN[Cutoff]` is 48, and `Cutoff` takes no depth taper. Change
the gain and a test of the *evaluator* fails.

Against the synthetic roster (all gains 1.0, no taper) a number in an assertion
is the evaluator's arithmetic and nothing else.

```rust
Case {
    routes: &[
        // (source, dest, depth, polarity, shape, scale_src, scale_shape, enabled)
        (SRC_A, DST_X, 1.0, Abs, Lin, NONE, Lin, true),
        (SRC_B, DST_X, 0.5, Direct, Exp, SRC_C, Exp, true),
        (SRC_A, DST_Y, 1.0, Direct, Lin, NONE, Lin, false), // off: contributes nothing
    ],
    sources: &[(SRC_A, -1.0), (SRC_B, 0.5), (SRC_C, 0.5)],
    expect:  &[(DST_X, 1.0 + 0.25 * 0.25), (DST_Y, 0.0)],
}
```

Every case runs through **every** evaluator path the crate offers (scalar and
banked, once [0334](0334-share-the-evaluator.md) lands) and the results must
agree **bit-exactly**. Float addition is not associative, and "same routes in
the same order" is already vxn-1b's stated contract between its two paths — this
generalises that guarantee instead of re-deriving it per synth.

Cases are data, so coverage is a table rather than a function each:

- all nine polarity × shape pairs;
- both scale-source polarities × all three scale bends;
- the on/off switch, including that a disabled slot is dropped identically by
  every path;
- several slots summing into one dest (the accumulate-order case);
- inert slots interleaved with live ones (compaction);
- zero-depth and `None`-endpoint short circuits;
- out-of-range codes degrading rather than aliasing.

Keep the existing randomised scalar-vs-bank parity sweep alongside this — the
table covers the cases someone thought of, the sweep covers the ones they
didn't.

## Acceptance criteria

- [ ] `Case` and its runner exist in `vxn-core-matrix`, generic over
      `MatrixRoster`, exercised against the synthetic roster.
- [ ] The coverage list above is present as cases; each is one table row.
- [ ] The runner asserts every available evaluator path agrees bit-exactly, and
      **fails loudly if only one path exists** rather than silently testing half
      of what it claims.
- [ ] Per-synth tests that were really mechanism tests are deleted, not
      duplicated. Per-synth tests that assert roster facts (this gain is 48,
      these dests take the taper) stay and are labelled as roster tests.
- [ ] Adding a case requires no new test function.

## Notes

- This ticket is why the epic is worth doing even if the evaluator is never
  shared: it makes "what does this matrix compute" answerable in one place.
- Prefer a Rust const table to a TOML fixture. The values are floats compared
  bit-exactly, and a text format adds a parse-and-round step between the
  intention and the assertion — the wrong kind of indirection for a golden test.
- Out of scope: audio-level verification. These are arithmetic vectors. Whether
  the result *sounds* right is a Reaper check ([[verify-audio-in-reaper]]).

## Close-out (2026-09-01)

- `Case` and `run_case` live in
  [golden.rs:218](../../crates/vxn-core-matrix/src/golden.rs#L218) and
  [golden.rs:436](../../crates/vxn-core-matrix/src/golden.rs#L436), generic over
  `MatrixRoster` and over the lane count, exercised against `TestRoster`. The
  table holds **36 cases**; every source value is a dyadic rational chosen so
  each shaping arm lands on another, which is what lets the expectations be
  compared bit-for-bit rather than with a tolerance.
- The coverage list is table rows, pinned by tests rather than by reading:
  `polarity_shape_pairs_are_all_covered` (all nine pairs),
  `scale_polarities_and_bends_are_all_covered` (both polarities × three bends),
  and `the_short_circuit_and_degradation_cases_are_all_present` (switched-off,
  unscaled, unwired source, unwired dest, zero depth, out-of-range curve code,
  out-of-range scale bend, empty table, ≥3 routes into one dest, inert
  interleaved with live).
- The runner fails loudly rather than silently testing half of what it claims:
  `MIN_EVAL_PATHS = 2` at [golden.rs:240](../../crates/vxn-core-matrix/src/golden.rs#L240),
  asserted by `the_runner_has_more_than_one_path_to_compare`. Paths are compared
  bit-exactly.
- Per-synth mechanism tests are deleted, not duplicated: 12 from vxn-1b's
  `eval.rs`, 22 from vxn-2's `matrix.rs` (grep for the named tests returns
  nothing). What stays is labelled — see the banner at
  [eval.rs:402](../../vxn-1b/crates/vxn1b-engine/src/eval.rs#L402) and
  [matrix.rs:1023](../../vxn-2/crates/vxn2-engine/src/matrix.rs#L1023) — plus
  vxn-1b's randomised scalar-vs-bank parity sweep, kept as the ticket asks.
- Each synth also bridges its own endpoints onto the synthetic roster and runs
  the same cases through its real evaluator (`the_shared_golden_vectors_hold_for_vxn1b`,
  `the_shared_golden_vectors_hold_for_vxn2`). Added after review showed two
  harness-owned paths alone would leave vxn-2's curve arms uncovered;
  mutation-checked by wiring vxn-2's `Abs` arm to `pol_direct`, which the bridge
  then fails.
- Adding a case requires no new test function — one row in `CASES`.
- Null test `-inf dBFS` on both engines; tests-only change, no arithmetic moved.
