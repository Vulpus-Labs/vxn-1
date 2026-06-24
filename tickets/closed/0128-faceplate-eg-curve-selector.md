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

## Close-out (2026-06-24)

- **Decision: user-facing.** Per the user's call, the per-op eg-curve is exposed
  as a faceplate control (not patch-state-only). A per-op **RAMP exp/lin** toggle
  sits in the op-row Envelope column.
- Full non-CLAP opcode path wired, mirroring the KS-curve precedent (eg-curve is
  patch state, *not* a CLAP-automatable enum — so there is no CLAP enum-control
  to test; the opcode + snapshot tests are the analogue, exactly as KS curve has
  no enum-control test):
  - `Vxn2Params` trait: `eg_curves()` / `set_eg_curve()` / `take_dirty_eg_curve()`
    ([model.rs](../../vxn-2/crates/vxn2-app/src/model.rs)), impl on `SharedParams`
    ([shared.rs](../../vxn-2/crates/vxn2-engine/src/shared.rs)).
  - Events `Vxn2UiCustom::SetEgCurve` / `RequestEgCurveSnapshot`,
    `Vxn2ViewCustom::EgCurveSnapshot { curves: [u8;6] }`
    ([events.rs](../../vxn-2/crates/vxn2-app/src/events.rs)); controller handlers
    + `eg_curve_snapshot_event` / `push_eg_curve_snapshot`
    ([controller.rs](../../vxn-2/crates/vxn2-app/src/controller.rs)).
  - CLAP pump drains `take_dirty_eg_curve` → pushes `EgCurveSnapshot`
    ([vxn2-clap/lib.rs](../../vxn-2/crates/vxn2-clap/src/lib.rs)); seeded dirty on
    boot/`mark_all_dirty`, so the snapshot auto-delivers at boot + preset load.
  - ui-web: `set_eg_curve` / `request_eg_curve_snapshot` opcode parse +
    `eg_curve_snapshot` serialise ([ui-web/lib.rs](../../vxn-2/crates/vxn2-ui-web/src/lib.rs));
    `egCurves` cache (bootstrap.js), `eg_curve_snapshot` dispatcher (main.js),
    the toggle + `onEgCurveSnapshot` repaint (panels/op-row.js), CSS (style.css).
- Tests (green): `parse_custom_set_eg_curve`, `parse_custom_request_eg_curve_snapshot`,
  `serialise_view_eg_curve_snapshot_shape` (ui-web); `set_eg_curve_then_snapshot_round_trips`
  (vxn2-app controller). Workspace builds clean.
- Manual: the toggle's live behaviour (and that it round-trips through preset
  TOML / host state) is part of the user's Reaper verification pass alongside
  0125. JS vitest deps aren't installed locally (CI runs them); the JS changes
  are additive and isolated.
