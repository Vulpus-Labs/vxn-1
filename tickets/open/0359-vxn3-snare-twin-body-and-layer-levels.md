---
id: "0359"
product: vxn-3
title: "vxn-3 second tuned body for the snare (808 twin-resonator) + independent layer levels instead of crossfades"
priority: high
created: 2026-09-04
epic: E034
---

## Summary

Two problems in the same few lines of the Noise family's mix.

**One sine is not a snare body.** `Noise` sums a single tuned oscillator per lane
([noise.rs:305](../../vxn-3/crates/vxn3-engine/src/engines/noise.rs#L305)). The 808 snare's
body is **two** bridged-T resonators — roughly 180 Hz and 330 Hz — and their beating,
hollow ring is a large part of what makes the sound identifiable. A single sine gives a
thud under the noise, not a snare.

**Crossfades cap the total energy.** The mix is
`bp * (1.0 - mix) + tone_sum * mix + snap_sum`
([noise.rs:312](../../vxn-3/crates/vxn3-engine/src/engines/noise.rs#L312)) — so you cannot
have full noise *and* full body, which is exactly what a snare wants. Metal has the same
disease, nested twice ([metal.rs:338-339](../../vxn-3/crates/vxn3-engine/src/engines/metal.rs#L338-L339)),
and the `× 2.0` fudge factor at
[metal.rs:332](../../vxn-3/crates/vxn3-engine/src/engines/metal.rs#L332) is the symptom: a
level compensation hand-tuned to work around a crossfade that shouldn't have been one.

## Design

- **Second tuned body.** Add `tone_phase2` / `tone_inc2` to the per-lane SoA state (the
  arrays are `[_; LANES]`, so this is two more small arrays, no allocation change). Two
  new params:
  - **`Body2`** (`Ratio`, 1.0..4.0, default **1.0**) — the second oscillator's frequency
    as a multiple of the sequenced note. The 808 relationship is ≈ 1.8.
  - **`Body2 Level`** (`Percent`, 0.0..1.0, default **0.0**) — off by default, so every
    existing flavour is bit-for-bit unchanged.
  Both bodies share `tone_env` and `tone_decay` (one decay for the body layer is right —
  it is a single struck membrane).
- **Independent layer levels.** Replace `tone_mix`'s crossfade with explicit per-layer
  gains. Keep `P_TONE_MIX` as-is for serialisation-shape stability but *derive* the two
  gains from it such that the existing behaviour is reproduced when the new
  `Noise Level` / `Body Level` params sit at their defaults — or, if that proves fiddly,
  add the two gains and keep `Mix` as a legacy no-op with a migration note. Either way the
  acceptance criterion is unchanged output at defaults.
- **Re-author `flavour_snare()`** with the second body engaged at ≈1.8 ratio, and the two
  layers at levels that no longer fight each other. Its current base is
  `[0.07, 0.15, 0.29, 1050.0, 1.2, 0.5, 1.0, 0.012]`
  ([noise.rs:77](../../vxn-3/crates/vxn3-engine/src/engines/noise.rs#L77)).

## Acceptance criteria

- [ ] Defaults render bit-for-bit identical to the pre-ticket engine for every authored
      Noise flavour.
- [ ] A test proves the second body sounds: with `Body2 Level > 0` and `Body2 != 1.0`, the
      output contains energy at the second ratio's frequency that is absent at level 0
      (zero-crossing count or a windowed correlation against the expected sine is enough —
      match the existing tests' cheap-proxy style).
- [ ] A test proves noise and body can both be at full level simultaneously without the
      other being attenuated (peak of each layer measured in isolation, then together).
- [ ] `flavour_snare()` re-authored; `noise_flavours_are_distinct` still passes.
- [ ] Round-trip + truncated-patch tolerance updated for the new `NOISE_P`.
- [ ] `cargo test -p vxn3-engine -p vxn3-clap` green; clippy clean; alloc-trap passes.

## Notes

- Metal's nested crossfade and its `× 2.0` compensation are the same bug in another
  family; fix them in **0361** (hat source rework) rather than here, since that ticket is
  already rewriting those lines.
- Depends on nothing; adjacent to 0358 in `Noise::render` but touching different lines.
