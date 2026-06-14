---
id: E012
product: vxn-2
title: Structural-review remediation — fix inert features, harden CLAP boundary, cleanup pass
status: open
created: 2026-06-10
---

## Goal

Remediate the findings of the 2026-06-10 holistic structural review.
The review's headline pattern: plumbing built end-to-end with the last
wire not connected. Four features are structurally complete but
functionally inert (`lfo1-depth`, `AmpSens`, `PitchSmoother`, CLAP
gesture brackets), solo-mode note-off never dispatches to the solo
path, and the host sees Hz for synced params. Around those fixes, a
hardening and cleanup pass: RT-safety at `Engine::reset`, a mechanical
guard against future inert params, CI for the vxn-2 crates, and
removal of dead code/docs the review enumerated.

Design decision folded in (2026-06-10): **`lfo1-depth` is removed,
not wired.** Send-signal amplitude is determined per-route by the mod
matrix depth column; a global LFO1 depth macro is redundant with it.
The param, its fader, and its doc trail all go.

## Scope

**In:**

- Remove `lfo1-depth` from the param table, snapshot, UI, and docs;
  bump the state blob to v4 with a migration that drops the value and
  remaps the LFO1 section offsets.
- Wire `amp_sens_coef` into the per-op level-modulation path in
  `stack_tick_*` (and the scalar `op_tick` reference path).
- Instantiate `PitchSmoother` in `Engine::process_block` so
  pitch-shaped matrix destinations ramp per-sample instead of stepping
  at block rate.
- Fix `Engine::note_off` → `PolyAlloc::note_off_patch` to dispatch on
  `AssignMode` (solo fallback-to-held-note currently unreachable).
- Emit CLAP `param_gesture_begin` / `param_gesture_end` events from
  the audio thread (port the VXN-1 `vxn-core-clap` gesture pattern);
  consume the `SharedParams.gestures` bitset that the controller
  already populates.
- Route `value_to_text` through `sync_aware_display` so host
  automation lanes show subdivisions when sync is on.
- Delete the dual Model→View emission paths: the `SetParam` /
  `SetParamNorm` controller echo and the `StateLoaded`
  `broadcast_all_params` call. The dirty-bitset pump (E005) is the
  single echo path.
- RT hardening: `Engine::reset` clears `PolyAlloc` in place instead of
  constructing a new one; `SINE_TABLE` becomes a const-initialised
  `static` instead of `LazyLock`.
- Param audibility sweep: an engine integration test asserting every
  audible param changes output between min and max — the mechanical
  guard that would have caught `lfo1-depth` and `AmpSens`.
- CI job running `cargo test` over the vxn2-* crates.
- Smooth `OpNLevel` / `OpNPan` matrix destinations: linear ramp across
  the block so LFO routes to level/pan stop zippering (found by ear
  2026-06-10; same block-stepping class as the PitchSmoother fix, but
  level/pan get exact linear tracking instead of the one-pole quantum).
- DSP hygiene: dedupe the base-Hz computation (`op.rs` / `stack.rs`),
  dedupe `xorshift_step` (`stack.rs` / `lfo.rs`), annotate `voice.rs`
  as bench/reference-only and fix its stale doc comments.
- Docs/dead-code cleanup: README out of "design phase", ADR 0001
  workspace forward-note, E002 broken ticket links, dead CSS blocks,
  orphaned `MatrixRowChanged` variant + JS handler, stale
  `vxn-2/target/` directory, stale `vxn2-clap/src/lib.rs` module doc.

**Out:**

- Deferred matrix destinations (`Lfo2Phase`, `Lfo1Rate`, `Lfo2Rate`,
  `StackDetune`, `StackSpread`) — documented v1 scope, untouched.
- Tail-length / `ProcessStatus` support, NoteExpression handling —
  tracked separately when polyphonic-expression work is scheduled.
- Per-sample sub-block automation accuracy in `process()` — E002
  follow-on, unchanged.
- `eg.rs` / `envelope.rs` module merge — naming friction only; not
  worth the churn until an envelope feature touches both.
- Preset system (separate epic when scheduled).

## Tickets

| # | Ticket | Priority |
|---|--------|----------|
| 1 | 0061 — Remove lfo1-depth param | high |
| 2 | 0062 — Wire AmpSens into level modulation | high |
| 3 | 0063 — Wire PitchSmoother into process_block | high |
| 4 | 0064 — Solo note-off dispatch | high |
| 5 | 0065 — CLAP gesture begin/end emission | high |
| 6 | 0066 — Sync-aware value_to_text | medium |
| 7 | 0067 — Drop dual Model→View emission | medium |
| 8 | 0068 — RT hardening: reset + SINE_TABLE | medium |
| 9 | 0069 — Param audibility sweep test | high |
| 10 | 0070 — CI for vxn-2 crates | high |
| 11 | 0071 — DSP hygiene dedup pass | low |
| 12 | 0072 — Docs and dead-code cleanup | low |
| 13 | 0074 — Smooth level/pan matrix modulation | high |
| 14 | 0075 — CONTROL_BLOCK render slicing in the CLAP shell | high |
| 15 | 0076 — Round the level-mod clamp corner | high |
| 16 | 0077 — Combined effective-level ramp | high |
| 17 | 0078 — Multiplicative level modulation | high |
| 18 | 0079 — DX7 feedback-scale recalibration | high |

Dependency order: 0061 and 0062 land before 0069 (the sweep test
would fail against the inert params). 0067 depends on nothing but
should land after 0065 so gesture-bracket behaviour is testable
without the echo noise. Everything else is independent.

## Acceptance

- `cargo build --workspace` + `cargo test --workspace` green at HEAD.
- `cargo bench --package vxn2-osc-bench` runs to completion; no
  regression in `stack` / `master_chain` benches beyond noise (the
  AmpSens multiply and pitch smoother add work to the hot path — the
  budget is ≤ 5% on `master_chain`).
- Param count is 179; a v3 blob saved before this epic loads
  correctly with `lfo1-depth` dropped and all other values intact.
- `AmpSens = 0` vs `AmpSens = 7` produces measurably different output
  with an LFO1→op-level matrix route active.
- LFO1→GlobalPitch at block size 256 produces no audible stepping
  (verified by the smoothing test in 0063, plus manual listen).
- LFO1→Op1Level and LFO1→Op1Pan at block size 256 produce no audible
  zipper (engine ramp in 0074 + CONTROL_BLOCK render slicing in 0075,
  verified by the zipper regression test, plus manual listen).
- In solo mode, note-off with another key held falls back to the held
  note (audible + test).
- Recording knob automation in Reaper/Bitwig produces a correctly
  bracketed automation gesture (manual test).
- A synced LFO rate shows a subdivision label (`1/8`) in the host's
  automation lane readout (manual test).
- One `ParamChanged` per model write reaches the view per tick — no
  double emission on `SetParam` or state load (asserted by test).
- CI runs vxn-2 tests on push and fails the build on test failure.

## Notes

Review reports (six agent sweeps + verification, 2026-06-10) are the
source for all file:line references in the tickets. Where a ticket's
line numbers drift from HEAD, the symbol names are authoritative.
