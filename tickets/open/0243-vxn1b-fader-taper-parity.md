---
id: "0243"
product: vxn-1b
title: "Fader calibration parity with VXN1 — apply the descriptor taper in SharedParams"
priority: high
created: 2026-08-06
epic: E039
depends: ["0209"]
---

## Summary

VXN1b's faders are **linear in plain value**; VXN1's are tapered. Cutoff is the
obvious case: the descriptor declares `Exp { mid: 800 }` over 16.35 Hz …
16 kHz in *both* synths, but VXN1b never applies it, so 800 Hz sits at ~5% of
the travel and the top half of the fader spans 8 k → 16 k. Every low value is
undialable; the two synths feel different on identical descriptors.

The divergence is two lines in
[vxn1b-engine/src/shared.rs](../../vxn-1b/crates/vxn1b-engine/src/shared.rs):

| | VXN1 (`vxn-engine/src/shared.rs`) | VXN1b (fork) |
|---|---|---|
| `get_normalized` | `desc.to_fader(v)` | `desc.to_normalized(v)` |
| `set_normalized` | `desc.from_fader(n)` | `desc.from_normalized(n)` |

`vxn-core-app`'s split is deliberate: `to_normalized`/`from_normalized` are the
*linear* host mapping, `to_fader`/`from_fader` are the *editor* mapping that
applies the taper. VXN1b's fork picked the linear pair.

## Survey (what was compared)

- **Descriptor tables.** Every float param in VXN1's `PatchParam`/`GlobalParam`
  against VXN1b's `ParamId`, on (min, max, default, unit, taper). **Zero
  mismatches** across all shared names — the tables were already like-for-like
  (cutoff `Exp{800}`, hpf `Exp{1000}`, env times `Exp{1}`, drive `Exp{1}`,
  glide `Exp{0.1}`, phaser rate `Exp{1}`, reverb decay `Exp{2}`, dynamics
  attack/release `Exp{10}`/`Exp{100}`). VXN1's `lfo_rate` (`Exp{5}`) matches
  both of VXN1b's per-layer `lfo1_rate`/`lfo2_rate`.
- **VXN1-only floats** are exactly the fixed-routing depths the matrix replaced
  (`*_lfo_depth`, `*_env_depth`, `mod_wheel_*`, `vel_cutoff_depth`,
  `filter_key_track`, `pitch_wheel_depth`). Their calibration is not portable —
  a matrix slot depth is one bipolar `[-1, 1]` param shared by every dest — and
  the equivalent lives in the per-dest cook/gain (`DestId::cook_depth`'s cubic
  Pitch taper, `DEST_GAIN`). Deliberate, documented, out of scope here.
- **JS side.** Neither synth applies taper in the browser: `makeFader` /
  `makeDial` send a raw position via `set_param_norm` and paint from the echoed
  norm. `util/drag.js` is byte-identical apart from VXN1b's `paintFader`
  hidden-container guard. The tuned-cutoff override (which bypasses the taper by
  design) is identical in both. So the fix belongs entirely in the Rust
  accessors — no JS change.
- **Blast radius.** `get_normalized`/`set_normalized` are reached only from the
  editor path (`ViewEvent::ParamChanged` and `UiEvent::SetParamNorm`). CLAP
  exchanges plain values against the descriptor range; presets and `clap.state`
  store plain. Host automation and persistence are unaffected.

## Acceptance criteria

- [x] `SharedParams::{get_normalized, set_normalized}` use `to_fader`/`from_fader`.
- [x] Cutoff at half travel reads 800 Hz; an octave at the bottom of the fader
      gets comparable travel to an octave at the top.
- [x] Position → value → position round-trips across the travel.
- [x] Linear params are unchanged (`to_fader` falls through to the linear map).
- [x] A permanent gate (`tests/taper_parity.rs`) compares VXN1's and VXN1b's
      float calibrations by name, so a future table edit can't drift silently.

## Notes

- The dials (Dynamics) share the same write path, so they gain the taper too —
  Dyn Attack `Exp{10}` over 0.1–200 ms was the same crush as cutoff.
- Matrix depth faders are `Taper::Linear`, so they are bit-unchanged.
- `panels/fader.js`'s `subdivisionLabel` comment (inherited from VXN1) claims
  the echoed norm is the linear range position. It was already wrong for VXN1
  and is now wrong for VXN1b in the same way; tempo-sync rate resolution is not
  implemented in VXN1b's engine yet, so nothing reads it. Left as-is to keep the
  two files identical — fix in both when VXN1b gains tempo sync.
