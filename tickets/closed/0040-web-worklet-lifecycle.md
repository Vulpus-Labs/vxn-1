---
id: "0040"
product: vxn-2
title: "Harden: worklet lifecycle, sample-rate, trap safety"
priority: medium
created: 2026-06-14
epic: E015
depends: ["0038"]
---

## Summary

Harden the worklet audio-host (0038) for real-world use — the lifecycle
and failure-mode work that turns a working render loop into a dependable
one. Closes the audio-thread half of
[E015](../../epics/open/E015-web-event-driven-core.md).

## Design

- **Instantiate-from-bytes + silence-until-ready**: the worklet receives
  wasm bytes (via `processorOptions`, per 0034), instantiates async, and
  outputs silence until live — already prototyped in 0034's
  `vxn-processor.js`; make it robust (buffer events arriving pre-ready,
  per the spike's `pendingNotes`).
- **Sample rate**: build the engine at the worklet's `sampleRate` global,
  and handle context sample-rate differences (the engine's
  `Synth::set_sample_rate` exists; wire it).
- **Suspend/resume**: cleanly stop/restart rendering with AudioContext
  state changes without dropping ring state or leaking voices.
- **Teardown**: free the engine (`vxn_destroy` analogue), detach the node,
  release the SAB references — no leaks across re-init.
- **Trap safety**: a wasm trap/panic in `process()` must not permanently
  silence the context. Catch at the worklet boundary; recover (re-init) or
  fail loud to the main thread. The plugin unwinds at the host boundary
  ([[vxn1-architecture]] panic policy); the web needs an equivalent.

## Acceptance criteria

- [ ] The worklet outputs silence until the wasm is live, buffering any
      events that arrive first, then applying them in order.
- [ ] The engine runs at the context sample rate; a sample-rate change is
      handled without artefacts.
- [ ] Suspend/resume and teardown/re-init leave no leaked voices, stuck
      notes, or dangling SAB references.
- [ ] A forced trap in `process()` does not permanently kill audio — it is
      caught at the worklet boundary and surfaced to the main thread.

## Notes

- Depends on [0038](0038-web-worklet-audio-host.md) (the host being
  hardened). Last E015 ticket — when this closes, the event-driven core is
  complete and E016/E017/E018 build on a stable contract.
- Reuses 0034's lifecycle scaffolding in
  `vxn-1/crates/vxn-wasm/web/vxn-processor.js` as the starting point.
- Out of scope: the main-thread AudioContext lifecycle (E016) — this
  ticket is the worklet side only.

## Close-out (2026-06-15)

- **Lifecycle runner** [host-runner.mjs](../../vxn-1/crates/vxn-wasm/web/host-runner.mjs)
  (`WorkletHostRunner`), shared by the worklet and the harness, wrapping the
  0038 `AudioHost` with the lifecycle + failure-mode policy. Owns the wasm
  bytes + SAB refs so it can re-instantiate after a trap without losing
  transport state.
- **Silence-until-ready + pre-ready event buffering** (AC1): `process()`
  before `init()` resolves fills exact silence and does **not** drain the
  ring, so events the producer writes pre-ready survive (their read index is
  untouched) and apply in order on the first ready quantum. Verified: a
  note pushed pre-ready applies at its offset (onset 11 for offset 10) once
  live; `ring.pending()` confirms no pre-ready drain.
- **Sample rate** (AC2): engine built at the worklet `sampleRate` global;
  `vxn_host_set_sample_rate` → `Synth::set_sample_rate` wired
  ([host.rs](../../vxn-1/crates/vxn-wasm/src/host.rs)) for context-rate
  changes / offline render. Rust test `set_sample_rate_keeps_rendering` +
  harness both confirm audio survives the change.
- **Suspend/resume + teardown** (AC3): `reset()` →
  `vxn_host_reset`/`Synth::reset` clears sounding voices without touching the
  ring/store (resume-after-suspend stuck-note guard); `destroy()` frees the
  host (`vxn_host_destroy`) and nulls the SAB/bytes refs. Rust test
  `reset_clears_sounding_voices` + harness (reset silences a held note,
  post-destroy `process()` is safe silence, refs released).
- **Trap safety** (AC4): a render-thread trap is caught at the runner
  boundary — `process()` never throws, outputs silence, surfaces the trap to
  the main thread via `onTrap` (worklet re-posts `{type:"trap"}`), marks the
  poisoned instance not-ready, and kicks an async re-instantiate over the
  **same SABs** so transport state survives and audio recovers. Proven
  headlessly via a test-only `vxn_host_force_trap` (Rust `panic!` → wasm
  `unreachable`): harness sees the trap caught + surfaced ("unreachable"),
  then a fresh note renders again (onset 6 for offset 5) after recovery.
- **Worklet** [vxn-processor-0038.js](../../vxn-1/crates/vxn-wasm/web/vxn-processor-0038.js)
  now drives the runner: port messages `keyMode`/`splitPoint`/`sampleRate`/
  `reset`/`destroy`; posts `ready`/`trap` back to main; `process()` is one
  trap-safe `runner.process()`.
- **Tests:** `cargo test -p vxn-wasm` 16/16 (adds reset + sample-rate);
  [harness-0040.mjs](../../vxn-1/crates/vxn-wasm/harness-0040.mjs) all checks
  pass; 0035/0038 harnesses + 0037/0039 JS suites still green. AudioContext
  bootstrap stays E016.
