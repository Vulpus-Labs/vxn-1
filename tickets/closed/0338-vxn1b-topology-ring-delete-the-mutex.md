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

- [x] No mutex is acquired on vxn-1b's audio thread. Grep `process()` and
      everything it calls; `Mutex`/`lock()` must not appear on that path.
- [x] A single-field topology edit applies that field, not a whole-patch
      re-sync.
- [x] Ring overflow has a defined, tested behaviour (resync flag → snapshot),
      not merely an argument that it cannot happen.
- [x] Preset/state load still goes through the snapshot path, not the ring.
- [x] The stale claim in `shared.rs`'s module doc is corrected — it currently
      asserts topology changes only on state/preset load, which stopped being
      true when `edit_matrix_slot` started raising the flag.
- [~] Render-hash baseline unchanged. This is transport, not arithmetic.
      *vxn-1b had no baseline when this landed — 0329 captured it a day later.
      Verified instead by a scripted bit-identical before/after render; see the
      close-out.*
- [x] An edit posted from the UI is observable in the render on the **next**
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

## Close-out (2026-09-01)

**Implemented in `239e317` on 2026-08-30; the ticket was left open.** This
close-out is a verification pass against the acceptance criteria on today's
`main`, not a record of fresh work. Every claim below was re-checked rather than
read off the commit message.

- **No mutex on the audio thread.** `SharedParams` keeps exactly one — `matrix:
  Mutex<[MatrixTable; 2]>`, now the main thread's authoritative copy — and every
  one of its eight call sites is a main-thread CLAP entry point:
  `matrix_snapshot` (editor echo, `state.save`), `copy_layer` / `reset_layer` /
  `edit_matrix_slot` (`push_scope_frame`, the UI drain), `service_topology_resync`
  (`on_timer`, main-thread `flush`), `restore_from_bytes` (`state.load`), and
  `engine_state` (`activate`).
  - `activate` is the one that reads the guarded copy on the way *in*, which is
    correct — CLAP puts it on the main thread with no audio thread in existence —
    and the code says so, along with why it then publishes a snapshot *behind*
    any stale queued records rather than discarding them: superseding keeps the
    read cursor the audio thread's alone.
  - The `flush` split is the subtle one and it is right. CLAP's
    `clap_plugin_params.flush` runs on the **audio** thread while active; the
    locking one at [lib.rs:864](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L864)
    is `PluginMainThreadParams::flush`, and clack's audio-thread counterpart
    ([`PluginAudioProcessorParams`](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L884))
    touches nothing but the local mirror and the engine.
  - `sync_from_store` re-syncs via `state_with(engine.matrices())`, handing the
    store the engine's *own* tables rather than reading the guarded copy — that
    substitution is what makes the reload path lock-free rather than merely
    rarer.
  - `KeyState` went the same way in the same commit: from its own `Mutex` to a
    packed `AtomicU32`. It was the second lock on the render path and the AC
    would have been half-met without it.
- **Single-field edits stay single-field.** `TopoMsg::Edit` carries one
  `MatrixEdit { layer, slot, field, value }`; `drain_topology` applies it with
  `apply_edit` straight onto `engine.matrix_mut(layer)`. No allocation, no
  whole-patch rebuild. Covered by
  `a_single_field_edit_applies_that_field_and_nothing_else`, which also asserts
  the drain does *not* report a snapshot and that a topology edit raises no param
  reload.
- **Overflow is defined and tested, not argued.** A full ring drops the record
  and raises a sticky `resync` flag; the producer republishes the whole table as
  one `TopoMsg::Snapshot` once there is room, and suppresses individual edits
  while a resync is owed (the snapshot is taken after they hit the table, so
  pushing them too would be redundant). Four tests:
  `a_full_ring_refuses_the_push`, `the_resync_flag_is_sticky_until_cleared`,
  `a_full_ring_falls_back_to_the_snapshot_path` (which asserts convergence on
  exactly the store's topology, dropped edit included), and
  `edits_made_while_a_resync_is_owed_ride_the_snapshot`.
- **Bulk stays on the snapshot path.** `a_state_restore_crosses_as_one_snapshot_not_as_edits`
  pins one record for a whole load rather than 32 slot edits;
  `a_snapshot_leaves_depth_to_the_params` pins that a snapshot writes topology
  and leaves depth to the accompanying param re-sync (ADR 0001 §5 / 0205).
- **The stale doc claim is gone.** `shared.rs`'s header now states the mutex is
  one the audio thread never takes, names `edit_matrix_slot` as what invalidated
  the old justification, and describes the three channels (values / topology /
  bulk) explicitly.
- **No added latency.** `an_edit_posted_between_blocks_is_audible_in_the_next_one`
  is the direct test. Structurally, `drain_topology` is the first thing
  `sync_from_store` does and `sync_from_store` is the first thing `process` does,
  so an edit applies in the block that drains it.
- **The two-store race is handled in both directions.** A bulk change publishes
  as a snapshot push *then* a `reload` store, and `sync_from_store` is immune to
  landing between them either way: a drained snapshot implies the reload, and a
  reload seen after the drain triggers a second pop. `a_reload_seen_after_the_drain_still_finds_its_snapshot`
  and `activate_supersedes_records_older_than_the_state_it_adopts` cover the two
  halves.

### The one AC that could not be met as written

**vxn-1b had no render-hash baseline when this landed.** `tests/baseline.rs`
arrived the next day in `3d14b0a` (ticket 0329), so "render-hash baseline
unchanged" was unmeasurable at the time and cannot be reconstructed now — there
is no pre-0338 hash to compare against. What was done instead, per the commit
message: a scripted session (param edits, single-field topology edits on both
layers, a multi-edit drag, key ops, copy/reset layer, a state restore) rendered
**bit-identically** before and after — 1824 stereo frames, md5
`5e0b3658994b00aa0e9c9fd2d4ecfdae`. That is a stronger check than the hash for
this change, since it exercises the edit paths a fixed reference patch does not.
The baseline captured since has held unchanged through 0328 and 0330–0334.

### Verified on today's `main`

`cargo test -p vxn1b-engine -p vxn1b-clap` green (343 + 7 + 25 across the suites,
0 failures), including all eleven `topology::` / `shared::` tests named above and
the `baseline.rs` render-hash and null tests.

### Sizing note worth keeping

`TOPO_RING_SLOTS = 64`, not the web ring's 1024, and the reason is on the const:
`TopoMsg::Snapshot` (two 16-slot tables) dominates the enum, so a 1024-slot ring
would cost ~400 kB per plugin instance to buy headroom nothing can consume. 64
records is ~64 combo picks between two `process` calls. A `const {}` assert pins
the power-of-two, because the mask *is* the wrap and a non-power-of-two capacity
would alias cells the fullness guard believes are distinct — silent record
corruption rather than a test failure.

### Left open

- The web build shares `SharedParams` as a UI model but feeds its worklet through
  `vxn1b-wasm`'s own event codec, so nothing over there drains the ring: after
  enough edits it sits permanently full with a resync owed. Inert — the guarded
  tables the web build actually reads stay correct, and a full ring costs one
  refused push per edit — but it does mean `topology_backlog` and
  `topology_resync_pending` are meaningless off the CLAP path. Documented in
  `shared.rs`'s header rather than fixed; a real fix is a web-side drain, which
  is not this ticket.
