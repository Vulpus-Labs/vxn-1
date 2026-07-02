---
id: "0166"
product: vxn-1
title: Extract vxn-1 engine/app shared test apparatus — synth builders, preset loop, PresetStore
priority: medium
created: 2026-07-01
epic: E031
---

## Summary

vxn-1's engine and app test modules carry the same shapes multiple times:
four near-identical "clean single sine" synth builders, a preset round-trip
loop rolled by hand four times, an inline TOML header pasted across four
preset tests, three overlapping `PresetStore` impls (~250 lines), and a
"names from drained events" pattern copied five times. Extract shared
helpers so each test reads as its scenario, not its scaffolding.

Line numbers are as-reviewed on 2026-07-01; re-grep by name.

## Acceptance criteria

`vxn-engine/src/lib.rs`:

- [x] Collapse the four builders `pitched_synth` (~1341), `osc2_sine_synth`
      (~1611), `glide_synth` (~2658), `deterministic` (~2090) — all "single
      sine, osc2 muted, vibrato killed, chorus off, fast attack" — into one
      `clean_sine_synth()` base plus small mutators.
- [x] Add `assert_fx_off_is_dry_pass(&[(GlobalParam, f32)])` and use it for
      the phaser/reverb bypass pair (`phaser_off_passes_dry_unchanged` ~2779,
      `reverb_off_passes_dry_unchanged` ~2832).
- [x] Add `advance_ch0_lfo(rate, free_run)` for the three `per_voice_lfo1_*`
      tests (~1793/1817/1836).

`vxn-engine/src/preset.rs`:

- [x] Add `assert_all_params_match(back, expected)` and route the four
      round-trip loops through it + the existing `dense_state()` fixture
      (~340/374/634/675), retiring the hand-rolled per-param loops.
- [x] Add `fn upper_preset(body: &str) -> String` wrapping the shared
      schema/`[meta]`/`[performance]` header for the four inline-TOML tests
      (~432/451/471/489).

`vxn-app/tests/controller.rs`:

- [x] Consolidate the three `PresetStore` impls — `MockPresetStore` (~172),
      `TempPresetStore` (~254), `MixedStore` (~891) — into one configurable
      disk-backed store (MixedStore and TempPresetStore already duplicate
      `user_load`/`list_user_tree`). Hoist to module scope.
- [x] Add `fn loaded_names(rx) -> Vec<String>` for the `drain(&view_rx)...
      filter_map(PresetLoaded => name)` pattern copied ~5× (~842/857/872/
      948/957).

`vxn-clap/tests/host_smoke.rs`:

- [x] Add `fn load_entry()` for the repeated
      `unsafe { PluginEntry::load_from_raw(&VXN_ENTRY, c"...") }.unwrap()`
      (~31/41).

- [x] `cargo test -p vxn-engine -p vxn-app -p vxn-clap` green; assertions
      unchanged.

## Notes

The `MixedStore` scenario itself (forward-step crosses factory→user) is a
clarity finding tracked in 0168 — but the impl consolidation lives here
since it's the same three-impl duplication. Do them together if working
this file. Preset round-trip *deletions* (engine↔engine ~340/374 subsumed
by byte-parity) are 0162; here only the shared-loop extraction.

## Close-out (2026-07-02)

Committed as `39ee0fc`.

- `vxn-engine/src/lib.rs`: `clean_sine_synth` base (pitched/osc2_sine/glide/
  deterministic all delegate); `assert_fx_off_is_dry_pass` (phaser+reverb
  bypass); `advance_ch0_lfo` (2 of 3 per_voice_lfo1 tests — the shape-loop
  one kept inline, needs a fresh synth per shape).
- `vxn-engine/src/preset.rs`: `assert_all_params_match` +  `upper_preset`
  header wrapper (4 inline-TOML tests). Routed whatever round-trip tests
  survived 0162's deletions.
- `vxn-app/tests/controller.rs`: 3 PresetStore impls → one `TestPresetStore`
  (`memory`/`with_factory`/`disk` ctors), ~−180 lines; `loaded_names` (5
  sites); `step_preset_spans_factory_into_user` now reads clearly (also
  satisfies 0168's clarity note for that test).
- `vxn-clap/tests/host_smoke.rs`: `load_entry` helper.

Pure refactor; `cargo test -p vxn-engine -p vxn-app -p vxn-clap` green.
