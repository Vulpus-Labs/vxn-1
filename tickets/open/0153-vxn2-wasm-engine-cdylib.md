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

- [ ] `vxn2-wasm` crate exists (`crate-type = ["cdylib"]`), deps
      `vxn2-engine` + `vxn2-app` only, builds for
      `wasm32-unknown-unknown --release -C target-feature=+simd128`.
- [ ] C exports mirror vxn-wasm: `vxn_host_new(sample_rate)`,
      `vxn_host_render(ptr, n_events, key_mode, split_point)`,
      `vxn_host_set_param`/`get_param`, `vxn_host_events_ptr`,
      `vxn_host_out_l`/`out_r`, `vxn_host_set_sample_rate`, `vxn_host_reset`.
- [ ] Render loop slices the 128-frame quantum at event sample-offsets and
      applies decoded events — same shape as the `vxn2-clap` process batch
      loop (single source of render truth).
- [ ] Event codec ported: 16-byte fixed slots, same type tags as vxn-1;
      param norm-vs-plain flag honoured via the vxn-2 param descriptors.
- [ ] Param-index field width re-checked against vxn-2's ~250+ params
      (vxn-1 had 165) — confirm `u16` still fits and is documented.

## Notes

Reference: `vxn-1/crates/vxn-wasm/src/host.rs` (~470 lines, render loop,
ticket 0038) and `codec.rs` (~250 lines, 16-byte framing, ticket 0037).
Codec has a twin JS impl in ticket 0155 — keep the wire format
byte-identical. Build env needs rustup not Homebrew rust (memory
`wasm-build-toolchain`). Blocks 0154/0156 wiring.
