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

- [ ] `vxn2-web-controller` crate exists (`crate-type = ["cdylib"]`),
      depends on `vxn2-app` (and `vxn-app`/`vxn-core-app` as that pulls
      in), NO engine dep.
- [ ] Builds for `wasm32-unknown-unknown --release`.
- [ ] C-ABI surface accepts encoded UiEvent opcodes and emits ViewEvent
      opcodes (same opcode vocabulary the native `vxn2-ui-web` bridge uses).
- [ ] Controller logic is reused unchanged — zero vxn-2-controller-logic
      edits between native and web (parity with vxn-1 ADR 0009 / 0007).

## Notes

Reference: `vxn-1/crates/vxn-web-controller`. The opcode codec must match
what the faceplate-bridge (ticket 0155/0157) speaks. vxn-2's custom
opcodes live in `vxn2-ui-web/src/lib.rs` (`parse_custom_ui` /
`serialise_custom_view`) — the web path reuses that vocabulary.
