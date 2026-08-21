---
id: "0276"
product: vxn-1b
title: "Break up bank::render — 490 lines over 18 parallel lane arrays"
priority: medium
created: 2026-08-21
epic: null
depends: ["0273", "0275"]
---

## Summary

[`RenderBank::render`](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L549) runs
from line 549 to line 1045 — just under 500 lines in one function. It has grown
one destination at a time (0208 smoothing, 0242 cross-mod, 0260 Pan, 0261 per-osc
PWM, 0268–0270 envelope/LFO dests, 0271 lifetime) and every addition took the
same shape: another `[T; N]` scratch array, another target, another `_active`
flag, another branch in the per-quantum tick.

The block-start lane loop
([bank.rs:598–765](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L598-L765)) alone
is ~150 lines and declares eighteen scratch arrays it fills in lockstep:

```rust
let mut pw1 = [0.5f32; N];      let mut pw2 = [0.5f32; N];
let mut amp_c = [AmpCoeffs::default(); N];  let mut amp_stat_tgt = [0.0f32; N];
let mut base1 = [0.0f32; N];    let mut base2 = [0.0f32; N];
let mut pitch_tgt = [0.0f32; N]; let mut sweep_tgt = [0.0f32; N];
let mut pwm_tgt = [(0.0f32, 0.0f32); N];
let mut xmod_tgt = [0.0f32; N]; let mut pm_idx = [0.0f32; N];
let mut pitch_active = [false; N]; let mut pwm_active = [false; N];
let mut xmod_active = [false; N];
let mut pan_tgt = [0.0f32; N];  let mut pan_active = [false; N];
```

Nothing here is *wrong* — the array-of-lanes layout is deliberate and NEON-
friendly, and the hot loop should stay that way. The problem is that the function
no longer has a readable shape: the reader cannot see where "resolve this block's
per-lane state" ends and "render frames" begins, and the next destination will
make it worse.

## Design

Three extractions, none of which changes the data layout or the loop structure.

**(a) `LaneTargets`.** `base1`, `base2`, `pitch_tgt`, `sweep_tgt`, `pwm_tgt`,
`xmod_tgt`, `pan_tgt`, `amp_stat_tgt` are one logical record per lane, written
together and read together. Replace the eight arrays with `[LaneTargets; N]`.
The trigger path then collapses from

```rust
self.smooth.snap_pitch(v, pitch_tgt[v], sweep_tgt[v]);
self.smooth.snap_slow(v, pwm_tgt[v], xmod_tgt[v], ac.stat);
self.smooth.snap_pan(v, pan_tgt[v]);
```

to a single `self.smooth.snap_all(v, &tgt[v])`, which is also where the
three-way `snap_pitch`/`snap_slow`/`snap_pan` split stops needing explanation.

Keep the `_active` flags as separate `[bool; N]` arrays — they are read by the
`any()` reductions and the per-quantum branch, not alongside the targets.

**(b) `fn lane_sources(&self, v, ctx, ...) -> SourceInputs`.** The
`SourceInputs` construction at
[bank.rs:622–643](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L622-L643) plus the
LFO-1 onset gain above it is self-contained and carries three paragraphs of
comment about `spread_pos`. It reads far better as a named function.

**(c) `fn free_released_lanes(&mut self, active, gate, ...)`.** The 0271
voice-lifetime block
([bank.rs:1006–1035](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L1006-L1035)) is
thirty lines of allocation *policy* — with a nine-line rationale comment — living
in the middle of the frame loop. It uses no loop-local state beyond `v`. Lift it
whole.

Out of scope: the frame loop's kernel dispatch (sync / PM / plain, ring, sub,
noise) stays exactly as it is. It is dense, but it is dense for codegen reasons
([[vxn1-soa-match-defeats-simd]]) and breaking it up risks the vectorisation.

## Acceptance criteria

