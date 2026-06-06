---
id: "0097"
title: Engine bus — phaser pre-chorus, FDN reverb, master drift
priority: medium
created: 2026-06-06
epic: E018
---

## Summary

Wire the new DSP + params into the engine's per-block FX bus.

Canonical chain becomes:
```text
dry → phaser → chorus → delay → reverb → limiter → out
```

Phaser slots between dry and chorus (pre-time-effects so its
resonant peaks survive the chorus's chorale). Reverb swaps from
`StereoVReverb` to `FdnReverb`. Drift wires from
`GlobalParam::MasterDrift` into the existing per-voice
`drift_amount` plumb.

## Acceptance criteria

- [ ] `Synth` (or wherever the FX chain lives —
      `crates/vxn-engine/src/lib.rs`): `reverb` field type
      changes from `StereoVReverb` to `FdnReverb`.
- [ ] `Synth.phaser: StereoPhaser` added, instantiated in `new`,
      reset in `reset`.
- [ ] Per-block FX dispatch (the `update_effects` /
      `process_block` loop): phaser tick inserted before
      chorus; reverb tick called with the new `(size, decay,
      damp, mix)` param shape.
- [ ] Param smoothers added for the new phaser params
      (rate/depth/fb/mix) and reverb params (size/decay/damp/mix)
      with the existing crate idiom — match `chorus.rs` set
      pattern. Reverb size's 500 ms internal smoother does the
      heavy lifting; engine-side just feeds set_params.
- [ ] `MasterDrift` param routes into `Engine::drift_amount`
      directly. Per-voice salt seeding unchanged.
- [ ] `DEFAULT_DRIFT_AMOUNT` either becomes documentation
      (constant kept, no longer the live default) or is deleted
      — the live default is now `MasterDrift`'s declared
      default (0.0 per 0096).
- [ ] Bypass semantics: with all `*_on` switches off and
      `phaser_on = 0`, the bus is bit-identical to dry input.
      Add a unit test if one doesn't exist.
- [ ] Old reverb voicing macro helper (`reverb_macro(type,
      depth)`) deleted — no caller after the param swap.
- [ ] `cargo test --workspace` green.
- [ ] `cargo build -p vxn-clap --release` succeeds.

## Notes

Per-block control-rate: phaser's Rate is host-rate but cheap to
smooth at control rate; lerp inside `set_params` is fine.
Phaser FB at the signed range can ring — clamp internally to
±0.9 at the smoother output and you're safe.

Reverb's `size` smoother is intentionally inside the DSP (vxn2
ADR §7); don't double-smooth at the engine layer.

Limiter stays last. Oversample is orthogonal to FX placement.

Drift wiring is a single line in `set_param`:
```rust
GlobalParam::MasterDrift => self.engine.drift_amount = value,
```
…or equivalent — match the existing arm style.

If the baseline tripwire test (`vxn-1/crates/vxn-engine/tests/baseline.rs`,
modified per git status) hashes against the dry bus,
`reverb_on = 0` and `phaser_on = 0` must keep it stable. If the
hash is FX-inclusive, regenerate it as part of this ticket and
note the rationale in the commit.
