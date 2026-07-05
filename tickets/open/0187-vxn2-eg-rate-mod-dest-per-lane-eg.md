---
id: "0187"
product: vxn-2
title: "Mod-matrix eg-rate dests (global + per-op) on a per-lane amp EG — VoiceSpread-driven rate divergence"
priority: medium
created: 2026-07-05
epic: null
depends: []
---

## Summary

Add mod-matrix **targets** that scale envelope *rate*, so a `VoiceSpread →
eg-rate` route makes the voices in a stack evolve at slightly different speeds
(decorrelation). Companion to `stack-detune`/`stack-spread`: those spread
pitch/pan, this spreads envelope *time*. Covers all three envelope families:

- **Amp EGs** (per-op, per-lane) — smears transients, decorrelates the rate at
  which per-op modulation ramps. Dests `op1..6-eg-rate` (+ `global-eg-rate`).
- **Pitch EG** (per-lane) — decorrelates the pitch sweep across the unison stack
  → chorusing. Dest `pitch-eg-rate` (+ `global-eg-rate`).
- **Mod env** (one-per-voice, *not* per-lane — it drives per-stack targets like
  filter cutoff where lane decorrelation is meaningless). Dest `mod-env-rate`
  (+ `global-eg-rate`), scaled uniformly per note; best driven by per-stack
  sources (velocity/key/LFO). `voice-spread → mod-env-rate` correctly reads as
  tier-collapse.

`global-eg-rate` scales **all three** families at once; the per-op / pitch /
mod dests layer on top.

