---
id: E001
title: vxn-core-* shared crate extraction
status: open
created: 2026-06-06
---

## Goal

Promote the repo root to a Cargo workspace owning a `crates/` tree of
shared `vxn-core-*` crates, then migrate vxn-1 onto them. Land before
vxn-2's `E003-faceplate` opens so the HTML editor + Controller layer is
built once, not twice.

When this epic closes:

- Repo root is a Cargo workspace. `vxn-1/` and `vxn-2/` either fold in
  as path members or become sub-workspaces pointing at root crates via
  relative paths.
- `crates/vxn-core-utils`, `vxn-core-app`, `vxn-core-ui-web`,
  `vxn-core-clap` build, test, and publish-dry-run cleanly.
- `vxn-1` consumes all four crates with no behavioural regression. The
  factory bank still loads, the WebView faceplate still renders, all
  vxn-1 tests pass, audio output is bit-identical to a tagged
  pre-extraction baseline (delta < 1e-6 RMS on a fixed test patch
  rendered against a fixed MIDI input).
- `vxn-2`'s open `E003-faceplate` epic is updated to depend on
  `vxn-core-app` + `vxn-core-ui-web` rather than recreate them.
- A short ADR (root `adrs/0001`) records the split and the rationale
  for what was extracted vs left synth-local.

## Why now

`E003-faceplate` is open. Its scope explicitly says "Mirrors
`vxn-1/crates/vxn-app` in shape" and "macOS-first; Windows / Linux
popup paths stubbed". That is the second implementation of code vxn-1
already ships. Extract upward (UI/Controller, CLAP scaffold) before
E003 lands, so the editor is written against a shared surface from day
one. DSP and engine code stays per-synth — the signal models are the
differentiator and forced abstraction there is pure cost.

## Scope

**In:**

- Top-level `Cargo.toml` workspace at repo root.
- `crates/vxn-core-utils` — denormal/FTZ guard, one-pole smoother,
  `note_to_hz`, host-tempo subdivision table (BPM ↔ Hz / beats).
- `crates/vxn-core-app` — `ParamModel` trait, `ParamDesc` / `ParamKind`
  / taper, `UiEvent` / `ViewEvent` / `HostEvent` schema, gesture
  bracket types, `Controller` event loop, `EditorBackend` trait,
  `PresetStore` trait, preset corpus model.
- `crates/vxn-core-ui-web` — `wry` WebView lifecycle, parent-handle
  handoff, JS↔Rust IPC bridge (`batch_chunks`), `evaluate_script` tick
  batcher, native text-input popup (macOS / Win / Linux).
- `crates/vxn-core-clap` — `clack` 3-thread scaffold
  (Shared / MainThread / AudioProcessor), CLAP event dispatch
  (note / bend / CC / mod-wheel / aftertouch), `LocalParams`
  audio-thread mirror pattern, state-blob save/load via `ParamModel`,
  gesture bracket emit. Synth-agnostic — instantiates over a generic
  `Engine: ProcessBlock + ParamModel` plug.
- vxn-1 migration onto all four shared crates with full test pass and
  byte-identical audio diff (where determinism allows).
- E003 scope update to reference shared crates.
- Root ADR documenting the split.

**Out:**

- DSP primitives (oscillators, filters, envelopes, LFOs). Signal
  models diverge between vxn-1 (analog osc + OTA ladder) and vxn-2
  (FM operator + DX7 algos). No shared crate.
- Voice allocator / voice stack. vxn-1 = 8-voice scalar dual-layer,
  vxn-2 = 16-voice SoA + stacking. Different shapes.
- `SharedParams` atomic store. Similar but not identical between the
  two; duplication cost low vs extraction effort. Revisit if a third
  consumer appears.
- Param tables (`PatchParam`, `GlobalParam`, vxn-2's 380-param
  registry). Per-synth by definition; only the `ParamModel` machinery
  is shared.
- Modulation matrix. vxn-1 = fixed routes (ADR 0004), vxn-2 = 8×9
  generic matrix-only routing. Different topology.
- xtask CLAP bundling. Deferred to a follow-up — both bundle scripts
  work today and the bundle format is small.

## Tickets

- [x] [0001 — Root workspace bootstrap + skeleton crates](../../tickets/closed/0001-workspace-bootstrap.md)
- [x] [0002 — vxn-core-utils (FTZ, smoother, note utils, host-sync)](../../tickets/closed/0002-vxn-core-utils.md)
- [x] [0003 — vxn-core-app (ParamModel, Controller, events, backend)](../../tickets/closed/0003-vxn-core-app.md)
- [x] [0004 — vxn-core-ui-web (wry shell, IPC bridge, text-input popup)](../../tickets/closed/0004-vxn-core-ui-web.md)
- [x] [0005 — vxn-core-clap (clack scaffold, event dispatch, state I/O)](../../tickets/closed/0005-vxn-core-clap.md)
- [ ] [0006 — vxn-1 migration + E003 unblock](../../tickets/open/0006-vxn-1-migration.md) *(partial — see below)*

