---
id: "0221"
product: vxn-1b
title: "Two-layer state + preset format — versioned, with single-patch migration"
priority: high
created: 2026-07-31
epic: E039
depends: ["0214", "0216"]
---

## Summary

Extend VXN1b's host-state blob and preset TOML to carry **two layers +
KeyMode/split**, versioned, with migration for any single-patch presets already
saved. Per [ADR 0002](../../vxn-1b/adrs/0002-vxn1b-dual-layer.md) §4 "Consequences."

## Design

- **Host state (`PluginState`)**: carry two `Synth` patches + two matrix topology
  records + KeyMode + split point + LFO2-sync flag + global (drift/FX/mixer).
  Bump the state version tag; add a decode path that reads the old single-patch
  layout and lifts it into Layer 1 with Layer 2 off (= Single).
- **Preset TOML**: name-keyed sparse format extended with a `[layer2]` section
  (and layer-scoped matrix), plus keymode/split/global. A preset with no
  `[layer2]` loads as single-patch (Layer 2 off) — so pre-existing presets remain
  valid without rewrite.
- **Factory bank**: the E038/0212 bank rebases onto this format (single-patch
  presets stay valid via the no-`[layer2]` default; add a couple of split/dual
  demo presets when convenient — not required to close this ticket).
- Mind `include_dir` no-rerun ([[vxn2-include-dir-no-rerun]]) when touching
  factory TOMLs.

## Acceptance

- Host state + preset TOML round-trip two full layers + KeyMode/split/LFO2-sync.
- **Migration**: a pre-change single-patch VXN1b preset / saved host state loads
  as Layer 1 + Layer 2 off, bit-identical to its old sound.
- A preset with no `[layer2]` section loads as single-patch (no error).
- Round-trip tests: dual and split presets survive save → reload; legacy preset
  migrates. `cargo test -p vxn1b-engine` green.

## Close-out (2026-08-08)

- **Host state.** `PluginState` gains `key: KeyState`, written as a 4-byte
  record *after* the two layer blocks so their v5 offsets are unchanged;
  `VERSION` 5 → 6 ([state.rs:54](../../vxn-1b/crates/vxn1b-engine/src/state.rs#L54),
  [state.rs:151](../../vxn-1b/crates/vxn1b-engine/src/state.rs#L151)). Before
  this, KeyMode/split/LFO2-link had no persistence at all — they lived only in
  `SharedParams`, so a split patch reloaded as Single.
  Tests `state::tests::round_trips_key_state`,
  `state::tests::factory_state_is_single_mode`,
  `state::tests::missing_key_record_is_an_error`,
  `state::tests::blob_length_is_two_full_layers_plus_the_key_record`.
- **Engine.** `Engine::load_state` applies `state.key`, so a state load is a
  complete restore rather than depending on the shell's separate key channel
  ([engine.rs:319](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L319)). Test
  `engine::tests::load_state_restores_the_keyboard_record`.
- **Store.** `SharedParams::to_state` snapshots the key channel;
  `restore_from_bytes` writes it back and raises `key_dirty` — a separate wire
  from `reload`, which alone would not carry it. Added a non-consuming
  `key_state()` peek so the main-thread echo can't swallow the audio thread's
  re-sync ([shared.rs:221](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L221)).
  Test `shared::tests::snapshot_restore_carries_key_state_and_flags_the_key_channel`.
- **Preset TOML.** Codec now maps a whole `PluginState`. Layer 1 stays at the
  top level; Layer 2 lands in optional `[layer2.params]` / `[[layer2.matrix]]`
  and the keyboard in an optional `[keys]` (`mode` / `split-point` /
  `lfo2-link`) ([preset.rs:67](../../vxn-1b/crates/vxn1b-engine/src/preset.rs#L67)).
  `read_preset` is now the single parse entry point (`from_toml_str` dropped).
  Tests `preset::tests::both_layers_and_keys_round_trip`,
  `preset::tests::dual_mode_without_split_round_trips`,
  `preset::tests::layer2_section_is_sparse_too`,
  `preset::tests::layer2_warnings_name_their_layer`,
  `preset::tests::partial_keys_section_defaults_the_rest`,
  `preset::tests::unknown_key_mode_warns_and_falls_back_to_single`.
- **Migration (presets).** Both new sections are optional on read and omitted on
  write when at the factory default, so every pre-0221 file loads as Layer 1 as
  written + factory Layer 2 + `Single`, and re-saves as the same text. Tests
  `preset::tests::legacy_single_layer_preset_loads_as_layer1_plus_factory_layer2`,
  `preset::tests::keys_without_layer2_is_valid`,
  `preset::tests::single_layer_patch_writes_no_layer2_or_keys`.
- **Deviation — host-state migration.** The v5 blob is *rejected*, not lifted.
  This ticket predates 0216, which had already established reject-old for the
  binary format (the layer block is positional, so an old blob read at a newer
  length slides topology bytes into param slots rather than failing cleanly) —
  VXN1b is pre-release, so no saved sessions are owed a migration. The migration
  that matters, and that the name-keyed sparse text format can actually deliver,
  is the preset one above.
- **Deviation — derived KeyMode in the file.** `[keys]` stores `mode`, not
  `KeyState`'s two booleans, so a split *armed while Layer 2 is off* normalises
  to `Single` on preset save. The host-state blob keeps both toggles verbatim,
  so nothing is lost across a DAW save.
- **Editor echo (beyond the acceptance list).** `KeyState` is not a CLAP param,
  so a preset / `state.load` / undo moved it with nothing telling an open
  faceplate — the same gap 0247 fixed for topology. Added `push_key_echo`
  ([clap/lib.rs:331](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L331)), a
  `kind: "keys"` payload
  ([ui-web/lib.rs:141](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L141)), and
  the dispatch handler that decomposes the mode back into the two toggles. The
  JS reflectors (`model.setLayer2On` / `setSplitEnabled` / `setSplitPoint` /
  `setLfo2Link`) already existed with no producer. Vitest
  `dispatch-orchestration`: "decomposes a keys echo into the layer-2 / split /
  link reflectors", "turns both toggles off on a keys echo back to Single".
- **Factory bank.** Not rebased — 0212 is still open and the ticket makes that
  optional. Single-layer factory presets stay valid via the no-`[layer2]`
  default.
- `cargo test -p vxn1b-engine` green (167 lib + 25 integration); vxn1b-clap 7,
  vxn1b-ui-web 9; `npx vitest run` 235 across 29 files.
- **Not verified in a DAW.** Closed on the user's call before the Reaper
  listening check (save split patch → reload → routing + faceplate).
