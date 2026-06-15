---
id: "0056"
product: vxn-1
title: "Key-mode / split-point control path (non-automatable, over the port)"
priority: medium
created: 2026-06-15
epic: E017
depends: ["0042"]
---

## Summary

The control surface for Whole/Dual/Split key mode and the split point. These are
NON-automatable shared state (ADR 0003 §3): they never occupy a param-store slot
and never travel on the event ring — they go out-of-band on the worklet port and
latch at block start. A thin, named helper module the faceplate (E018) binds its
mode buttons / split control to, so the page never pokes the port shape directly
and the mode↔int mapping lives in one place.

## Design

- **Module** `vxn-1/crates/vxn-wasm/web/key-mode.mjs`.
- **Enum** `KeyMode { WHOLE:0, DUAL:1, SPLIT:2 }` + `KEY_MODE_LABELS` — the wire
  values matching the engine's key-mode ordering. Mirrors the vxn-clap
  `UiEvent::Custom` key-mode/split path (same modes, same "set once per block").
- **`setKeyMode(host, mode)`** — accepts the numeric enum or a case-insensitive
  name; routes to `host.setKeyMode(m)` (which owns the port hop). Junk → `null`,
  no port write (never poisons the worklet with a bad value).
- **`setSplitPoint(host, note)`** — clamps 0..127, routes to
  `host.setSplitPoint(n)`; returns the clamped note.
- **`attachKeyMode(host, opts)`** — stateful convenience for a faceplate: pushes
  the initial mode/split on attach (so worklet + UI start in sync) and tracks
  the current values (`getMode`/`getModeLabel`/`getSplitPoint`,
  `setMode`/`setSplitPoint`). Holds no audio state — the worklet is the source
  of truth; this just remembers what was last sent so the UI can reflect it.

## Acceptance criteria

- [ ] `setKeyMode` routes WHOLE/DUAL/SPLIT (by enum and by name) to
      `host.setKeyMode`; out-of-range / unknown → `null`, no call made.
- [ ] `setSplitPoint` clamps to 0..127 and routes to `host.setSplitPoint`.
- [ ] `attachKeyMode` pushes the initial mode + split on attach and tracks the
      current values through subsequent sets.

## Notes

- Routes over `WebHost.setKeyMode`/`setSplitPoint` (worklet port), NOT params,
  NOT the ring — these are E015 non-automatable state.
- The display widgets (mode buttons, split-point control) are E018's; this is
  the control path only.

## Close-out (2026-06-15)

- **Module.** `vxn-1/crates/vxn-wasm/web/key-mode.mjs`. `KeyMode {WHOLE:0,
  DUAL:1, SPLIT:2}` + `KEY_MODE_LABELS`. `setKeyMode(host, mode)` accepts enum
  or case-insensitive name, routes to `host.setKeyMode`; junk → `null`, no port
  write. `setSplitPoint(host, note)` clamps 0..127 → `host.setSplitPoint`.
  `attachKeyMode(host, opts)` pushes initial mode+split on attach and tracks
  current values (`getMode`/`getModeLabel`/`getSplitPoint`).
- **Routing.** Over `WebHost.setKeyMode`/`setSplitPoint` (worklet port) — NOT
  params, NOT the ring; E015 non-automatable state, latched at block start.
- **Tests.** `web/key-mode.test.mjs` §1 (setKeyMode by enum + by name, junk →
  null no-op), §2 (split clamps), §3 (attachKeyMode initial push + tracking,
  bad setMode leaves tracked mode unchanged). Uses a fake host recording the
  port-routed calls.
- **Build.** `key-mode.mjs` added to xtask MODULES; bundles to dist.
- **Headless run note.** Harness written + self-reviewed; manual `node` run
  pending (script execution blocked in this sandbox).
