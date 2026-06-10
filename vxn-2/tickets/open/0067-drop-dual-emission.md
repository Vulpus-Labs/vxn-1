---
id: "0067"
title: "Drop dual Model→View emission: SetParam echo + StateLoaded broadcast"
priority: medium
created: 2026-06-10
epic: E006
depends: ["0065"]
---

## Summary

Seventh ticket of [E006](../../epics/open/E006-review-remediation.md).
E005 made the dirty-bitset pump the single Model→View path, but two
pre-E005 echo paths survived in `vxn-core-app`'s controller:

1. `SetParam` / `SetParamNorm` handlers call `emit_param_changed`
   immediately after `model.set`
   (vxn-core-app `controller.rs:196-201`). The pump emits the same id
   again next tick — two `ParamChanged` per UI write.
2. `HostEvent::StateLoaded` calls `broadcast_all_params` (180 events)
   while `load_bytes` has already flipped all dirty bits — the pump
   broadcasts all 180 again the same tick. ~360 events per state load.

Harmless today (idempotent repaints) but it doubles view traffic and
becomes a jitter source now that `bindGestureGated` drops events
selectively. ADR 0003 §"What dissolves" intended these gone; only the
`SetMatrixRow` echo was actually deleted.

## Fix

Delete both echo paths so the pump is the only emitter for
model-backed state. **Caution: `vxn-core-app` is shared with VXN-1**,
which does not have the dirty-bitset pump. Options, in preference
order:

1. If VXN-1's tick still relies on the controller echo, gate the echo
   behind a controller config flag (`echo_param_writes: bool`) that
   VXN-2 sets false — smallest shared-crate footprint.
2. If VXN-1 has its own polling diff and the echo is redundant there
   too, delete outright.

Investigate which holds before choosing; record the answer in the
commit message.

Keep: `SetOpTab` echo (pure UI state, no model backing — correct per
ADR 0003) and `RequestMatrixSnapshot` (explicit UI-initiated query).

## Acceptance criteria

- [ ] Controller test: one `SetParam` UI intent + one pump tick yields
  exactly one `ParamChanged` for that id (count asserted — this is
  the regression test the review found missing).
- [ ] State-load test: `StateLoaded` + one pump tick yields exactly
  one `ParamChanged` per param and one `MatrixSnapshot`.
- [ ] VXN-1 (`cargo test --workspace`) still green — the shared-crate
  change must not regress the vxn-1 editor echo behaviour.
- [ ] ADR 0003 §Removed updated to note `push_matrix_snapshot`'s
  retained scope (the `RequestMatrixSnapshot` handler) — review found
  the ADR's removal note overbroad.

## Notes

Mid-drag host-automation suppression is wholly the view's job now
(`bindGestureGated`, ticket 0060) — the gesture suppression in
`handle_host` is bypassed by the pump regardless; if this ticket's
investigation finds it fully dead for VXN-2, fold its removal in.