- [ ] ~~`RenderBank::render` is under ~250 lines~~ — **not met, criterion was
      unrealistic**: 459 → 433. See the close-out. The block-start resolution
      and the frame loop *are* visibly separated.
- [ ] ~~Scratch declarations at least halved~~ — **not met, criterion was
      miscounted**: 30 → 24. See the close-out.
- [x] `snap_pitch` / `snap_slow` / `snap_pan` are called through one entry point.
- [x] Default-patch render is **bit-identical** before and after.
- [x] A modulated render (LFO→Pitch, LFO→Pan, Env→Cutoff, wheel→PWM) is
      bit-identical before and after.
- [x] No regression in the busy-profile timing — spot-check against
      [[vxn1-render-loop-optimized]]'s recipe, since `LaneTargets` changes the
      scratch layout the block-start loop writes.

## Notes

Depends on [0273](0273-vxn1b-routing-rules-single-statement.md) (dead parallel
implementation gone, clamps named) and
[0275](0275-vxn1b-motion-smoother-lane-onepole.md) (smoother collapsed) — both
make this a smaller diff.

The bit-identity criteria are the real gate. This is a readability ticket; any
audible difference means it was done wrong.

Deliberately **not** included: a macro over `DestId`'s enum / `from_u8` /
`DEST_NAMES` / `DEST_LABELS` / `DEST_GAIN` parallel tables. Adding a destination
currently means five edits across three files, which is a genuine tax, but the
tables are readable as they stand and a macro would cost more legibility than it
saves. Revisit if the dest set grows much past its current 16.

## Close-out

Landed 2026-08-21. Files touched: `vxn1b-engine/src/{bank.rs, mod_smoothing.rs}`.

Landed as designed:

- **`LaneTargets`** replaces eight parallel `[f32; N]` arrays (`base1`, `base2`,
  `pitch_tgt`, `sweep_tgt`, `pwm_tgt`, `xmod_tgt`, `pan_tgt`, `amp_stat_tgt`)
  with one record per lane. The `_active` flags stayed separate arrays, as
  planned — they feed the cross-lane `any()` reductions.
- **`MotionSmoother::snap_all`** replaces the three-call `snap_pitch` /
  `snap_slow` / `snap_pan` sequence at the trigger site.
- **`lane_sources`** extracts the `SourceInputs` build and the LFO-1 onset gain.
- **`free_released_lanes`** lifts 0271's ~30-line lifetime policy out of the
  frame loop.
- **`set_lane_filter`** (beyond the ticket) extracts the ladder-coefficient tail
  of the lane loop and returns the HPF cutoff.
- Four `═══ Phase N ═══` markers name the block's structure: bank-wide setup,
  per-lane resolution, cross-lane decisions, frame loop.

Two acceptance criteria were **not** met, and were unrealistic as written:

- **"Under ~250 lines"** — `render` is 433, down from 459. The frame loop's
  kernel dispatch is ~200 of those and was deliberately left alone for codegen
  reasons ([[vxn1-soa-match-defeats-simd]]); the ticket scoped it out but then
  set a target that assumed it away.
- **"Scratch declarations at least halved"** — 30 → 24. Eight arrays collapsed
  into `tgt`, but the frame loop's own lane buffers (`o1`, `o2`, `ring`, `sub`,
  `noise`, `mix`, `hp`, `filt`, `amp`, `pan_l`, `pan_r`) are most of the count
  and are not targets. The number was set by eye over the block-start section
  only.

The readability goal the numbers were proxies for is met: the phases are named,
each is short, and the per-lane pass reads as resolve-then-cook rather than as
eighteen interleaved array writes.

Bit-identity confirmed on all four patches (default, all continuous dests live,
the note-on-latched dests, cross-mod + HPF).

`busy_profile` spot-check, release build: poly 14.6× → 15.1× realtime, stack
15.1× → 15.0×. No regression — `LaneTargets` interleaving the per-lane writes
did not cost anything measurable.
