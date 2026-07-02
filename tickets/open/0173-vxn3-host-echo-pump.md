---
id: "0173"
product: vxn-3
title: "vxn-3 host echo pump — faceplate/p-lock writes → host params, alloc-free flush"
priority: medium
created: 2026-07-02
epic: E032
---

## Summary

Echo internally-originated param changes (faceplate edits + p-locks) back to the
host so automation lanes and generic-UI knobs track them — the reverse direction
of 0171's host→engine path. Adapt vxn-2's dirty-bitset pump. Must not oscillate
against host writes and must stay allocation-free on the audio thread.

Design: ADR 0003 §3 (p-locks compose with the host layer) + the vxn-2 dirty-bitset
pump pattern (`vxn2-clap`). Depends on 0171 (params + value cache).

## Design

- **Dirty bitset.** A per-param dirty flag set whenever an *internal* source
  (faceplate `SetMacro`/`SetGain`/…, or a p-lock resolving to a host-facing param)
  changes a host-facing value. On the param flush + `process()`, emit a
  `ParamValue` output event for each dirty id and clear it; call `request_flush`
  when dirty outside the process call.
- **No feedback loop.** Host-originated writes (0171) update the value cache but do
  **not** set the dirty flag — only internal changes echo. This is the vxn-2
  discipline; without it a host write ping-pongs. p-locks that momentarily move a
  param *do* echo (the host sees the automated value), reverting on latch release.
- **p-lock coverage.** p-locks (0050) may target macro + mix params; when a lock is
  active on a host-facing param, its resolved value is what echoes, so a host
  automation lane and a p-lock agree on the displayed value.
- **RT discipline.** The flush walks a fixed bitset + fixed value array and pushes
  into the host's output event buffer — no allocation. Extend the existing
  alloc-trap test in
  [vxn3-clap/src/lib.rs:291](vxn-3/crates/vxn3-clap/src/lib.rs#L291) to cover a
  block that dirties + flushes params.

## Acceptance criteria

- [ ] A faceplate edit to a mix/macro param emits a host `ParamValue` output event
      and `request_flush`; the host's automation lane / generic UI reflects it.
- [ ] A p-lock on a macro / mix param round-trips: the resolved per-step value
      echoes to the host and reverts on latch release.
- [ ] Host-originated writes do **not** re-echo (no feedback oscillation) —
      unit/integration tested.
- [ ] Param flush is allocation-free (alloc-trap test extended and green).
- [ ] `cargo test -p vxn3-clap` green.

## Notes

- The echo reads the same value cache 0171 owns; the dirty bitset lives beside it.
- Gesture begin/end (`ParamGestureBegin/End`) for faceplate drags is optional
  polish — note it but don't block the ticket on host-specific gesture handling.
