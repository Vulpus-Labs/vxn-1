---
id: "0103"
title: PatchParam — add Spread
priority: medium
created: 2026-06-07
epic: E019
---

## Summary

Append one new patch param for the per-voice stereo spread feature:

- `Spread` — 0..1 normalised, default 0.0.

Belongs to the Voice block conceptually (alongside AssignMode,
Detune, Glide). Default 0 preserves bit-identity with all existing
presets — see epic E019 for the rationale (no separate Mono mode
because StereoPhaser/StereoChorus already hold dual-chain state
internally; spread=0 IS mono).

## Acceptance criteria

- [ ] `crates/vxn-app/src/params.rs`: `PatchParam::Spread` appended
      after `LayerLevel`. `PATCH_PARAMS` desc table extended with
      name, range, default.
- [ ] `PatchParam::COUNT` recompiles cleanly.
- [ ] CLAP id layout still computes via the patch-id helper — no
      static id assumptions to break (per
      [[vxn1-id-stability-dropped]]).
- [ ] `set_param` / `param_value` plumbing in `vxn-engine` updated
      to read/write the new field. Stored on the patch state but
      consumed by the engine voice loop in 0104.
- [ ] Preset round-trip:
      - Loading a preset without a `spread` key: defaults to 0.0.
        Existing presets unchanged.
      - Loading a preset with the key: round-trip preserves value.
      - Saving emits the key.
- [ ] `cargo test -p vxn-engine -p vxn-app` green.

## Notes

Preset format is name-keyed per [[vxn1-preset-system]] / ADR 0005 —
default-fill handles the migration automatically.

Param table can be re-ordered to group Voice-block params
(AssignMode, Legato, UnisonDetune, PortamentoTime, Spread) if
readability calls for it.

The actual stereo routing logic, pan-coefficient derivation, and
voice-sum branch all live in 0104 — this ticket is purely the
param-table addition.
