---
id: "0076"
product: vxn-2
title: "Filter drive as a mod-matrix destination"
priority: medium
created: 2026-06-20
epic: null
depends: []
---

## Summary

The OTA filter's `drive` ([params.rs:555](../../vxn-2/crates/vxn2-engine/src/params.rs#L555),
range `[0.1, 16]`, default 1.0) was the only filter control that could not be
modulated — cutoff and resonance are already per-stack matrix dests
([matrix.rs](../../vxn-2/crates/vxn2-engine/src/matrix.rs)), drive was read
straight from `fp.drive`. This adds `DestId::FilterDrive` so drive can be a
matrix target like cutoff/resonance. Part of the filter epic (ADR 0004; tracked
as "E007" in DSP comments — note that id collides with the closed vxn-1
worklist epic of the same number, see Notes).

## Design

- New `DestId::FilterDrive` **appended** (discriminant 42) after the op-phase
  block so the blob dest space stays a 1:1 prefix for older patches.
- Tier `PerStack` (one scalar per voice, collapses to lane 0) — same as
  cutoff/resonance; added to the `VoiceIdx` degenerate-coherence special case.
- Modulated in the **log/octave domain**, `DEST_GAIN = 4.0`: consumer applies
  `drive · 2^value` then clamps to `[0.1, 16]`. Matches the drive param's own
  exponential taper around 1.0 (full depth ±4 oct = ×16 / ÷16), consistent with
  the cutoff idiom. Applied in
  [engine.rs `set_stack_filter_coeffs`](../../vxn-2/crates/vxn2-engine/src/engine.rs#L1305)
  via a new `FILTER_DRIVE_IDX` accumulator column.
- JS faceplate picks it up automatically — the dest dropdown is built from
  `build_matrix_lists_json`, no JS change.

## Acceptance criteria

- [x] `DestId::FilterDrive` wired end-to-end: enum, `N_DESTS`, `DEST_NAMES`,
      `DEST_LABELS`, `DEST_GAIN`, `tier()`, `from_u8`, coherence
- [x] Engine reads `dest_vals[i][0][FILTER_DRIVE_IDX]` and applies it
      log-domain before `OtaLadderCoeffs::new`
- [x] Matrix round-trip + coherence tests cover FilterDrive (disc 42, PerStack,
      `VoiceIdx` degenerate)
- [x] `cargo test -p vxn2-engine -p vxn2-ui-web` green; `cargo check --workspace`
      clean
- [ ] Manual DAW check: route an EG/LFO to Filter Drive, confirm audible
      drive sweep (per [[verify-audio-in-reaper]])

## Notes

- **Stale test fixed as a side effect:** `build_matrix_lists_json_includes_all_enum_widths`
  ([vxn2-ui-web/src/lib.rs](../../vxn-2/crates/vxn2-ui-web/src/lib.rs#L823))
  asserted 36 dests but the real count was already 42 — the E023 op-phase block
  never updated it, so the test was failing before this change. Bumped to 43 and
  added phase/filter-drive index asserts.
- **Epic-id collision:** "E007" in vxn-2 DSP comments/memory ([[vxn2-filter-epic]])
  means the *filter* epic (ADR 0004). The unified worklist's `epics/closed/E007`
  is the unrelated vxn-1 `faceplate-js-cleanup`. The vxn-2 008x filter tickets
  predate this worklist and have no files here, so `epic: null`.

## Close-out (2026-06-22)

- `DestId::FilterDrive` (discriminant 42) wired end-to-end: enum,
  `N_DESTS`, `DEST_NAMES`/`DEST_LABELS`/`DEST_GAIN` (4.0, log/octave
  domain), `tier() = PerStack`, `from_u8`, `VoiceIdx` degenerate
  coherence. Engine applies `drive · 2^value` clamped `[0.1,16]` via a
  new `FILTER_DRIVE_IDX` accumulator in `set_stack_filter_coeffs`
  ([engine.rs:1305](../../vxn-2/crates/vxn2-engine/src/engine.rs#L1305)).
  JS dest dropdown picks it up from `build_matrix_lists_json` (no JS
  change).
- Tests: matrix round-trip + coherence cover FilterDrive;
  `build_matrix_lists_json_includes_all_enum_widths` bumped to 43
  (also fixed the pre-existing stale 36-count assert). `cargo test
  -p vxn2-engine -p vxn2-ui-web` green. Manual DAW drive-sweep check
  waived at close.
