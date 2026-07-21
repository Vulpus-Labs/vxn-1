---
id: "0154"
product: vxn-2
title: vxn2-web-controller cdylib — main-thread controller wasm
priority: medium
created: 2026-06-30
epic: E030
---

## Summary

New `vxn2-web-controller` crate: the vxn-2 controller (the `vxn-app` MVC
arbiter) compiled to wasm for the main thread, reused verbatim — the same
arbiter that drives the native CLAP build. Ports
`vxn-1/crates/vxn-web-controller`. UiEvent in → model mutation → ViewEvent
out, no engine dependency (engine lives in the worklet, ticket 0153).

## Acceptance criteria

- [x] `vxn2-web-controller` crate exists (`crate-type = ["cdylib"]`),
      depends on `vxn2-app` + `vxn2-engine` (see divergence — "no engine dep"
      is impossible for vxn-2). Builds `wasm32-unknown-unknown --release`
      (229 KB, 22 `vxnc_*` exports).
- [x] C-ABI surface accepts encoded UiEvent opcodes (`vxnc_ui_*` — set_param /
      set_param_norm / begin/end_gesture / editor_ready + the Vxn2 custom set:
      set_op_tab / set_matrix_row / set_ks_curve / set_eg_curve / request_*
      snapshots / request_full_rebroadcast) and emits packed ViewEvent records
      (ParamChanged + OpTabChanged + Matrix/Ks/Eg snapshots) via
      `vxnc_view_ptr`/`_len`. Wire format documented in the module header;
      decoded JS-side in 0157.
- [x] Controller logic reused unchanged: the model is `SharedParams` and the
      pump is `vxn2_app::tick_vxn2` — the SAME types `vxn2-clap` drives. The
      Model→View emitter is a re-impl of `vxn2-clap::drain_dirty_bits` (dirty-
      bitset drain, echo disabled) — identical logic, no new arbiter.

## Close-out (2026-07-10)

Done. `cargo test -p vxn2-web-controller` → 4 pass (full-table first-tick
broadcast, single-param dirty surface, matrix-row snapshot, readback drift →
ParamChanged); wasm32 release builds clean.

**Divergence — "NO engine dep" is not achievable for vxn-2.** vxn-1's `vxn-app`
was standalone, so its web controller had no engine link. vxn-2's `vxn2-app`
*itself* depends on `vxn2-engine` — the `Vxn2Params` impl and the param table
(`TOTAL_PARAMS` / descriptors / `sync_aware_display`) live on `SharedParams`
inside the engine crate (by design, orphan rule). So the controller links
`vxn2-engine`, but **never runs it**: no `Engine::process` on the main thread;
the audio engine is a separate wasm in the worklet. This turned into an
advantage — the controller reuses `SharedParams` as its model verbatim (same as
native), so `snapshot_bytes`/`restore` use the real vxn-2 blob codec for free
(helps 0159), and the whole Model→View path is the native `drain_dirty_bits`.

Preset/corpus/journal/state opcodes (vxn-1's `vxnc_load_factory` /
`vxnc_take_journal` / `vxnc_*_state` / `vxnc_corpus_json`) are **out of scope
here** — they land with browser persistence (0159).

## Notes

Reference: `vxn-1/crates/vxn-web-controller`. The packed ViewEvent wire format
(module header) is what the faceplate bridge (0157) decodes; vxn-2's custom
opcode vocabulary is `Vxn2UiCustom` / `Vxn2ViewCustom` (vxn2-app `events.rs`),
the same set the native `vxn2-ui-web` bridge speaks.
