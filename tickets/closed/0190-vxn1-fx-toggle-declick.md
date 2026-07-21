---
id: "0190"
product: vxn-1
title: "FX toggle declick — raised-cosine bypass crossfade on phaser/chorus/delay/reverb/limiter"
priority: medium
created: 2026-07-07
epic: E035
depends: []
---

## Summary

Toggling a master FX in vxn-1 hard-switches the audio path with no crossfade, so
every on/off edge clicks. Phaser/chorus/delay/reverb copy the dry bus through
when off and snap the wet in when on
([lib.rs:258-296](../../vxn-1/crates/vxn-engine/src/lib.rs#L258-L296)); the on/off
params are `Glide::Snap`
([smoothing.rs:113-131](../../vxn-1/crates/vxn-engine/src/smoothing.rs#L113-L131)).
Only the limiter clears state on its off→on edge
([lib.rs:300-306](../../vxn-1/crates/vxn-engine/src/lib.rs#L300-L306)).

Give each stage a short **equal-gain raised-cosine crossfade** between its dry
input and wet output, the same declick vxn-2 uses for its filter toggle
(`FILTER_XFADE_MS`, equal-gain because dry/wet are strongly correlated).

## Design

**Shared helper — [smoothing.rs](../../vxn-1/crates/vxn-engine/src/smoothing.rs)**

Add a small `BypassXfade`:

```rust
struct BypassXfade {
    len: usize,        // fade window in samples (~10 ms @ base rate)
    remaining: usize,  // 0 ⇒ idle; > 0 ⇒ fade in flight
    to_wet: bool,      // direction: true = dry→wet (engage), false = wet→dry
    on: bool,          // last-seen flag, for edge detection
}
```

- `arm(now_on: bool)` — on a flag edge, set `remaining = len`, `to_wet = now_on`,
  `on = now_on`. No-op if `now_on == on`.
- `active(&self) -> bool` — `remaining > 0`.
- Per-sample weight: `t = (len - remaining_at_sample) / (len-1)`,
  `rise = 0.5 - 0.5*cos(PI*t)`, then `(w_dry, w_wet) = to_wet ? (1-rise, rise) :
  (rise, 1-rise)`. Weights sum to 1 (equal-gain). Decrement `remaining` by the
  block length after the block. Match the exact curve/law vxn-2 documents in
  `render_block_filter_xfade`.

Fade length from a `ms → samples` helper at the base rate (reuse the
`one_pole_coeff` neighbourhood; no per-sample smoother needed here — this is a
deterministic ramp, not an exponential glide).

**Per-stage application — [lib.rs `MasterFx`](../../vxn-1/crates/vxn-engine/src/lib.rs#L245)**

One `BypassXfade` per toggleable stage: `phaser`, `chorus`, `delay`, `reverb`.
Rewrite `process_block` so each stage:

1. Edge-detects via `xfade.arm(flag)`. On the **off→on** edge, reset that stage's
   DSP state first (as the limiter already does) so wet starts from a clean tail.
2. Runs wet **iff** `flag || xfade.active()` — otherwise the current zero-cost
   passthrough copy stands. This gate is what keeps idle CPU flat and preserves
   the sample-exact-when-absent guarantee
   ([lib.rs:242-244](../../vxn-1/crates/vxn-engine/src/lib.rs#L242-L244)): the
   passthrough branch is only taken when the fade is fully idle.
3. When running wet during a fade, blend into the output:
   `out[i] = w_dry*dry[i] + w_wet*wet[i]`. When `flag && !active`, wet passes
   straight through (steady on). The stage's input `dry` is the previous stage's
   output bus (chain order phaser→chorus→delay→reverb unchanged).

Stage notes:
- **Phaser / chorus** are `process_block_stereo(in, out)` — keep the input bus,
  process to a wet temp, blend. Chorus already copies its input to a temp
  ([lib.rs:269-274](../../vxn-1/crates/vxn-engine/src/lib.rs#L269-L274)); reuse it
  as the dry reference.
- **Delay** is per-sample
  ([lib.rs:278-284](../../vxn-1/crates/vxn-engine/src/lib.rs#L278-L284)) — capture
  `dry = out[i]` before `self.delay.process`, blend after. During a fade-out the
  echo tail fades gently instead of cutting.
- **Reverb** wraps its own internal dry/wet
  ([lib.rs:289-296](../../vxn-1/crates/vxn-engine/src/lib.rs#L289-L296)); the
  bypass fade crossfades the reverb *output* bus against the reverb *input* bus —
  a second, outer mix. On on→off the tail fades under the ramp rather than
  cutting.
- **Limiter** — keep the existing reset-on-edge
  ([lib.rs:300-306](../../vxn-1/crates/vxn-engine/src/lib.rs#L300-L306)); add a
  `BypassXfade` too since engaging it steps the level. Lower priority than the
  four wet effects; land it here for consistency.

## Acceptance criteria

- [x] `BypassXfade` helper exists with the equal-gain raised-cosine weight (zero
      slope at both endpoints), a `ms→samples` window (~10 ms), edge-armed.
- [x] Phaser/chorus/delay/reverb/limiter each crossfade dry↔wet on both toggle
      edges; off→on resets the stage's DSP state before the wet fades in.
- [x] Wet is computed only when `flag || fade active`; once an on→off fade
      completes the stage returns to the zero-cost passthrough and the engine is
      bit-exact vs the current effect-absent fast path (existing sample-exact
      test still green — `baseline_render_is_stable` unchanged, plus new
      `all_fx_off_is_bit_exact_across_fx_params`).
- [x] No per-sample cost added in steady state; idle profile unchanged vs
      [[vxn1-render-loop-optimized]] (passthrough branch taken only when the fade
      is fully idle; wet gated on `flag || active`).
- [x] `cargo test -p vxn-engine` green.

## Close-out

Landed in `smoothing.rs` (`BypassXfade` + `raised_cosine_rise` + `ms_to_samples`)
and `lib.rs` (`MasterFx`). Each of phaser/chorus/delay/reverb/limiter owns a
`BypassXfade`; `process_block` arms on the flag edge, resets the stage's DSP on
off→on, and blends `w_dry·dry + w_wet·wet` while the fade is active, falling back
to the exact passthrough copy when idle. Fade window `FX_XFADE_MS = 10.0`.

Notable changes beyond the spec:

- **`limiter_was_on` removed.** The limiter's reset-on-off→on edge is now driven
  by `BypassXfade::arm` returning the engage edge (same behaviour, one mechanism).
- **Reverb held internally `on`.** `MasterFx::update` now sets the FDN's internal
  bypass to `true` unconditionally; master reverb on/off is owned by the outer
  `reverb_fade` gate + crossfade. This is what lets the reverb tail keep sounding
  *through* an on→off fade instead of the internal flag snapping the wet away
  mid-fade. `update`'s `reverb_on` param dropped (redundant).
- **First-block prime.** A `fades_primed` guard adopts the live flags on the
  first `process_block` after construction/reset (via `BypassXfade::prime`), so an
  effect that boots engaged (e.g. the default-patch chorus) doesn't ramp in at
  startup — this keeps `baseline_render_is_stable` bit-identical.

Offline click test (`tests/declick.rs`, 0192): the **join** `d4` (the switch
sample itself) drops from ~2.3e-1 on a hard switch to ~1.6e-4 with the crossfade
for every effect — the hard-switch click is gone. An LFO-modulated effect's own
cold-start onset (chorus/delay line filling, limiter grabbing) still shows a few
ms after the edge; the crossfade attenuates it under low wet weight but can't
remove it without keeping the effect warm while bypassed (CPU-gated design
forbids). Not a toggle click. Reaper listen + final fade-length tuning: [[0192]].

## Notes

- Ported mechanism: vxn-2 `FILTER_XFADE_MS` + `render_block_filter_xfade`
  (equal-gain raised cosine). We apply it per-stage rather than whole-chain
  because vxn-1 FX carry stateful delay lines/LFOs that can't be dual-rendered.
- Sibling ticket [[0191]] reuses this helper for the oversampling fade-in.
- DAW listen + fade-length tuning tracked in [[0192]] / [[verify-audio-in-reaper]].
