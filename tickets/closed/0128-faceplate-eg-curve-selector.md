---
id: "0128"
product: vxn-2
title: "Faceplate UI selector for per-op eg-curve (optional)"
priority: low
created: 2026-06-23
epic: E026
depends: ["0124"]
---

## Summary

Sixth (optional) ticket of [E026](../../epics/open/E026-dx7-log-level-curve.md).
Decide whether the per-op `eg-curve` (`Lin | Exp`) from 0124 should be
user-facing in the web faceplate, and if so add the selector.

## Design

The KS-curve precedent is patch-state-only with **no** UI control, so the
default expectation is that `eg-curve` is likewise hidden (Exp for ~all patches;
exposing it risks clutter for a rarely-touched escape hatch). If we do expose it,
add a per-op `Lin/Exp` selector in `vxn2-ui-web` mirroring an existing enum
control and wire it through the opcode path.

## Acceptance criteria

- [ ] Decision recorded: user-facing or patch-state-only.
- [ ] If user-facing: per-op selector in `vxn2-ui-web` (HTML + main.js), bound to
      the `op{N}-eg-curve` id, with the standard enum-control test.
- [ ] If not: close as won't-do with the rationale (parity with KS-curve).

## Notes

Lowest priority in the epic; the default (Exp) is correct for essentially every
DX7 patch, so most users never need to touch this.
