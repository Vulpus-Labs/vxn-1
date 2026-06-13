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

## Close-out (2026-06-13)

**Investigation:** vxn-core-app is shared by vxn-1 and vxn-2. vxn-2's CLAP shell
runs the dirty-bitset pump (`push_model_diffs` / `drain_dirty_bits`) as the
single Model→View emitter. vxn-1's shell has *no* pump but **does** have its own
value-diff poll (`push_param_diffs`, `last_seen` vector) — its own code comment
notes the controller echo + diff poll already double-emit and the WebView
dedupes on the wire. So the echo is redundant in vxn-1 too (option 2 was
viable), but vxn-1's poll has no gesture gating and relies on the controller's
echo timing/gesture-suppression for the host-automation path. Chose **option 1
(config flag)** for minimal blast radius: vxn-1 behaviour is byte-identical.

**Change:** `Controller::echo_param_writes: bool` (default `true`), setter
`set_echo_param_writes`. Gates the two named echo paths only — `SetParam`/
`SetParamNorm` `emit_param_changed` and `StateLoaded` `broadcast_all_params`.
vxn-2 sets it `false` at both construction sites. `SetOpTab`,
`RequestMatrixSnapshot`, preset/corpus, and the gesture-gated `ParamAutomation`
echo are untouched. vxn-2 never routes `HostEvent::ParamAutomation` through the
controller (host param events fold into the audio thread + pump), so that path
is already inert for vxn-2 — nothing to fold in per Notes.

**Tests (vxn2-clap):** `ui_set_param_emits_exactly_one_param_changed` and
`state_load_emits_one_param_changed_per_param_and_one_snapshot` assert counts
across both channels. `cargo test --workspace` green (vxn-1 included). ADR 0003
§Removed corrected: `push_matrix_snapshot` retained for `RequestMatrixSnapshot`;
new bullet documents the echo gating.