Blocker uncovered while scoping: the amp EG is currently **per-op scalar,
shared across all 8 unison lanes**, not per-lane. One `EgState` per op
([stack.rs:202](../../vxn-2/crates/vxn2-dsp/src/stack.rs#L202)), ticked once per
op ([stack.rs:848](../../vxn-2/crates/vxn2-dsp/src/stack.rs#L848)), broadcast to
lanes at render
([stack.rs:1029](../../vxn-2/crates/vxn2-dsp/src/stack.rs#L1029) →
[1047](../../vxn-2/crates/vxn2-dsp/src/stack.rs#L1047) `(lvl + lvl_mod[k])`).
Per-lane amplitude variation today is an additive `lvl_mod[k]` offset, **not**
an independent envelope — so there is nothing per-lane to scale. Real rate
divergence (stages transitioning at different times) needs per-lane EG state.
The `rate_mult` arg to `eg.cook()`
([eg.rs:164](../../vxn-2/crates/vxn2-dsp/src/eg.rs#L164)) is the fold point once
the EG is per-lane.

Cost is control-rate, not per-sample: the marcher runs in `eg_tick`, so 6 ops ×
8 lanes = 48 scalar marchers/tick vs 6 now; render already loops per lane so the
per-lane level read is ~free. (The `vxn1-envelope-soa-not-worth-it` memory was
about a *per-sample* SoA EG — does not apply here.)

## Design

**Per-lane amp EG (DSP — [vxn2-dsp/src/stack.rs](../../vxn-2/crates/vxn2-dsp/src/stack.rs), [eg.rs](../../vxn-2/crates/vxn2-dsp/src/eg.rs))**

- `Op.eg: EgState` → `eg: [EgState; STACK_LANES]`
  ([stack.rs:202](../../vxn-2/crates/vxn2-dsp/src/stack.rs#L202)).
- Fan every touchpoint over lanes: `note_on`
  ([562](../../vxn-2/crates/vxn2-dsp/src/stack.rs#L562)), `note_off`
  ([632](../../vxn-2/crates/vxn2-dsp/src/stack.rs#L632)), `eg_all_idle`
  ([650](../../vxn-2/crates/vxn2-dsp/src/stack.rs#L650)), radiated-amp sum
  ([669](../../vxn-2/crates/vxn2-dsp/src/stack.rs#L669)), `reset`
  ([683](../../vxn-2/crates/vxn2-dsp/src/stack.rs#L683)), `kill_release`
  ([701](../../vxn-2/crates/vxn2-dsp/src/stack.rs#L701)), `force_sustain`
  ([877](../../vxn-2/crates/vxn2-dsp/src/stack.rs#L877)), `eg_tick`
  ([848](../../vxn-2/crates/vxn2-dsp/src/stack.rs#L848) — tick 8× same `dt`),
  `cook_op` ([939](../../vxn-2/crates/vxn2-dsp/src/stack.rs#L939)).
- Render hot loop reads a **contiguous** per-block level mirror
  `StackCore::op_eg_level: [[f32; STACK_LANES]; N_OPS]`, NOT `ops[i].eg[k].level`
  directly. Gathering `.level` across 8 ~80 B `EgState` structs every sample
  strides ~10 cache lines/op and cost **+7–8%** steady-state (measured, `stack`
  bench 55→59 µs). The mirror (laid out like `op_level_mod`) is refreshed once
  per control tick in `refresh_eg_levels` — called from `eg_tick`, `note_on`,
  `force_sustain`, `note_off`/`silence`; the render then does a straight
  vectorisable load and perf returns to baseline (within 0.5 %, NEON `.4s` lane
  ops intact). EG level is constant across a block so the mirror is exact.
- New `rescale_eg_rates(&[[f32; STACK_LANES]; N_OPS])`: multiply each lane EG's
  `rates_per_sec` **and** `log_rates` in place. Called right after cook — no
  re-cook, so `note_on` needs no structural split. Re-applied after
  `retarget_pitch` (legato re-cooks).

**eg-rate dests (engine — [vxn2-engine/src/matrix.rs](../../vxn-2/crates/vxn2-engine/src/matrix.rs), [engine.rs](../../vxn-2/crates/vxn2-engine/src/engine.rs))**

- 7 new `DestId` variants appended **after `FilterDrive`** so the blob dest
  space stays a 1:1 prefix for older patches: `GlobalEgRate`,
  `Op1EgRate..Op6EgRate`. `GlobalEgRate` scales all 6 ops; per-op dests add on
  top of it.
- Update `N_DESTS` 42→49, `DEST_NAMES`, `DEST_LABELS`, `DEST_GAIN`, `tier()`,
  `from_u8`, and the tier-table test
  ([matrix.rs:1456](../../vxn-2/crates/vxn2-engine/src/matrix.rs#L1456)).
- Tier = **PerLane** — so `VoiceSpread` (per-lane) → eg-rate is coherent (no
  tier-collapse), which is the whole point.
- Domain = **log/octave** like `lfo*-rate`/`cutoff`/`filter-drive`:
  `DEST_GAIN = 4.0` (±4 octaves = ×16/÷16 rate at full depth), matching the
  sibling rate dests. A narrower span reads as almost no effect because summing
  many unison lanes averages their envelopes (unlike detune, rate spread smears
  rather than beats); consumer clamps the summed octaves to ±4. Linear taper
  (the log gain already shapes it). The `voice-spread` *source* is additionally
  scaled by the Stack-Spread param, so a low spread setting shrinks this route
  regardless of depth — crank Spread (and prefer plucky/percussive patches) for
  an obvious effect.
- **Note-on static** consumption (not block-rate): after `stack.note_on` fills
  lane meta, snapshot the note-on source values → reuse `eval_sources` +
  `eval_dests` into a scratch `LaneDestVals` → per op·lane scale
  `= exp2(dest[k][GlobalEgRate] + dest[k][OpNEgRate])`, clamp → `rescale_eg_rates`.
  Re-run after `retarget_pitch`.

**Pitch env → per-lane, Mod env stays single (DSP)**

- `meta.pitch_eg: PitchEgState` → `[PitchEgState; STACK_LANES]`. Unlike the amp
  EG, the pitch EG has **no per-sample read** — its `level_st` folds into
  `base_st` in the block-rate `apply_pitch_mult`, baked into per-lane
  `phase_inc`. So per-lane pitch env costs only block-rate work: **no SoA mirror
  needed**. The `PitchEg` matrix *source* reads lane 0 to keep its per-stack
  tier (all lanes identical unless a pitch route decorrelates them).
- `PitchEgState::scale_rates` + `Stack::rescale_pitch_eg_rates(&[f32; LANES])`.
- Mod env stays `meta.mod_env: ModEnvState` (single). It's *time*-based (ms →
  slopes), so `ModEnvState::scale_rates` multiplies slopes (`Lin`) / divides
  taus (`Exp`); `Stack::rescale_mod_env_rate(f32)`.
- **Perf:** `eg_tick` gates the per-lane amp+pitch env ticks to active lanes
  (`< density`) — a density-1 voice ticks one lane, not eight; `eg_all_idle`
  matches the gate so retirement still fires. `stack` bench: d1/d4 back to
  baseline, d8 (max unison) +~1.75% block-rate only, per-sample hot path
  unchanged.

**Dests / eval (engine)**

- Appended after the amp dests: `PitchEgRate` (PerLane), `ModEnvRate`
  (PerStack). `N_DESTS` 49→51. `GlobalEgRate` now feeds amp+pitch (per-lane) and
  mod env (lane-0 collapse).
- Fresh-block note-on eval computes per-lane amp + pitch scales and a lane-0 mod
  scale, all `exp2(clamp(±4 oct))`, and calls the three rescale methods.

## Acceptance criteria

- [ ] Amp EG is per-lane (`[EgState; STACK_LANES]` per op); all lifecycle,
      tick, render, and test touchpoints updated; existing EG/stack tests pass.
- [ ] 7 new dests wired end to end: enum, `N_DESTS`, names/labels/gain, `tier()`
      (PerLane), `from_u8`, tier-table test extended.
- [ ] Note-on eval applies a per op·lane rate scale from the matrix;
      `VoiceSpread → eg-rate` at a nonzero depth makes two lanes of one stack
      reach a given EG stage at measurably different times (new DSP test).
- [ ] `eg-rate` depth 0 (or no route) leaves EG rates bit-identical to today
      (regression guard).
- [ ] Older patch blobs (dest ids ≤ 42) still decode unchanged; new dests
      appear in the matrix UI dropdown via the exported descriptor.
- [ ] `cargo test -p vxn2-dsp -p vxn2-engine` green; no per-sample cost added
      to the render loop.

## Notes

- Design decisions confirmed with user: **per-lane EG (faithful)**, scope =
  amp EGs only with **both** a global (all-ops) dest and per-op dests, timing =
  **note-on static** (spread is fixed per voice, so no continuous re-scale
  needed — this is why an in-place `rescale_eg_rates` beats a note_on split).
- Out of scope: pitch-EG / mod-env rate scaling (amp EGs only); continuous
  (per-tick) rate modulation from live sources like LFO.
- Related: [[vxn2-stack-soa]], [[vxn2-level-mod-pipeline]],
  [[vxn2-architecture]]. Sibling stack-spread macro lives in the same
  eval/note-on machinery ([matrix.rs:333](../../vxn-2/crates/vxn2-engine/src/matrix.rs#L333)).
- Follow the append-only blob discipline the dest table already documents
  ([matrix.rs:365](../../vxn-2/crates/vxn2-engine/src/matrix.rs#L365)); do not
  reorder existing dest ids.
