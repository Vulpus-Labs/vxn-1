---
id: "0332"
product: monorepo
title: "One roster row per destination: gain, depth taper, tier and smoothing class together"
priority: medium
created: 2026-08-30
epic: E049
depends: ["0329"]
---

## Summary

A destination's properties currently live in four structures kept in step by
hand. For vxn-2: `DEST_GAIN` (a const array), `cook_depth` (a match arm list),
`PITCH_DESTS` (a const array naming the smoothed dests), and `DestId::tier` (a
match). For vxn-1b: `DEST_GAIN`, `cook_depth`, and the smoothing tiers, which
live in a different module entirely
([mod_smoothing.rs](../../vxn-1b/crates/vxn1b-engine/src/mod_smoothing.rs)).

Add a destination and you must remember all four. Nothing enforces it — a
missing `DEST_GAIN` entry is a silent 0.0 or a wrong default, and a dest left
out of the smoothing list simply stairsteps.

Extend `matrix_enum!` (shared as of [0330](0330-share-curve-vocabulary.md)) so a
destination row declares everything keyed on it, and the four structures become
generated.

## Design

```text
Cutoff = 4, "cutoff", "Cutoff", gain = 48.0, taper = linear,
             tier = per_stack, smooth = block;
Pitch  = 1, "pitch",  "Pitch",  gain = 12.0, taper = cubic,
             tier = per_lane,  smooth = quantum_cascade;
```

Every column mandatory. The property this buys is the one `matrix_enum!`
already buys for names: **you cannot add a destination without deciding**,
because the row will not compile until every column is filled. That is the whole
point — not brevity.

`Tier` and `Smoothing` come from [0329](0329-vxn-core-matrix-crate-skeleton.md).
Sources keep their existing `uni`/`bi` column, which is the same idea and
already works this way in vxn-1b.

## Acceptance criteria

- [ ] `DEST_GAIN`, `cook_depth`, `tier` and the smoothing classification are
      generated from the row list in both synths; no hand-maintained parallel
      arrays remain.
- [ ] Omitting any column is a **compile** error, and a test or doc comment
      demonstrates that (a `compile_fail` doctest is the cheap way).
- [ ] Every generated value equals its hand-written predecessor. Diff the
      generated `DEST_GAIN` against the old array elementwise in a test that is
      then deleted in the same commit — the point is to prove the transcription,
      not to keep the old array alive.
- [ ] vxn-2's `PITCH_DESTS` is derived from `smooth = quantum_cascade` rather
      than declared separately.
- [ ] Both render-hash baselines **byte-identical** — constants are transcribed
      here, not recomputed, so this is the other ticket held strictly. A moved
      bit means a gain or taper was transcribed wrong, which the elementwise
      diff above should already have caught.

## Notes

- Transcription is where this ticket will go wrong: 51 vxn-2 destinations, each
  with four properties, moved by hand. Do it in one mechanical pass and lean on
  the elementwise diff test above rather than reading it twice.
- **The row list generates two name tables, not one.** The roster's `u8` is a
  storage index with the sentinel excluded, so its tables are `N` long while the
  synth's existing wire tables stay `N + 1` with `None` at 0. Both come off the
  same row list; neither is hand-maintained. Decided in 0329 and recorded in
  [ADR 0003](../../adrs/0003-vxn-core-matrix.md) §2 — read that before writing
  the macro, because it is the difference between transcribing the existing
  tables unchanged and generating a second, shorter pair.

- `Smoothing` is only *declared* here — nothing reads it until
  [0335](0335-declared-target-smoothing.md). Declaring it early means the
  smoothing ticket is a consumer change, not a data-entry change.
- vxn-1b declares `Amp = block` even though its bank smooths part of the Amp
  coefficient per-sample. That is the documented exception in
  [ADR 0003](../../adrs/0003-vxn-core-matrix.md) §3: the envelope part must stay
  per-frame exact, so vxn-1b's VCA does its own factoring and the engine is not
  told about it. Put that in a comment on the row, or someone will "fix" it.
- The same applies to vxn-2's engine-side motion (ADR 0003 §3): op
  level/pan/phase dests (per-sample linear ramp), `StackDetune`/`StackSpread`
  (block-rate one-pole) and the nine EG-rate dests (consumed once at note-on)
  all declare `block` — their motion is target application, not routing.
  Comment each of those rows too.
- Taper column facts to transcribe carefully: **13** vxn-2 dests take the cubic
  taper — 7 of the 8 `PITCH_DESTS` (`Lfo2Phase` explicitly passes through,
  gain 1.0) plus the 6 stack-pitch dests, which are *not* in `PITCH_DESTS`.
  The taper set and the smoothing set overlap but are not the same set; keep
  the columns independent and derive neither from the other. Don't forget the
  fifth hand-synced structure while retiring the four named above:
  `is_pitch_shaped`, kept in step with `PITCH_DESTS` only by a test.

## Close-out (2026-09-01)

- `matrix_enum!`'s destination form now takes four mandatory columns — `gain =`,
  `taper =`, `tier =`, `smooth =` — and generates `DEST_GAIN`, `cook_depth`,
  `tier` and the smoothing classification from the row list in both synths
  ([curve.rs](../../crates/vxn-core-matrix/src/curve.rs)). The hand-kept arrays
  are gone: vxn-2's `DEST_GAIN`, `DestId::tier`, `DestId::cook_depth` and
  `is_pitch_shaped` no longer exist as hand-written items, and vxn-1b's
  `DEST_GAIN` became a generated table.
- Omitting a column is a compile error, shown by a `compile_fail` doctest on the
  macro ([curve.rs:86](../../crates/vxn-core-matrix/src/curve.rs#L86)).
- The macro emits the sentinel-free roster tables (`N` long) beside each synth's
  existing `N + 1` wire tables, off the same rows, per ADR 0003 §2.
- `PITCH_DESTS` is derived from `smooth = quantum_cascade`
  ([matrix.rs:566](../../vxn-2/crates/vxn2-engine/src/matrix.rs#L566)) rather
  than declared, which retires the fifth hand-synced structure and the
  `pitch_shaped_set_matches_constant` test that policed it.
- **Transcription proved mechanically, not by re-reading.** Every dest's gain,
  tier, pitch-set membership and nine cooked depths were dumped as raw bits from
  the pre-ticket tree and from this one and diffed: identical across all 52
  vxn-2 and 17 vxn-1b destinations. The dumps were temporary and are not
  checked in.
- Deriving `PITCH_DESTS` changed its **order** — `[GlobalPitch, Lfo2Phase,
  Op1..Op6]` became `[Op1..Op6, GlobalPitch, Lfo2Phase]` — so the pitch
  smoother's row indices are now resolved by name at compile time in
  [engine.rs](../../vxn-2/crates/vxn2-engine/src/engine.rs) instead of written as
  literals. Null test `-inf dBFS` on both engines confirms no row got crossed.
- Both render hashes byte-identical to the branch base. (vxn-2's `EXPECTED`
  fails locally on a macOS 14 debug build, which is the documented
  profile/OS artifact recorded in `baseline.rs` — not a regression, and no
  re-capture is owed.)
- Review fix carried in the same commit: the const guard on vxn-1b's gain table
  now asks `index()` for the row rather than assuming it equals `i`, so it can
  still fail if 0333 moves `index()` into discriminant space.
