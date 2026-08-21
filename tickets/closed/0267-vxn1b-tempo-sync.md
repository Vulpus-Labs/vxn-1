---
id: "0267"
product: vxn-1b
title: "Tempo sync: make the LFO Sync toggles work, add Delay Sync"
priority: medium
created: 2026-08-20
epic: E039
---

## Summary

VXN1b shipped the `lfo1_sync` / `lfo2_sync` **params** and their faceplate
toggles, but never the feature behind them. Flipping Sync changed nothing
audible (the engine read `Lfo1Rate` / `Lfo2Rate` as literal Hz regardless), and
the rate fader's popup showed an **empty label** instead of a subdivision —
`vxn1b_ui_web::build_subdivisions_json` was a stub returning `[]`, so
`window.vxn.subdivisions` was empty and `subdivisionLabel()` had nothing to
index. The plugin also never read the host transport, so there was no tempo to
sync to. Delay had no Sync toggle at all.

Port VXN1's tempo-sync feature (`vxn_app::sync`, E004 / 0015) onto VXN1b's
two-layer CLAP map, and extend it to the delay.

## Acceptance criteria

- [x] New `vxn1b_engine::sync` module over the shared
      `vxn_core_utils::sync` subdivision table: rate↔sync CLAP-id partners for
      per-layer LFO 1 / LFO 2 and the global delay, `lfo_rate_hz` /
      `delay_time_seconds` resolvers, and `sync_aware_display`.
- [x] New global param `delay_sync`, plus its faceplate toggle in the Delay
      panel's strip and the `delay_time ↔ delay_sync` pair in `dispatch.js`.
- [x] Synced LFO rate = the fader position's subdivision at the host tempo;
      synced delay time = that subdivision's **period**.
- [x] `DELAY_MAX_SECONDS` 2 s → 4 s, and the synced time clamps to the *line's*
      capacity rather than the Time knob's 2 s ceiling. A subdivision period
      legitimately runs past the knob (`1/1` is 4 s at 60 BPM); sizing the line
      to the knob would have shown `1/1` while the ear heard 2 s. Costs ~768 KB
      per instance at 48 kHz.
- [x] `Engine::set_tempo` / `on_transport_restart`, driven from the CLAP
      `process` transport. Non-finite / non-positive BPM ignored; the default is
      `DEFAULT_TEMPO_BPM` (120) so a tempo-less host still runs.
- [x] `value_to_text` and the editor's `ParamChanged` broadcast both read
      through `sync_aware_display`, so a synced param reads out as e.g. `1/4T`
      in the DAW **and** in the faceplate popup. A sync toggle that flips
      re-pushes its rate partner so the label switches without a value change.
- [x] `build_subdivisions_json` splices the real table.
- [x] State `VERSION` 8 → 9 (`DelaySync` lengthens the positional param block).
- [x] Tests: engine `sync` unit tests (partner round-trip, tempo tracking,
      delay clamp), JS pairing test extended to the delay pair. Workspace
      `cargo check --all-targets` clean; 259 engine + 274 JS tests pass.

## Notes

Subdivision index comes from `desc.to_fader(value)` on both sides —
`SharedParams::get_normalized` **is** `to_fader`, so the JS `norm` in a
`ParamChanged` and the engine's own resolution pick the same entry. Do not
"simplify" either side to a linear `to_normalized`.

Existing presets are unaffected: the sparse TOML is name-keyed, so `delay_sync`
is simply absent and defaults off.

## Close-out

Landed 2026-08-20. Files touched: `vxn1b-engine/src/{sync.rs (new), params.rs,
synth.rs, engine.rs, fx.rs, state.rs, lib.rs}`, `vxn1b-clap/src/lib.rs`,
`vxn1b-ui-web/src/lib.rs`, `vxn1b-ui-web/assets/{dispatch.js, faceplate.html,
fixtures/params.js, __tests__/dispatch-orchestration.test.js}`.

**Not verified by automated test:** the audible result and the Delay panel's new
strip placement — both need a listen/look in Reaper.
