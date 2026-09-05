---
id: "0356"
product: vxn-3
title: "Faceplate: Y-centre curve editing and the groove pool UI"
priority: medium
created: 2026-09-04
epic: E050
depends: ["0350", "0352", "0353"]
---

## Summary

Closing faceplate ticket of
[E050](../../epics/open/E050-vxn3-continuous-lane-editor.md). Two surfaces:

- **Y-centre curve editing** — drag the control points sitting on beat markers
  (0350) to sweep a lane's whole contour as one gesture.
- **Groove pool UI** — name, save, assign, swap and lock-together the grooves of
  0352.

## Design

### Curve editing

Control points sit on beat markers, so they move when markers move (0349) and
need no independent handles in X — dragging is vertical only. The curve renders
behind the diamonds as the lane's baseline, and diamonds render **relative** to
it (0350), so dragging a control point visibly sweeps every hit in the region.

This is safe only because the curve is continuous ([ADR 0007](../../vxn-3/adrs/0007-vxn3-continuous-lane-editor.md)
§6) — a per-slot step function here would make horizontal hit drags jump
vertically. Render the curve so the interpolation is legible; a user who thinks
it is stepped will place hits expecting a jump.

### Groove pool

Named grooves, assignable per lane, per ADR 0006's pooling carried into ADR 0007
§8. The lock-together control needs to make two things explicit in the UI, since
both are decisions the ADR requires be deliberate:

- **Locking asks which lane's beat markers to adopt.** No implicit
  first-lane-wins.
- **Unlocking leaves lanes where they are.** Say so at the point of unlock, so
  nobody expects a spring-back.

Lock covers beat markers only — sub-count and swing stay per lane, and the UI
should show that, since a locked grid with visibly different subdivisions is the
feature and will otherwise read as a bug. Dim or mark the grid on lanes with an
overridden sub-count.

## Acceptance criteria

- [ ] Y-centre control points drag vertically on beat markers; the curve
      re-renders live.
- [ ] Diamonds render relative to the curve; dragging a control point sweeps
      every hit in the interpolated region.
- [ ] Dragging a hit horizontally across a beat marker produces no vertical jump.
- [ ] Curve editing goes through 0350's clamped-tangent evaluation; no edit can
      push the curve outside the lane bounds.
- [ ] Grooves can be named, saved to the pool, and assigned per lane.
- [ ] Swapping a lane's groove visibly re-times its hits without editing them.
- [ ] Lock-together prompts for the source lane; unlock leaves positions
      unchanged and says so.
- [ ] Lanes with an overridden sub-count are visually marked while locked.
- [ ] Groove assignment and the pool round-trip through project state.

## Notes

Last ticket of E050 — with this landed, the epic's acceptance criteria are all
exercisable from the faceplate.

Groove extraction from an existing pattern and groove import are out of scope
(E050 §Scope); the pool ships with hand-authored grooves plus whatever a user
saves.
