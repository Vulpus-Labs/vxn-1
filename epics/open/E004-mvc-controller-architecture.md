---
id: E004
product: vxn-1
title: MVC controller architecture (vxn-app crate, event channels, behaviour-preserving cutover)
status: open
created: 2026-05-30
---

## Goal

Introduce the `vxn-app` crate, the `ParamModel` / `EditorBackend` traits,
the `UiEvent` / `HostEvent` / `ViewEvent` enums and the `Controller` that
drives them — then route the existing Vizia editor and the clack shell
through the controller. **No new features, no UI change.** The faceplate
looks and feels identical; only the data flow underneath moves.

Decisions recorded in [ADR 0007](../../vxn-1/adrs/0007-vxn1-mvc-architecture.md).

## Background

Today UI control callbacks write `SharedParams` directly, raise
gestures inline, and re-derive preset state from `vxn-engine` on the
fly. The audio thread reads atomics; the host echo is built into the
clack `process` loop via `LocalParams`. There is no single arbiter when
"a parameter changes" — the answer is split across three crates.

That is fine for one engine and one UI; it does not scale to a second
of either. ADR 0007 lays out the destination (Model / Controller / View
with a `vxn-app` crate carrying the controller and the traits); this
epic is the behaviour-preserving cutover that makes it real.

## In scope

- New crate `vxn-app` (workspace member).
- Traits: `ParamModel`, `ParamDescriptor`, `EditorBackend`.
- Event enums: `UiEvent`, `HostEvent`, `ViewEvent`.
- `Controller<M: ParamModel>` skeleton + `tick()` loop.
- `SharedParams: ParamModel` impl (in vxn-engine, against the trait
  from vxn-app).
- Wire vxn-clap to drive the controller from `process` / `flush` /
  state save+restore. Audio path **unchanged** (still reads atomics).
- Migrate every Vizia editor write to post a `UiEvent`. Replace the
  current `PollAutomation` re-read with a `ViewEvent` drain in the
  editor's idle callback.

## Out of scope

- WebView UI (E005).
- Browser ergonomics, floating popups, preset corpus redesign (E011).
- Renaming `vxn-ui` → `vxn-ui-vizia`. Deferred to E005 to avoid a
  needless workspace churn this epic.
- New features. Any behaviour change is a regression.

## Phasing

- **0033** scaffold `vxn-app` + trait surface + event enums.
- **0034** `SharedParams: ParamModel`; descriptor adaptor.
- **0035** `Controller<M>` skeleton — owns mpsc receivers, tick loop,
  no-op handlers that mirror today's direct writes.
- **0036** host events → controller (clack shell extracts to
  `HostEvent`, posts; controller folds to model + emits ViewEvents).
- **0037** UI events → controller (every Vizia callback posts
  `UiEvent`; editor stops touching `SharedParams` directly).
- **0038** preset IO moves into controller; editor reads corpus from
  `ViewEvent`s only.

Each ticket is sized to land green tests on its own; the existing
preset / state integration tests are the regression net.

## Acceptance

- `cargo test --workspace` passes at every ticket boundary.
- Plugin still loads, plays, accepts host automation, records UI
  edits as automation, saves and restores state, loads factory and
  user presets, switches key mode and split point — **with no
  observable change** to the user.
- vxn-clap's `process` loop reads `SharedParams` atomics with the same
  cadence and structure it does today (audio thread integrity).
- `vxn-app` has no `vizia` / `wry` dependency. Its only UI awareness
  is the `EditorBackend` trait. A `Controller<MockModel>` round-trip
  test runs without any GUI toolkit linked.
