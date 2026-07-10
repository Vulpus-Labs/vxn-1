---
id: "0153"
product: vxn-2
title: vxn2-wasm engine cdylib — C-ABI render loop + event codec
priority: medium
created: 2026-06-30
epic: E030
---

## Summary

New `vxn2-wasm` crate: the vxn-2 engine compiled to
`wasm32-unknown-unknown` as a raw C-ABI cdylib (no wasm-bindgen) for the
AudioWorklet. Ports `vxn-1/crates/vxn-wasm` (`src/host.rs`, `src/codec.rs`)
with `vxn-engine`→`vxn2-engine`, `vxn-app`→`vxn2-app`. The 2026-06-30
spike already proved the engine cross-compiles unchanged, so this is the
host wrapper, not core work.

## Acceptance criteria

- [x] `vxn2-wasm` crate exists (`crate-type = ["cdylib"]`), dep
      `vxn2-engine` only (see divergence 1), builds for
      `wasm32-unknown-unknown --release -C target-feature=+simd128`.
- [x] C exports mirror vxn-wasm: `vxn_host_new(sample_rate)`,
      `vxn_host_render(ptr, n_events)` (key-mode/split dropped, divergence 2),
      `vxn_host_set_param`/`set_param_norm`/`get_param`, `vxn_host_events_ptr`,
      `vxn_host_out_l`/`out_r`, `vxn_host_set_sample_rate`, `vxn_host_reset`,
      plus `vxn_host_max_events` / `vxn_quantum` / `vxn_host_force_trap`.
- [x] Render loop slices the 128-frame quantum at event sample-offsets and
      applies decoded events — same shape as the `vxn2-clap` process batch
      loop (single source of render truth), with each region sub-chunked to
      `CONTROL_BLOCK` = 32 (divergence 3). Proven by a byte-identical
      fold→slice→chunk reference test.
- [x] Event codec ported: 16-byte fixed slots, tag numbering carried from
      vxn-1; param norm-vs-plain flag honoured via `ParamDesc::from_normalised`.
      Golden-byte table retained for the JS twin (0155).
- [x] Param-index field width re-checked: vxn-2 has **209** params (not the
      ~250 estimated), well inside `u16`; asserted against
      `vxn2_engine::TOTAL_PARAMS` in a test.

## Close-out (2026-07-10)

Done. `cargo test -p vxn2-wasm` → 14 pass; wasm32 release+SIMD builds clean
(272 KB), all 14 `vxn_host_*` / `vxn_*` exports present in the artifact.

**Divergences from the vxn-1 mechanical port** (vxn-2 architecture, not
optional choices):

1. **Dep is `vxn2-engine` only, not `+ vxn2-app`.** vxn-2's param table
   (`ParamDesc` / `desc_for_clap_id` / `TOTAL_PARAMS`) lives in the engine
   crate; vxn-1 needed `vxn-app` because its ids lived there. No `LocalParams`
   mirror either — that type lives in the non-wasm-clean `vxn2-clap`. The host
   folds the atomic `SharedParams` store straight into the engine via
   `Engine::snapshot_params` (which calls `apply_block_params`).

2. **No key-mode / split-point.** vxn-2's FM engine is a single voice pool with
   no dual/split layer, so vxn-1's `EV_KEY_MODE` (7) / `EV_SPLIT_POINT` (8)
   events and the `vxn_host_render` key-mode/split args are dropped. Tags 7/8
   stay reserved so notes/params/gestures keep vxn-1's byte numbering.

3. **Params are block-granular, not per-sample.** vxn-2 has no per-id
   `Engine::set_param`; `vxn_host_set_param` / `EV_PARAM` write the store and
   the engine folds it once at the top of each render. A mid-quantum `EV_PARAM`
   lands at the next quantum — the same one-block latency the plugin documents.
   `process_block` also asserts `len <= CONTROL_BLOCK`, so each event-sliced
   region is rendered in ≤32-frame chunks (the plugin's `control_chunks`).

Ticket text above updated to match. 0154/0156 wiring unblocked.

## Notes

Reference: `vxn-1/crates/vxn-wasm/src/host.rs` (render loop, ticket 0038) and
`codec.rs` (16-byte framing, ticket 0037). Codec has a twin JS impl in ticket
0155 — keep the wire format byte-identical (golden table is the contract).
Build: `rustup target add wasm32-unknown-unknown` under the pinned 1.95.0
toolchain; `RUSTFLAGS="-C target-feature=+simd128"`. Blocks 0154/0156 wiring.
