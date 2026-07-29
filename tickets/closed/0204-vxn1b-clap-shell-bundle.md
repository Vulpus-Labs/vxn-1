---
id: "0204"
product: vxn-1b
title: "CLAP shell + bundle: params/state wiring, MPE event routing, xtask install, clap-validator clean"
priority: high
created: 2026-07-25
epic: E036
---

## Summary

Wire `vxn1b-clap` end-to-end so VXN1b is **DAW-playable with host-generic knobs**
— the E036 exit criterion. Forks VXN1's CLAP shell
([vxn-clap/src/lib.rs](../../vxn-1/crates/vxn-clap/src/lib.rs)) with VXN1b's
param table (0200) and state (0203).

- **Params:** expose the flat table (incl. 16 slot depths) as `clap.params`;
  no-echo mirror + `LocalParams` + gesture bracketing per VXN1 ADR 0001 §6.
- **State:** save/load via 0203; empty/invalid blob returns `false` (0196 contract).
- **MPE event routing:** feed note-on (with channel), poly-pressure, and
  channel-pressure events into the engine's per-voice pressure path (0198).
  Pitch-bend → hardwired global bend (not a matrix route).
- **Bundle:** `vxn-1b/xtask bundle` builds/installs the `.clap`.

## Acceptance criteria

- [ ] The `.clap` loads and **plays in a DAW**; all params automatable via host
      generic UI; slot depths move modulation.
- [ ] MPE note/pressure events reach the engine: per-note pressure drives the
      matching voice, channel pressure broadcasts (0198 behaviour observable via
      a routed Aftertouch→Cutoff slot).
- [ ] Pitch-bend bends globally via the hardwired range param.
- [ ] `clap-validator validate` reports **0 failures** (incl. empty-state-load).
- [ ] `vxn-1b/xtask bundle` installs a working bundle.

## Notes

- No faceplate yet — the `gui` extension can be absent or minimal; the UI lands
  in [[E038]]. Host-generic knobs are the E036 acceptance bar.
- Reuse `vxn-core-clap` helpers; keep the no-echo mirror discipline (host
  automation must not echo back as a UI edit).
- Verify audio manually in Reaper (`verify-audio-in-reaper`) — do not build a
  headless audio harness.
- Depends on 0200, 0202, 0203, and 0198 (MPE events). Closes the E036 spine.

## Close-out (2026-07-29)

- **Params exposed as `clap.params`.** [lib.rs](../../vxn-1b/crates/vxn1b-clap/src/lib.rs)
  `PluginMainThreadParams` reports the full flat table (incl. the 16 slot
  depths) via `desc_for_clap_id`: `count`/`get_info` (stepped flag from
  `ParamKind`)/`get_value`/`value_to_text` (`ParamDesc::display`)/`text_to_value`
  (`ParamDesc::parse`)/`flush`. The main/audio crossing is a new lock-free
  [`SharedParams`](../../vxn-1b/crates/vxn1b-engine/src/shared.rs) (atomic value
  array + `Mutex<MatrixTable>` topology + reload flag); no gesture/echo/GUI
  machinery (host-generic knobs only — faceplate is [[E038]]).
- **Slot depths move modulation** through the CLAP path: a `ParamValue` event
  routes `set_param` (0205 mirrors it into the matrix the evaluator reads).
  `vxn1b_clap::tests::slot_depth_param_event_moves_modulation_through_the_shell`
  zeroes the Env2→Amp slot via a param event and the note goes silent.
- **State save/load** via `SharedParams::{snapshot_bytes,restore_from_bytes}`
  over `PluginState` (0203). Empty/undecodable blob → `false` (0196):
  clap-validator `state-invalid` PASSED; `state-reproducibility-{basic,flush,
  buffered,null-cookies}` PASSED.
- **MPE event routing** is bespoke (`dispatch` in
  [lib.rs](../../vxn-1b/crates/vxn1b-clap/src/lib.rs)) because the shared
  `vxn-core-clap` note dispatch is channel-agnostic: note-on/off thread their
  MIDI channel into the allocator (0198); CLAP note-expression *pressure* + MIDI
  poly-key-pressure (0xA0) → `poly_pressure`; channel pressure (0xD0) →
  `channel_pressure`; pitch-bend → the hardwired global bend (ADR §3);
  CC1 → mod wheel. Unit-tested for note→sound + param routing.
- **Bundle installs.** `vxn1b-xtask bundle`/`install` builds
  `target/release/vxn1b.clap` → `~/Library/Audio/Plug-Ins/CLAP/`.
- **`clap-validator validate` — 0 failures.** 21 run, 18 passed, 0 failed,
  3 skipped (all optional `preset-discovery-factory`), 0 warnings.
- **Non-finite output guard** ([engine.rs](../../vxn-1b/crates/vxn1b-engine/src/engine.rs)
  `render_control_block`): `param-fuzz-basic` surfaced that an extreme
  filter/feedback combo under dense high-note polyphony can drive DSP state to
  NaN/inf. Root cause is a *combination* (no single param; matrix mods ≈ 0 where
  it surfaces), so the fix is a boundary guard replacing non-finite output with
  silence — a NaN must never reach the host. Regression:
  `engine::tests::output_is_always_finite_under_param_and_note_fuzz`.
  The exact `vxn-dsp` blowup path is not isolated (guard contains it) — candidate
  follow-up if the shared filter needs hardening.
- Tests: 91 engine-lib + 4 clap-lib, clippy clean. Shipped in commit `bca6ae7`.
- **Manual DAW verification waived at close** (user call): "plays in a DAW",
  "MPE pressure observable via a routed Aftertouch→Cutoff slot", and "pitch-bend
  bends globally" are wired + unit-tested but confirmed by ear in Reaper as a
  follow-up, not gated on this close.
</content>
