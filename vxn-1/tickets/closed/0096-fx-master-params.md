---
id: "0096"
title: Param table — drop reverb_type/depth, add phaser + FDN + drift
priority: medium
created: 2026-06-06
epic: E018
---

## Summary

Rewrite the `GlobalParam` table for the new FX/master surface.

**Drop**:
- `ReverbType`
- `ReverbDepth`

**Add**:
- `ReverbSize` — 0..1 normalised (→ 0.2..2.0 size scale via
  `MIN_SIZE_SCALE`/`MAX_SIZE_SCALE`), default 0.5
- `ReverbDecay` — RT60 seconds, 0.2..10.0, default 2.5
- `ReverbDamp` — 0..1 normalised, default 0.4
- `MasterDrift` — 0..1 normalised, default 0.0
- `PhaserOn` — bool, default 0
- `PhaserRate` — 0.05..10.0 Hz, default 0.5
- `PhaserDepth` — 0..1, default 0.7
- `PhaserFB` — -0.9..0.9, default 0.0
- `PhaserMix` — 0..1, default 0.5

**Keep**: `ReverbOn`, `ReverbMix`, all chorus / delay params,
master tune/volume, limiter, oversample, LFO2.

Per [[vxn1-id-stability-dropped]], re-order freely — group by
section (Master, LFO2, Phaser, Chorus, Delay, Reverb) for
readability.

## Acceptance criteria

- [ ] `crates/vxn-app/src/params.rs`: `GlobalParam` enum updated.
      `GLOBAL_PARAMS` descriptor table updated with name, range,
      default, and any enum-label lists.
- [ ] `GlobalParam::COUNT` recompiles cleanly (compile-time
      derived from the enum's last variant).
- [ ] CLAP id layout still computes via `global_clap_id(g)` —
      no static id assumptions to break.
- [ ] `crates/vxn-engine/src/lib.rs` `set_param` arms updated:
      drop two arms, add nine.
- [ ] `Synth` / `Engine` struct gains storage for the new param
      values (smoothers as needed in 0097; this ticket just lays
      the param table + plumbing).
- [ ] Preset round-trip: loading a TOML with
      `reverb_type = "Hall"` or `reverb_depth = 0.5` logs a
      warning and ignores the unknown key (or silently — match
      existing unknown-key behaviour). Saving omits removed
      keys.
- [ ] `cargo test -p vxn-engine -p vxn-app` green.

## Notes

The preset loader is name-keyed (ADR 0005 / [[vxn1-preset-system]]) so
removing `reverb_type` is safe — old presets just lose that
field on load. New fields default-fill. No migration code
needed beyond a one-line audit of the loader to confirm
unknown-key handling is graceful.

If the loader currently errors on unknown keys, change it to
warn-and-skip in this ticket (one-line policy change). If
already warn-and-skip, no change.

Drift range mapping: store the param as 0..1 and use it
directly as `Engine::drift_amount` in 0097 — the existing
`DEFAULT_DRIFT_AMOUNT` constant becomes documentation only (or
delete it).

The phaser FB range is signed because the upstream allpass loop
accepts ±0.9 — negative feedback flips notch parity.

Reverb defaults aim at a tasteful out-of-box voicing: medium
size (0.5), 2.5 s tail (medium room), mild damp (0.4), mix
0.3. Re-tune in 0099 if the factory audit disagrees.
