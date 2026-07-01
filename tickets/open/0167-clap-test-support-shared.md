---
id: "0167"
product: monorepo
title: Shared CLAP test-support module — push_param_event, event_log, builders
priority: medium
created: 2026-07-01
epic: E031
---

## Summary

The CLAP event-buffer ritual is open-coded across vxn-2's `vxn2-clap`
(`src/lib.rs`, `src/local.rs`, `tests/smoke.rs`) and — per the tests' own
comments ("Mirrors VXN1", "same pattern as VXN-1's vxn-clap") — mirrors
vxn-1's `vxn-clap` scaffolding. The same "push a `ParamValueEvent` into an
`EventBuffer`, iterate, apply" pattern, the `mk_shared`/`mk_main`/`mk_audio`
builder trio, the `event_log` decoder, and the `build_controller` helper
are each re-implemented per file. Hoist them into one shared test-support
surface used by both synths.

Line numbers are as-reviewed on 2026-07-01; re-grep by name.

## Acceptance criteria

- [ ] Decide the home and record it in a one-line ADR note or the ticket
      close-out: either a new `vxn-test-support` dev-dependency crate, or
      `#[cfg(test)]` / `pub` test helpers exported from the existing shared
      `vxn-core-clap` / `vxn-core-app` crates (preferred if it avoids a new
      crate). Must be reachable from both vxn-1 and vxn-2 clap crates.
- [ ] Provide `push_param_event(buf, id, value)` — currently defined in
      `vxn2-clap/src/local.rs` (~284) and re-open-coded in `src/lib.rs`
      (~698/743/843) and `tests/smoke.rs`. Single definition, all call sites
      routed through it.
- [ ] Provide `event_log(buf) -> Vec<(&str, u32, u32)>` (the gesture
      decoder, `local.rs` ~385) and an `emit_after(shared, local,
      frame_count)` one-liner collapsing the `EventBuffer::with_capacity` +
      `emit` + `event_log` triple repeated ~8× in the four `emit_*` gesture
      tests (`local.rs` ~404/443/465/488).
- [ ] Provide the `mk_shared`/`mk_main`/`mk_audio` builders (already good in
      `vxn2-clap/src/lib.rs` ~669) from the shared module.
- [ ] Provide a single `build_controller` helper. Note: `controller.rs`
      (~15) and `editor_smoke.rs` (~45) currently define two *different*
      helpers with the same name — unify or clearly distinguish them.
- [ ] Migrate vxn-1 `vxn-clap` / `vxn-app` test call sites to the shared
      helpers where they duplicate the same ritual (the comments claim they
      already mirror each other — verify and dedupe).
- [ ] Coordinate with 0162's `const EDITS` extraction — put the shared
      state-round-trip edit list in this same test-support module.

- [ ] `cargo test -p vxn2-clap -p vxn-clap -p vxn2-app -p vxn-app` green;
      no behavioural change.

## Notes

This is the single most-repeated apparatus in the review and the only
cross-crate one, so it is the highest-leverage extraction — but it touches
the most files, so land it after the per-crate tickets (0164–0166) settle
their local line numbers. E024/E027 already pushed shared-core
consolidation (`vxn-core-clap` etc.); this extends that discipline to the
test layer. Prefer reusing those crates over standing up a new one.
