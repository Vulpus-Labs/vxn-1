---
id: "0202"
product: vxn-1b
title: "Matrix evaluator: replace fixed routing with generic source→dest accumulate; render-parity gate"
priority: high
created: 2026-07-25
epic: E036
---

## Summary

The spine of VXN1b. Replace VXN1's fixed per-channel routing loop with a generic
mod-matrix evaluator ([ADR 0001](../../vxn-1b/adrs/0001-vxn1b-overall-design.md)
§2, §4), preserving VXN1's smoothing so the seeded default patch (0201) sounds
**bit-identical** to VXN1.

- **Per control block (sr/32):** evaluate all ten sources into a
  `[lane][source]` table (envs, LFOs, velocity, key, wheels, aftertouch pressure
  from 0198, note-random from 0199).
- **Accumulate per dest:** `out[dest] += source · curve(depth) ·
  scale_norm(scale_src)`, slots summing additively. `scale_norm`: unipolar
  passthrough; bipolar `(x+1)·0.5` clamp `[0,1]` (ADR 0009; polarity table from
  0201).
- **Dest application keeps VXN1 granularity:** per-sample cutoff coefficient
  interpolation, per-sample pitch, block-rate one-pole for gain-like dests.
- **Allocation-free**; extend the alloc-trap test.

The old fixed resolution (`cutoff_mod = lfo1·d_lfo1 + …`) is deleted; the generic
evaluator subsumes it.

## Acceptance criteria

- [ ] Generic evaluator produces per-dest modulation totals from the 16 slots +
      10 sources; slots to one dest sum.
- [ ] `scale_norm` matches the ADR 0009 polarity table; a `scale_src`=ModWheel
      route contributes 0 at wheel 0, full at wheel 1; a bipolar scale follows
      `(x+1)·0.5`.
- [ ] VXN1 smoothing preserved: cutoff coeff interpolated per sample; pitch per
      sample; gains block-rate.
- [ ] **Render-parity test:** the seeded default patch renders bit-identical (or
      within a documented float-hash tolerance) to VXN1's default-patch output
      for a fixed note/param sequence.
- [ ] Hot path allocation-free (alloc-trap test extended).

## Notes

- **Build the parity fixture first** — capture a VXN1 default-patch render (fixed
  seed, notes, block count) and assert VXN1b matches. This is the gate for the
  whole variant being faithful.
- Source eval stays **per-block**; only dest application is per-sample where
  VXN1's is — a naive per-sample matrix eval would regress CPU (RT discipline).
- Watch the SoA/branch lessons: keep the lane loop branch-light so NEON survives
  (`vxn1-soa-match-defeats-simd`); curve dispatch out of the inner lane loop.
- Depends on 0198, 0199, 0200, 0201. Feeds 0203 (persistence), 0204 (CLAP).
</content>

## Close-out (2026-07-26)

- Generic evaluator ([eval.rs](../../vxn-1b/crates/vxn1b-engine/src/eval.rs)):
  `eval_sources` → `eval_dests` accumulate `curve(source)·depth·DEST_GAIN·scale_norm`,
  slots summing additively; `scale_norm` per ADR 0009 polarity. Apply layer
  ([render.rs](../../vxn-1b/crates/vxn1b-engine/src/render.rs)) maps DestVals onto
  VXN1's pitch/cutoff/pwm/reso/amp math.
- Render fork ([bank.rs](../../vxn-1b/crates/vxn1b-engine/src/bank.rs)): 8-wide
  `RenderBank` forks VXN1's `render_block`, `resolve_mod` replaced by the matrix.
  Per-frame Amp (VCA follows Env2) factored `static + e1·env1 + e2·env2`; cutoff
  coeff ramped per-sample (inherited), gains block-rate.
- Engine ([engine.rs](../../vxn-1b/crates/vxn1b-engine/src/engine.rs)): 16-voice
  (2 banks), control-block chunking, mod-agnostic `BlockCtx` from params.
- **Render-parity gate** (`tests/parity.rs::default_patch_render_matches_vxn1`):
  default patch vs VXN1 (OS/FX/vibrato off), RMS ratio 1.00006, peaks identical;
  band ±0.5%. Alloc-free: `tests/alloc_free.rs`.
- Locked constants: `DEST_GAIN` (Pitch 12, XModSweep 48, Pwm 0.5, Cutoff 48,
  Amp 1, CrossModAmount 4), Key source = octaves rel C4, `KEY_CUTOFF_UNITY_DEPTH=0.25`.
- Deferred (inert at default): oversampling/decimation, FX (E037), unison/mono,
  E022 trims, per-voice `HpfCutoff`/`CrossModAmount` dests, LFO→Amp declick.
