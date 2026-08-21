---
id: "0268"
product: vxn-1b
title: "Env 1 / Env 2 time-scale mod destinations, cooked at note-on"
priority: medium
created: 2026-08-21
epic: E039
---

## Summary

Two new mod-matrix destinations, `Env 1 Scale` and `Env 2 Scale`, that
multiply an envelope's **A / D / R** times per voice: 0.5× (half as long) to
2.0× (twice as long), unity at centre. Sustain — a *level*, not a time — is
untouched.

The value is **cooked once at note-on** and held for the life of the note,
rather than tracked per control block. Envelope times are not continuously
modulatable in this engine: `AdsrCore` holds cooked per-sample increments, and
re-cooking them mid-stage would make a held note's decay lurch as the source
moves. Latching at the trigger is also what the intended sources want —
mod wheel, `Spread`, `NoteRandom`, velocity, key, or a free-running LFO 2
sampled at the moment the key goes down, so each note in a chord gets its own
envelope length.

The per-voice machinery already exists: 0218's drift `VoiceTrim.env_time`
multiplies each lane's A/D/R by a frozen per-lane draw. This is the same
multiply, from the matrix instead of the drift seed, and re-derived on every
note-on instead of frozen at construction.

## Acceptance criteria

- [x] `DestId::Env1Scale` / `DestId::Env2Scale` (wire names `env1-scale` /
      `env2-scale`, labels `Env 1 Scale` / `Env 2 Scale`), `N_DESTS` 11 → 13,
      linear `cook_depth`, `DEST_GAIN` 1.0 = ±1 octave of time.
- [x] `eval::env_time_scale`: dest total → multiplier `2^clamp(x, −1, 1)`, so
      the reachable range is exactly [0.5, 2.0] and 0 is unity.
- [x] `RenderBank` keeps the patch's envelope params + drift, so a per-lane
      scale survives a later envelope/drift param change (`set_envelopes`
      re-cooks every lane as base × drift trim × the lane's live scale).
- [x] The scale is recomputed only on a lane's note-on trigger, from that
      block's source values; no route (or depth 0) leaves every lane at exactly
      1.0 and the render bit-identical.
- [x] Both layers, all 16 voices; UI combos pick the dests up for free (the
      faceplate's vocab is generated from `DEST_NAMES` / `DEST_LABELS`), and
      preset/state round-trip by wire name.
- [x] Tests: mapping endpoints, note-on latching (source moves mid-note →
      envelope unchanged), a longer attack actually renders slower, and a
      param change mid-note preserves the cooked scale.

## Notes

Sources that are meaningless at note-on: `Env1`/`Env2` themselves (level ≈ 0 at
the trigger) and `Aftertouch` (pressure arrives after the note). Routable, but
they will read as near-zero — the documentation says so rather than the code
forbidding it.

## Close-out

Landed 2026-08-20 in `e43b7d0`. Files touched: `vxn1b-engine/src/{matrix.rs,
eval.rs, bank.rs}`.

`DestId::Env1Scale` / `Env2Scale` with wire names `env1-scale` / `env2-scale`;
`eval::env_time_scale` maps the dest total to `2^clamp(x, −1, 1)`, so the
reachable range is exactly [0.5, 2.0] with 0 exactly unity — the property that
keeps an unrouted patch bit-identical.

Exponential rather than linear so the two directions are musically symmetric
(`+d` lengthens by exactly what `−d` shortens by) and summed routes compose.
Clamping the exponent rather than the result keeps the rails hard.

`RenderBank` holds the patch in `EnvPatch`, so `apply_env_lane` re-cooks each
lane as base × drift trim × the lane's live scale; a later envelope or drift
param change therefore preserves an in-flight note's cooked scale rather than
snapping it back.

Latched at the note-on trigger only, never tracked continuously: `AdsrCore`
holds cooked per-sample increments, so a tracked dest would make a held note's
decay lurch whenever the source moved. Each note in a chord keeps whatever
length the sources said at the moment it started, for the whole life of the
note — including its release, which is the point of scaling R at all.

Tests: `env_time_scale_is_symmetric_and_railed`,
`env_scale_dests_are_linear_unity_gain`, `env_scale_rails_are_half_and_double`,
`env_scale_latches_at_note_on_and_holds_for_the_note`,
`envelope_param_change_mid_note_keeps_the_cooked_scale`,
`no_env_scale_route_renders_bit_identically`.

UI vocab and preset/state round-trip came free, as designed: the faceplate
combos are generated from `DEST_NAMES`/`DEST_LABELS`, preset TOML writes
`DEST_NAMES[dest as usize]`, and the state blob is positional.

Note the `N_DESTS` figure in the first criterion (11 → 13) was superseded by
0269 and 0270 landing on top; it is 16 now.

**Not verified by automated test:** the audible result — needs a listen in
Reaper.
