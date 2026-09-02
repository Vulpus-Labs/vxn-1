---
id: "0341"
product: monorepo
title: "Scale VCA gains the polarity axis: the route's nine curves apply to the scale source too"
priority: medium
created: 2026-08-31
epic: null
depends: []
---

## Summary

A route's own shaping is two axes — [`Polarity`](../../crates/vxn-core-matrix/src/curve.rs)
then [`Shape`](../../crates/vxn-core-matrix/src/curve.rs), nine combinations.
The scale VCA has only the bend: [`scale_norm`](../../crates/vxn-core-matrix/src/curve.rs)
folds the scale source into `[0, 1]` by that source's *own* polarity, clamps,
and bends. Three options, not nine.

That asymmetry costs real behaviour. `voice-position` scaling a route can only
mean "the voices on one side of the spread", never "the voices at both edges" —
the `Abs` rectification that exists for the primary source and was added
precisely for the voice-position case. Give the scale VCA the same nine.

Both slots carry the field today as a bare `Shape`
([vxn-1b matrix.rs:291-315](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs#L291-L315),
[vxn-2 matrix.rs:660-678](../../vxn-2/crates/vxn2-engine/src/matrix.rs#L660-L678));
vxn-2's doc comment on `scale_shape` argues there is no polarity axis, and that
argument is what this ticket overturns.

DSP-only. The UI that exposes it is [0340](0340-matrix-curve-glyph-picker.md),
which depends on this landing first.

## Design

**Semantics: an explicit polarity replaces the fold.** The VCA must land in
`[0, 1]` whatever happens, and today's fold is what gets it there. So:

- `None` — today's behaviour exactly: fold by the scale source's own polarity
  (`fold_bipolar` for a bipolar source, passthrough for a unipolar one), then
  clamp. **Bit-identical to the current path**, because it *is* the current
  path.
- `Abs` — `|v|`, then clamp. No fold: rectification already lands in `[0, 1]`.
  The gate opens at both extremes of a bipolar source and shuts at centre.
  Identity for a unipolar source, exactly as on the primary axis.
- `Bipolar` — `2v − 1`, then clamp. The gate stays shut over the source's lower
  half and opens across the upper — a threshold-ish gate. On an
  already-bipolar source this clamps hard (only `v ≥ 0.5` opens it at all);
  that is a legitimate, if blunt, setting, not a bug to design around.

Clamp **after** the polarity and **before** the bend, which is the order
`scale_norm` already documents — so the bend still only ever sees `[0, 1]`,
still fixes 0 and 1, and `Lin` stays exact identity.

The tempting alternative — apply polarity, then fold whatever comes out —
degenerates: a unipolar source under `Bipolar` would be `2v − 1` folded back by
`(v + 1)·0.5`, an exact round trip to `v`, making the setting a no-op. Don't.

**vxn-2's state blob does not change size or layout.** `pack_matrix_meta`
already gives the scale bend a whole nibble at bits 12..15
([shared.rs:734-758](../../vxn-2/crates/vxn2-engine/src/shared.rs#L734-L758)),
and `curve_code(None, shape) == shape as u8`, so writing the flat nine-value
code into that nibble leaves every existing blob decoding to its current
meaning. The field becomes `scale_curve`; values 3..=8 are simply ones no old
blob ever held.

**vxn-1b's blob grows a byte and bumps its version.** The topology record is a
fixed 7 bytes `[enabled, source, dest, polarity, shape, scale, scale_shape]`
([state.rs:13-33](../../vxn-1b/crates/vxn1b-engine/src/state.rs#L13-L33)); it
becomes 8 with `scale_polarity`. Pre-release, older blobs are rejected on read
rather than migrated (ADR 0002), so this is a clean version bump.

**Presets stay loadable in both synths.** vxn-1b's TOML spells the field as a
shape name (`scale_shape`, [preset.rs:398-416](../../vxn-1b/crates/vxn1b-engine/src/preset.rs#L398-L416));
add a sibling `scale_polarity` defaulting to `none` when absent. vxn-2's flat
`CURVE_NAMES` already covers all nine and its first three entries are
`lin`/`exp`/`log`, so a file naming a bare shape parses unchanged.

**Keep the hoist.** vxn-2 dispatches `(scale source bipolar, scale_shape)` once
per slot and expands a straight-line lane loop per arm
([matrix.rs:858](../../vxn-2/crates/vxn2-engine/src/matrix.rs#L858)) — that
hoist is worth ~47% of a fully-scaled 16-slot eval and the arm count goes from
6 to 12 (`Abs`/`Bipolar` ignore the source's own polarity, so it is not 18).
If the expansion gets unwieldy, it is a macro over the arms, not a runtime
match moved back into the loop — see [[vxn1-soa-match-defeats-simd]].

## Acceptance criteria

- [ ] `scale_norm` (or its successor) takes a `Polarity` alongside the `Shape`,
      with the three mappings above, in `vxn-core-matrix` — written once, used
      by both synths.
- [ ] Both `MatrixSlot`s carry the scale polarity; both engines apply it; the
      scale VCA output is still clamped to `[0, 1]` for every
      (source, polarity, shape) combination, including a NaN source (gate shut,
      per `clamp_unit`).
- [ ] **A patch that used the old three options renders bit-identically.**
      `None` is the current arithmetic unchanged — this is a hash check, not a
      null test, and a moved hash here is a bug.
- [ ] vxn-2: an existing state blob loads with every route's scaling unchanged,
      and the packed word is still one `u32` with the same field offsets.
- [ ] vxn-1b: the topology record is 8 bytes, the blob version is bumped, and
      a preset TOML without `scale_polarity` loads as `none`.
- [ ] Golden vectors cover the new combinations: the `route(...)` helper in
      [golden.rs](../../crates/vxn-core-matrix/src/golden.rs) takes a scale
      curve code, with cases for `Abs` and `Bipolar` scaling on both a unipolar
      and a bipolar scale source.
- [ ] `matrix_eval_scaled` in `vxn2-osc-bench` is within noise of its
      pre-change figure — the per-slot hoist survives.
- [ ] The wire carries a scale-polarity edit in both synths (the vocab is
      already shipped as `polarities`), so 0340 has something to drive.

## Notes

- Ordering with [0340](0340-matrix-curve-glyph-picker.md): this lands first;
  0340 is then pure UI. Until it does, the panels can keep their existing
  scale-bend pick-list — nine options with no glyph picker is still correct,
  just verbose.
- Independent of [E049](../../epics/closed/E049-shared-matrix-routing.md) but
  touches the same file. If E049's open tickets are in flight, expect to
  rebase against whichever of 0333/0334 lands first; the arithmetic here is
  additive and the conflict is mechanical.
- Out of scope: the primary axis (unchanged), depth automation, and any change
  to which sources may be scale sources — a scale source stays a leaf value,
  so no cycle risk is introduced.
- Doc debt this creates: vxn-2's `scale_shape` doc comment and `scale_norm`'s
  module note both state that no polarity axis exists here and explain why.
  Both must be rewritten, not deleted — the *reason* (the VCA has to land in
  `[0, 1]`) is still true and is what forces the clamp-after-polarity order.

## Close-out (2026-09-02)

- `scale_norm(bipolar, v, polarity, shape)` in
  [curve.rs:789](../../crates/vxn-core-matrix/src/curve.rs#L789) — polarity,
  clamp, bend, in that order. `Direct` is the pre-0341 fold verbatim. New
  `ScaleFold` collapses the slot's scale polarity and the source's own polarity
  into **four** arms (`Abs`/`Bipolar` never consult the source), so the lane
  loop's range-map dispatch went 2 → 4, not 6.
  `curve::tests::scale_norm_{direct_folds_clamps_then_bends,abs_opens_at_both_extremes,bipolar_gates_on_the_upper_half,lands_in_unit_range_for_every_combination}`,
  `scale_fold_resolves_to_four_arms_bitwise`.
- `MatrixSlot.scale_polarity` ([slot.rs:137](../../crates/vxn-core-matrix/src/slot.rs#L137));
  `Route.scale_bipolar` → `Route.scale_fold`, resolved in `compile_slots`
  ([slot.rs:393](../../crates/vxn-core-matrix/src/slot.rs#L393)). Both engines
  apply it — vxn-2 decodes it at
  [engine.rs:882](../../vxn-2/crates/vxn2-engine/src/engine.rs#L882), vxn-1b
  carries it through the Amp factoring at
  [bank.rs:1458](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L1458) and through
  the shared compile everywhere else. `[0, 1]` holds for every
  (source, polarity, shape) including NaN —
  `scale_norm_lands_in_unit_range_for_every_combination` asserts the NaN gate
  shut on all 18 combinations.
- **Bit-identical.** vxn-1b's render hash matches `EXPECTED` (`baseline.rs`, 4/4
  under `VXN_RENDER_HASH=1`). vxn-2 prints `0x95ac9a59d27aaddd` both here and
  from a `HEAD` worktree built before the change — byte-for-byte the same render;
  its `EXPECTED` is CI-captured and already failed on this box before 0341, as
  its own header documents. Null tests pass on both.
- vxn-2's state blob is unchanged in size and in every field offset. The scale
  nibble held a 3-value `Shape` and now holds the 9-value `(polarity, shape)`
  code — `curve_code(Direct, shape) == shape as u8`, so 0..=2 decode to exactly
  the VCA they always meant and 3..=8 are values no old blob ever wrote.
  `shared::tests::pre_0341_scale_nibble_decodes_to_the_same_vca`,
  `the_packed_matrix_word_keeps_its_field_offsets`. Field renamed
  `scale_shape` → `scale_curve` through row / codec / preset / JS; the preset key
  is `scale-curve` with `scale-shape` kept as a read alias.
- vxn-1b's topology record is 8 bytes at `VERSION = 14`
  ([state.rs:60-73](../../vxn-1b/crates/vxn1b-engine/src/state.rs#L60-L73)) —
  pinned together by `state::tests::the_slot_record_is_eight_bytes_at_this_version`,
  because a widened record read at the old version would slide bytes into the
  next layer's param block rather than fail. Preset gains a sparse
  `scale-polarity` key defaulting to `direct`:
  `preset::tests::{absent_curve_and_scale_default,scale_polarity_round_trips_and_stays_sparse_at_direct,unknown_scale_polarity_degrades_to_direct_with_warning}`,
  `state::tests::scale_polarity_round_trips_per_layer`.
- Golden `route(...)` takes a scale **curve** code
  ([golden.rs:197](../../crates/vxn-core-matrix/src/golden.rs#L197)), decoded with
  `curve_split` so an out-of-range byte degrades instead of aliasing. Eleven new
  cases: `Abs` at both extremes of a bipolar source, at its centre, on a unipolar
  source, and with both bends; `Bipolar` gating a unipolar source's upper half,
  shutting below the halfway point, clamping hard on a bipolar source, and with
  both bends. Coverage now asserts all 4 folds × 3 bends
  (`scale_folds_and_bends_are_all_covered`) *and* both source polarities under
  each scale polarity (`abs_scale_ignores_the_sources_own_polarity`).
- **`matrix_eval_scaled`: +1.4%, not within noise — recorded rather than
  claimed.** 73.1–74.4 ns → 74.4–75.5 ns, measured interleaved against a `HEAD`
  worktree binary over five rounds. The hoist survives (a de-hoisted `scale_norm`
  costs ~47–50%). Two supporting figures: the *fused* `match (fold, bend)`
  spelling — twelve expanded lane loops — cost **+4%**, and splitting into a
  4-arm range map then a 2-arm bend (`Lin` is the identity, no arm) recovers it
  to six loops, the same six as before the VCA had a polarity; and
  `matrix_eval_full`, whose routes are all unscaled and whose code path is
  untouched, moved ~+3% under **both** variants, so that share is code layout
  rather than dispatch. The measurement is in the `eval_dests_bank` comment;
  `matrix_compile_full` is +8%, real work paid once per block.
- Wire, both synths. vxn-1b: `MatrixField::ScalePolarity`, `scale-polarity` at
  ordinal 7 in `MATRIX_FIELD_NAMES`, codec address decode, and `scalePolarity` in
  the snapshot and the panel's `FIELDS` table (no control yet — that is 0340).
  `topology::tests::a_scale_polarity_edit_lands_on_its_own_column`,
  `codec::tests::matrix_addresses_round_trip`. vxn-2: the whole nine-value code
  rides the existing row, wire to slot —
  `engine::tests::a_wire_row_carries_the_scale_vcas_polarity_to_the_slot` — and
  the panel's scale pick-list already offers all nine from the exported `curves`
  list.
- Doc debt from the ticket's Notes cleared, reason kept and conclusion dropped:
  `scale_norm`'s note now explains that landing in `[0, 1]` is what *fixes the
  clamp-between-the-axes order* rather than what forbids a polarity axis, and
  spells out why "polarity then fold" degenerates to a no-op. Same for
  `MatrixSlot::scale_shape`'s "no polarity twin" and vxn-2's `scale_shape` doc.
  ADR 0003's "the packed word is exactly full" note records that 0341 was the
  first test of that and spent nothing. `vxn-2/DEVELOPERS.{md,html}` updated.
- `cargo test --workspace`: 1545 passed, 0 failed. JS: 43 suites / 347 tests
  (vxn-1b), 5 / 35 (vxn-2).