## Status — 2026-06-06

Tickets 0001–0005 closed. Shared crates ship and are tested:
55 unit + integration tests across `vxn-core-utils` (16),
`vxn-core-app` (11), `vxn-core-ui-web` (11), `vxn-core-clap` (10),
plus 6 `vxn-core-app` taper-math tests.

0006 partial: vxn-1 consumes the bit-identical leaf types
(`ParamDesc` / `ParamKind` / `Taper` / `ParamId` / `PresetMeta` /
`PresetStore` / `PresetCorpus` / `PresetLoad` / `UserPresetEntry` /
`UserFolderEntry`) and DSP utilities (`ScopedFlushToZero` /
`Smoothed`) from `vxn-core-*`. The full event-Custom rewire (vxn-1's
`UiEvent::SetKeyMode` / `SetSplitPoint` / `SetEditLayer` /
`ResetLayer` and the matching `ViewEvent` variants riding
`UiEvent::Custom` / `ViewEvent::Custom`) is deferred — it would touch
~40 call sites across `vxn-app` / `vxn-clap` / `vxn-ui-web` and
requires:

- a `Vxn1Params` extension trait carrying `key_mode` / `split_point`
- `Vxn1UiCustom` / `Vxn1ViewCustom` enums to ride the `Custom`
  payloads
- `parse_custom_ui` / `serialise_custom_view` closures wired into
  `vxn-core-ui-web::open_editor`
- the audio baseline diff harness (1e-6 RMS gate) per ticket 0006

All 5 deferred bullets live under 0006 still — the ticket stays
open. vxn-2's `E003-faceplate` epic is updated to depend on the
shared crates instead of re-implementing.

## Dependency order

```text
0001 (workspace bootstrap)
  │
  ├─> 0002 (vxn-core-utils) ───┐
  │                              ├─> 0006 (vxn-1 migration, E003 unblock)
  ├─> 0003 (vxn-core-app)  ─────┤
  │                              │
  ├─> 0004 (vxn-core-ui-web) ───┤
  │                              │
  └─> 0005 (vxn-core-clap) ─────┘
```

- 0001 lays workspace + empty skeleton crates so 0002–0005 can land in
  parallel without merge conflicts on `Cargo.toml`.
- 0002–0005 can be worked in parallel; only 0006 needs all four.
- 0006 is the integration step: rewire vxn-1 to consume shared crates,
  delete the now-duplicate code, prove byte-identical audio.

## Risks

- vxn-1 `Controller` carries vxn-1-specific param enums in event
  payloads. `vxn-core-app` events must take a generic param id
  (`u32` / `ClapId`), not `PatchParam`. Per-synth payloads via assoc
  type or `Box<dyn Any>` payload TBD in 0003.
- vxn-1 `ViewEvent::KeyModeChanged` and similar carry Layer / KeyMode
  enums. Either generalise (assoc type) or accept that
  `vxn-core-app::events` is "MIDI-keyboard synths with optional
  layers". Pick early in 0003.
- Bit-identical audio diff in 0006 only holds if extraction is pure
  refactor (no algorithm change). If LFO seeding or RNG state lives
  in a moved struct, fix the seeding contract before measuring.
- Three-workspace transition (root + vxn-1 + vxn-2) needs a single
  `Cargo.lock` strategy. Workspace inheritance via `[workspace.package]`
  + `[workspace.dependencies]` keeps clack rev pinned across all
  crates. Settle in 0001.

## Acceptance

- All four `vxn-core-*` crates build, test, and `cargo publish --dry-run`
  with no path-only deps.
- vxn-1 consumes all four crates. `cargo test -p vxn-1-*` passes. The
  WebView faceplate renders. The factory bank loads. Audio rendered
  against `tests/golden/<patch>.mid` matches the pre-extraction
  baseline within 1e-6 RMS.
- vxn-2's `E003-faceplate.md` is updated: scope lists shared crates,
  drops re-implementation language.
- Root `adrs/0001-vxn-core-split.md` records the extraction rationale,
  the explicit not-extracted list, and the boundary criteria for
  future shared crates.
- No new `unwrap`/`expect` introduced in audio-thread paths. No new
  allocations in the process callback (verified via existing vxn-1
  and vxn-2 RT lint suites).
