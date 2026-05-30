---
id: "0033"
title: Scaffold vxn-app crate (traits, event enums, Controller skeleton)
priority: high
created: 2026-05-30
epic: E009
---

## Summary

Create the `vxn-app` workspace crate that owns the controller surface
defined by ADR 0007: the `ParamModel`, `ParamDescriptor` and
`EditorBackend` traits, the `UiEvent`/`HostEvent`/`ViewEvent` enums, and
an empty `Controller<M: ParamModel>` skeleton with its `tick()` loop.
Nothing wired yet — this is the surface that 0034–0038 fill in.

## Acceptance criteria

- [ ] `crates/vxn-app/` added; workspace member + `workspace.dependencies`
      entry; depends only on `vxn-engine` (for `ParamId` / shared types
      it needs to name) and `std`.
- [ ] `ParamModel` and `ParamDescriptor` traits defined per ADR 0007 §4.
- [ ] `EditorBackend` trait defined per ADR 0007 §4.
- [ ] `UiEvent`, `HostEvent`, `ViewEvent` enums defined per ADR 0007 §3.
- [ ] `Controller<M>` struct: owns two `std::sync::mpsc::Receiver` ends,
      has a `tick(&mut self)` method (empty body), constructible from
      `Arc<M>`. Sender halves exposed via `ui_sender()` and
      `host_sender()`.
- [ ] `cargo test -p vxn-app` passes (empty test suite OK).
- [ ] `cargo check --workspace` passes; `vxn-app` does not pull in
      `vizia` or `wry`.

## Notes

Channels: `std::sync::mpsc` bounded at 1024 to start (ADR 0007 §3 open
question — pick this and revisit if it ever saturates). UI and host
each get their own channel so a host-event burst can't head-of-line
block UI intents.

`ParamId` is a newtype over `usize` (the CLAP id). Stays in
`vxn-engine` for now (matches `desc_for_clap_id`). vxn-app re-exports
it under a stable name so VXN-2 doesn't import vxn-engine's CLAP-id
module by reflex.

No impls of any trait yet. 0034 lands the `SharedParams: ParamModel`
impl; 0035 fleshes out `Controller::tick`.
