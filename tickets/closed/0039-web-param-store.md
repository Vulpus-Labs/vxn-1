---
id: "0039"
product: vxn-2
title: "Scaffold: cross-thread param store + audio→main diff readback"
priority: high
created: 2026-06-14
epic: E015
depends: ["0036"]
---

## Summary

Implement the cross-thread parameter store chosen in the
[0036](0036-web-controller-placement-adr.md) ADR — the web analogue of
`SharedParams` — plus the audio→main **param-diff readback** that lets the
UI see audio-thread param drift (host-automation-style writes). The
readback ports the timer-tick param-diff pump
([vxn-clap/src/lib.rs:193-236](../../vxn-1/crates/vxn-clap/src/lib.rs#L193-L236)).

## Design

Per the 0036 decision, one of:

- **SAB-backed atomic array** (preferred if 0036 picks it): a
  `SharedArrayBuffer` of 165 atomics indexed by CLAP id (69×2 patch + 27
  global). Worklet reads lock-free in the render loop; the controller
  writes on edits. Latest-value-wins semantics, matching `SharedParams`.
- **Param-events-on-the-ring**: param changes flow as 0037 records; the
  store is the engine's own `ParamValues`. Simpler, but the bulk
  preset-load case (165 at once) and the diff readback need care.

**Diff readback (either way)**: a path for the worklet to publish current
param values back to the main thread (a second SAB region the main thread
polls on rAF, or a return ring), so the main thread can diff against
`last_seen` and emit `ParamChanged` to the UI — exactly what the plugin's
pump does for host automation. This is what makes automation and
modulation visible in the UI.

## Acceptance criteria

- [ ] The 0036-selected store is implemented: the worklet reads params
      lock-free in the render loop; the controller/main thread writes.
- [ ] A bulk update (load a preset → 165 params) applies correctly and
      without glitching the audio path.
- [ ] An audio-thread param write is observable from the main thread via
      the diff readback, yielding a `ParamChanged`-equivalent — verified
      by a test that mutates a param on the audio side and sees it surface.
- [ ] Param addressing matches 0036 (CLAP-id layout) and the 0037 codec.

## Notes

- Depends on [0036](0036-web-controller-placement-adr.md) (decides the
  mechanism). Proceeds alongside [0038](0038-web-worklet-audio-host.md)
  (which reads the store).
- Reference: `vxn-engine/src/shared.rs` (`SharedParams`), the param-diff
  pump (cited above). Related: [[vxn1-id-stability-dropped]].
- The readback is what E018's UI bridge consumes to reflect automation —
  keep its shape compatible with `ViewEvent::ParamChanged`.
- Out of scope: the UI side of the readback (E018), preset storage (E019).

## Close-out (2026-06-14)

- **SAB store implemented (ADR 0009 option (a))** in
  [param-store.mjs](../../vxn-1/crates/vxn-wasm/web/param-store.mjs): one
  `SharedArrayBuffer`, two contiguous i32 regions — `[0..165)` STORE (main
  writes, worklet reads lock-free) and `[165..330)` READBACK (worklet
  writes applied values, main polls). Each word is an f32 **plain** value
  bit-cast to i32; the authoritative op is `Atomics.load/store` on the i32
  view with an aliasing `Float32Array` for the cast — the direct mirror of
  native `SharedParams` (`AtomicU32` + `f32::to_bits`,
  [shared.rs](../../vxn-1/crates/vxn-engine/src/shared.rs)). No
  `Atomics.wait` anywhere (forbidden on the render thread).
- **Bulk preset load:** `writeBulk(165)` = 165 independent single-word
  `Atomics.store`s. Per-slot atomicity guaranteed (a concurrent reader sees
  each slot fully-old or fully-new, never a torn float); no cross-slot
  transactionality — identical to native `SharedParams`. Documented + the
  test's interleaved reader saw 0 torn floats.
- **Diff readback ports `push_param_diffs`**
  ([vxn-clap/src/lib.rs:193-236](../../vxn-1/crates/vxn-clap/src/lib.rs#L193-L236)):
  `pollDiffs(store,lastSeen)` NaN-aware-scans the readback region against a
  `last_seen` mirror and emits `{id,plain,norm,display}` — one-to-one with
  `ViewEvent::ParamChanged`. `newLastSeen()` seeds all-NaN so the first poll
  broadcasts all 165 (preserved native behavior). `norm`/`display` are
  intentional stubs with `TODO(E018)` (exact taper/sync-aware strings come
  from vxn-app metadata via the controller wasm).
- **Worklet API for 0038:** `applyStoreToEngine(store,engine,workletSeen)`
  reads the store lock-free, applies only changed ids via the existing 0035
  `vxn_set_param` shim (no new wasm export), and echoes each into the
  readback so the main thread observes audio-thread drift; `newWorkletSeen()`
  NaN-seeds so the first render applies all 165.
- **Addressing reconciled:** the id layout (165 = 69×2 + 27) is imported
  from the 0037 codec ([event-codec.mjs](../../vxn-1/crates/vxn-wasm/web/event-codec.mjs)) —
  one declared constant shared by store and codec, no drift.
- **Tests:** `node web/param-store.test.mjs` all 38 checks pass — bulk 165
  round-trip, f32 bit-cast edges (0, -0, NaN-seed, ±Inf, FLT_MAX, denormal,
  negative), diff readback (single / none / NaN-seed-all / dup-suppression /
  multi-drift), no-glitch atomicity, and the end-to-end
  store→engine→readback→pollDiffs flow. `cargo build … --target
  wasm32-unknown-unknown --release` still builds (no Rust added).
