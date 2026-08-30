---
id: "0338"
product: vxn-1b
title: "Get the audio thread off the matrix mutex: topology edits ride an SPSC ring"
priority: high
created: 2026-08-30
epic: E049
depends: []
---

## Summary

vxn-1b's matrix topology lives behind a `std::sync::Mutex<[MatrixTable; 2]>`
with a `reload: AtomicBool` beside it
([shared.rs](../../vxn-1b/crates/vxn1b-engine/src/shared.rs)). The audio thread
takes that mutex:

```rust
// vxn1b-clap/src/lib.rs, in process()
if self.shared.params.take_reload() {
    self.engine.load_state(self.shared.params.engine_state());  // locks
}
```

The module doc justifies this as *"It changes only on state/preset load (main
thread), never per sample, so a lock the audio thread takes once when the
`reload` flag is set is cheap and RT-safe in practice."*

**That premise has drifted.** `edit_matrix_slot` sets the same `reload` flag, so
every topology change in the matrix overlay — every combo pick, every on/off
toggle — now routes the audio thread through the lock. It is no longer only
state/preset load.

Two problems, of different severity:

1. **Priority inversion.** A mutex on the audio thread is not a *cost*, it is a
   *risk*: if the editor thread is preempted while holding it, the render blocks
   and the dropout is audible. The window is small and gated, but "rare" is not
   "never", and this is the one place in either synth where the audio thread can
   block on another thread.
2. **The hammer is too big.** Any single-field edit triggers
   `load_state(engine_state())` — a full re-sync of both layers' params and
   topology — rather than applying the one field that changed.

vxn-2 has no equivalent exposure: its topology rides per-slot `AtomicU32` words
and the audio thread rebuilds the table from atomics every block, lock-free.

## Design

Adopt the two-channel model recorded in
[ADR 0003](../../adrs/0003-vxn-core-matrix.md) §4 rather than copying vxn-2's
approach wholesale:

- **Values** — depths and every other CLAP param — stay in the existing
  idempotent atomic store. They are latest-wins, they coalesce a knob drag for
  free, and the host needs `get_value` off the main thread regardless.
- **Topology** — source, dest, polarity, shape, scale source, scale bend,
  enabled — moves to an **SPSC ring** of the same `MatrixEdit { layer, slot,
  field, value }` records the UI already posts. The audio thread drains and
  applies; it never locks.
- **Bulk** — preset load and state restore keep the existing epoch/snapshot
  path. Pushing ~500 params plus 32 slots through a ring is the case a snapshot
  handles better, and it doubles as the ring's overflow backstop.

vxn-2's `vxn2-wasm` codec already carries a `MatrixEditEv` in a 16-byte slot
ring; vxn-1b's wasm path has the same shape. The native path is the one that
doesn't, which is the whole gap.

**State the overflow policy explicitly.** Topology edits are human-rate and the
existing web ring holds 1024 slots draining fully per render, so overflow should
be unreachable — but unreachable-by-argument is not the same as undefined. On a
full ring, set the resync flag and let the snapshot path carry it.

## Acceptance criteria

- [ ] No mutex is acquired on vxn-1b's audio thread. Grep `process()` and
      everything it calls; `Mutex`/`lock()` must not appear on that path.
- [ ] A single-field topology edit applies that field, not a whole-patch
      re-sync.
- [ ] Ring overflow has a defined, tested behaviour (resync flag → snapshot),
      not merely an argument that it cannot happen.
- [ ] Preset/state load still goes through the snapshot path, not the ring.
- [ ] The stale claim in `shared.rs`'s module doc is corrected — it currently
      asserts topology changes only on state/preset load, which stopped being
      true when `edit_matrix_slot` started raising the flag.
- [ ] Render-hash baseline unchanged. This is transport, not arithmetic.
- [ ] An edit posted from the UI is observable in the render on the **next**
      block, as now — the ring must not add a block of latency.

## Notes

- `priority: high` and `depends: []` — unlike the rest of
  [E049](../../epics/open/E049-shared-matrix-routing.md) this fixes a live
  real-time hazard rather than removing duplication, and it needs none of the
  extraction work first. It can land immediately and independently.
- The two-channel split is deliberately *not* "SPSC everywhere". Routing
  continuous parameter values through a queue would mean draining values that
  are immediately superseded, and would add a backpressure failure mode where
  today there is none. See ADR 0003 §4.
- Out of scope: changing vxn-2. Its topology-in-atomics works and is lock-free.
  It has its own reason to revisit the encoding eventually — the packed row word
  is exactly full — but that is not this ticket.
