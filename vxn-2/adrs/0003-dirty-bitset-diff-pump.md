# ADR 0003 — Dirty-bitset diff pump for Model → View

- **Status:** Accepted
- **Date:** 2026-06-10
- **Scope:** Replaces the per-tick poll-and-diff in `collect_param_diffs`,
  removes the `host_changed` / `ui_changed` flag arrays in `LocalParams`,
  unifies every Model mutation source (UI, host automation, state load,
  future preset load) under one observation pump, extends coverage from
  CLAP `values` to all Model state including mod-matrix topology.

## Context

The view (the WebView page) is event-driven: it never reads the Model
directly, it consumes `ViewEvent`s from the Controller. The Model
(`SharedParams`) is mutated from three sources today, and one more
arrives with the preset epic:

| Source | Thread | Lands in | Today's view-notify path |
|---|---|---|---|
| UI knob / matrix edit | Main | `SharedParams.values` + `matrix_meta` | Controller echo (`MatrixRowChanged`, `ParamChanged`) |
| Host CLAP automation | Audio | `LocalParams` → `publish` → `SharedParams.values` | Main-thread tick polls `last_seen` vs current |
| Host state load | Main | `SharedParams.values` + `matrix_meta` | **No notify path** (bug — required bespoke push from `PluginStateImpl::load`) |
| Preset load (future) | Main | `SharedParams.values` + `matrix_meta` + future fields | TBD |

Three different bridges for the same underlying event ("Model field
changed"). Two failure modes are already shipping:

1. **Coverage gap.** `last_seen: Vec<f32>` only tracks `values`. The
   matrix topology (`matrix_meta`, `matrix_extra_depth`) is not
   CLAP-automatable, so it never made it into `last_seen`. State load
   updated `matrix_meta` and the view never heard — the rows that
   shipped audibly active showed as empty in the editor. Workaround was
   to add a bespoke `push_matrix_snapshot(&mut ctrl)` call inside
   `PluginStateImpl::load`. Every new non-CLAP field will reintroduce
   this bug class until the discipline is uniform.

2. **Coordination cost on echo.** The Controller's
   `Vxn2UiCustom::SetMatrixRow` handler writes the Model **and** emits
   a matching `MatrixRowChanged` view event. Every UI-write handler in
   `vxn2-app::controller` has to remember to echo. The audio thread's
   `LocalParams::publish` flag (`host_changed`) exists so the
   main-thread tick can re-emit host writes. Two flag arrays
   (`host_changed` + `ui_changed`) and one shadow snapshot (`last_seen`)
   coordinate three writers, when one mechanism would do.

The codebase already trusts the polling diff for 180 params at audio
rate. Cost is not the problem; coordination + coverage are.

## Decision

Adopt a single dirty-bitset pump as the only Model → View bridge.

Every write site to `SharedParams` follows the same two-step:

```rust
self.values[id].store(v.to_bits(), Relaxed);
self.dirty_values[id / 64].fetch_or(1u64 << (id % 64), Release);
```

(Pseudocode; `set()` and `set_matrix_row_raw()` encapsulate the pair.)

The main-thread tick is the sole reader:

```rust
for w in 0..N_VALUE_WORDS {
    let mut bits = self.dirty_values[w].swap(0, Acquire);
    while bits != 0 {
        let b = bits.trailing_zeros();
        let id = w * 64 + b as usize;
        emit(ParamChanged { id, plain: self.get(id), ... });
        bits &= bits - 1;
    }
}
// Matrix gets its own word (16 slots fits in u16, but a u64 leaves
// room for slot 9-16 depth bits in the same struct).
let m = self.dirty_matrix.swap(0, Acquire);
if m != 0 {
    emit(MatrixSnapshot { rows: snapshot_rows(self) });
}
```

**Properties:**

- **Dedup free.** N writes to one id coalesce into one set bit. Hot
  CLAP automation no longer sprays events at the view.
- **No overflow.** Bounded by id count (≤ 256 bits total for vxn2).
- **Audio-thread safe.** Two `Relaxed`-store ops per write, plus one
  `fetch_or(Release)`. No allocation, no lock, no bounded channel.
- **Memory ordering.** `fetch_or(Release)` pairs with `swap(Acquire)`:
  the reader's `get(id)` after popping the bit sees the value that the
  writer stored before setting the bit.
- **Coverage uniform.** Every shared field that needs to reach the view
  carries a dirty bit. Adding a field is: declare bit, set on write,
  read in pump. No echo discipline per call site.
- **Source-agnostic.** Audio thread, main-thread Controller, state load,
  preset load, future automation — all the same two-step write. The
  pump doesn't care who wrote.

### What dissolves

The following code disappears or simplifies:

- `LocalParams::host_changed: [bool; TOTAL_PARAMS]` — the dirty bit is
  the change marker. `publish()` stays as the "audio thread fanout to
  shared store" function but no longer threads a flag array.
- `LocalParams::publish` partial-write logic — once `publish()` is just
  "fanout that audio thread saw to shared", the partial-vs-bulk
  reasoning is encoded in the bitset, not in `host_changed`.
- `VxnMainThread::last_seen: Vec<f32>` — shadow snapshot deleted.
- `collect_param_diffs` — replaced with a bitset-walking pump that also
  covers matrix. Sync-pair re-emit (the `lfo1-sync` / `delay-sync`
  partner re-emit currently in `collect_param_diffs`) survives but
  moves out of the diff loop into an explicit "after dispatch" step,
  triggered by the sync-flag id appearing in the dirty walk.
- `Vxn2UiCustom::SetMatrixRow` handler's `MatrixRowChanged` echo
  ([vxn2-app/controller.rs:54-56](../crates/vxn2-app/src/controller.rs))
  — the bitset catches it on the next tick. Optimistic UI paint covers
  the one-tick latency.
- `Vxn2UiCustom::SetOpTab` handler keeps its `OpTabChanged` echo —
  that's a pure UI mode state with no Model backing, so it doesn't
  ride the bitset; the echo is the only path.
- The `mod-matrix.js dispatchRow` dual-write (depth widget fires both
  `set_matrix_row` AND `set_param mtxN-depth`) collapses to one path:
  slots 1-8 → `set_param` (rides CLAP automation gestures); slots 9-16
  → `set_matrix_row` (no CLAP id).
- The bespoke `push_matrix_snapshot(&mut ctrl)` in
  `PluginStateImpl::load` is deleted. State load writes through `set` /
  `set_matrix_row_raw` which flip dirty bits; next tick pushes.

### What survives

- **`gestures` bitset.** Drives CLAP `gesture_begin` / `gesture_end`
  events out to the host (different direction: plugin → host, not
  Model → View). Not used to suppress Model-to-View updates.
- **`ui_changed` flag (or equivalent).** Audio thread's `emit()` walks
  it to send param-value events back to the host for UI-originated
  changes. Different direction (plugin → host); the bitset only carries
  Model → View. A second bitset for "UI-originated, host needs to know"
  is the moral upgrade — same pattern, different consumer.
- **Mid-drag suppression.** Lives in the **view**, not the pump. The
  pump pushes everything that drifted; the bound widget checks "am I
  the activeElement / is my mousedown flag set" and drops or queues
  the incoming event. Pattern already in `paintRow`
  ([mod-matrix.js:202-204](../crates/vxn2-ui-web/assets/panels/mod-matrix.js#L202)) —
  gets lifted into a small bind-layer helper so every primitive
  inherits it.

### Namespace question

Matrix state lives outside `values` (separate `AtomicU32` arrays).
Decision: **single shared struct, multiple bitset words, one whole-table
`MatrixSnapshot` push.**

```rust
pub struct DirtyBits {
    /// One bit per CLAP id (180 → 3 × u64).
    pub values: [AtomicU64; (TOTAL_PARAMS + 63) / 64],
    /// One bit per matrix slot meta + one per slot 9-16 extra depth.
    /// Any bit set → push whole MatrixSnapshot.
    pub matrix: AtomicU64,
}
```

Whole-table push for matrix beats 16 row events: tables only have 16
rows, JSON-serialising all of them is cheaper than building 16 separate
`MatrixRowChanged` envelopes, and the view's `onSnapshot` repaint is
already idempotent.

Per-domain bitsets (one for values, one for matrix meta, one for matrix
extra depth) is the alternative — purer, but no observable benefit for
24 total slot bits. Re-evaluate if matrix grows to dozens of fields.

### Architectural framing

This locks in the unidirectional MVC the codebase is already
half-implementing:

```text
UI events  ─┐
            ├─► Controller ─► SharedParams ─► [dirty pump] ─► View events ─► View
Host autom ─┘                       ▲
                                    │
                              Audio thread (LocalParams::publish)
```

The Controller writes Model. The dirty pump observes Model. The view
receives events. The view never reads the Model. No echo bookkeeping per
write handler; no shadow snapshot; one discipline.

## Consequences

### Removed

- `VxnMainThread.last_seen: Vec<f32>` field and the NaN-seed initialisation.
- `collect_param_diffs` — the polling diff function and its tests, replaced
  with `drain_dirty_bits`.
- `LocalParams::host_changed` flag array.
- `Vxn2UiCustom::SetMatrixRow` handler's echo emission (the `MatrixRowChanged`
  view event push) — the bitset catches it next tick. (The event variant
  itself stays in `Vxn2ViewCustom` for the row-level repaint API; the snapshot
  may also emit per-row events in dev builds for diagnostic clarity.)
- The bespoke `push_matrix_snapshot` call inside `PluginStateImpl::load`
  (added in the load-time hotfix commit).
- The `mod-matrix.js dispatchRow` dual-dispatch on depth widget (only one
  path survives per slot range).

### Added

- `SharedParams.dirty_values: [AtomicU64; …]` + `SharedParams.dirty_matrix: AtomicU64`.
- `SharedParams::set` / `set_normalised` / `set_matrix_row_raw` flip the
  matching dirty bit after writing the value.
- `VxnMainThread::push_dirty_diffs` (or renamed `push_model_diffs`) —
  reads the bitsets, emits `ParamChanged` / `MatrixSnapshot` per drift.
- View-side bind helper (`bindGestureGated` or similar) — wraps every
  `set()` callback with the activeElement / drag-flag guard so primitives
  inherit mid-drag suppression uniformly.

### Kept

- `LocalParams::ui_changed` — different consumer (plugin → host
  `ParamValueEvent` emission), different direction. Survives in some form.
- `SharedParams.gestures` bitset — drives CLAP gesture brackets out to
  host. Read by `LocalParams::emit`, not by the dirty pump.
- `set_matrix_row_raw` dual-write to CLAP `values[OFF_MTX + slot]` for
  slots 1-8 (already there; just gains a `dirty_values` bit alongside
  the `dirty_matrix` bit).
- The `Vxn2UiCustom::SetMatrixRow` UI event itself — the page still
  needs a way to write topology. Only the *echo* on the response path
  goes away.

### Correctness

- **Ordering.** Writer: store value (`Relaxed`), then set bit
  (`Release`). Reader: clear bit (`Acquire`), then load value
  (`Relaxed`). Release/Acquire pair guarantees the reader sees the
  value written before the bit was set. Stronger orderings buy nothing
  for scalar param updates; this matches the existing `Relaxed` policy
  documented in `SharedParams`.
- **Race window — write between swap and load.** A writer flipping a
  bit AFTER the reader's `swap(0)` but BEFORE the reader's `get(id)`
  for that same id is fine: the swap returned 0 for that bit, the
  reader doesn't emit for this round, the writer's bit is now set,
  next tick emits with the latest value. No event lost.
- **Race window — write between value-store and bit-set.** Reader can
  observe an empty bitset between writer's `store` and writer's
  `fetch_or`. Reader skips this round. Next tick observes the set bit
  and reads the value (which is still the same — value was already
  stored). No event lost.
- **Coalescing semantics.** Multiple writes to the same id between
  two ticks emit one event carrying the latest value. This is what
  callers want (host doesn't need every intermediate, view doesn't
  paint per intermediate). The current `collect_param_diffs` already
  has this property; we're preserving it.

### Performance

- Audio thread: one extra `fetch_or(Release)` per `apply_input`. Memory
  contention is bounded by the bitset word, ~64 ids per word. Ample
  headroom vs the existing per-block atomic load fan-out.
- Main-thread tick: replaces N=180 plain reads + comparisons with M
  atomic swaps (M ≤ 4 words) + popcount-driven iteration over set bits.
  For typical UI workload (handful of changes/tick), strictly cheaper.
  For full-table broadcast (NaN-seed today, state load tomorrow),
  comparable.
- View side: serialisation work unchanged. WebView IPC is the cost
  floor; the pump's structure doesn't move that needle.

### Migration

Internal refactor — no on-wire format change (the blob format is
unaffected; this is purely the live Model → View pump). No deployed
patches affected.

### Documentation impact

- `vxn-core-app::Controller` doc: clarify that custom UI events should
  write the Model and skip echo emission unless they carry pure-UI
  state with no Model backing.
- `SharedParams` doc: document the dirty-bitset as the canonical change
  channel; the per-field writer pattern is the contract.

## Open questions

- **Do we extend the bitset to cover the audio-thread → host emit path
  too?** That's the `ui_changed` flag's job today. Same shape, same
  cost. Probably yes, but treat as follow-up: it's a separate consumer
  with its own semantics (gesture brackets, host-side dedup), and the
  Model → View pump alone is the high-value win.
- **Should `MatrixSnapshot` events carry only the changed rows?** Today
  it ships all 16. The bitset has per-slot resolution, so per-row push
  is achievable. Decision: stay with whole-table snapshot for now —
  serialisation cost is negligible at 16 rows; the simplification of
  the view-side handler (one render path, no merge logic) is worth more
  than the bandwidth saving.
- **Sync-pair re-emit (`lfo1-sync` flip re-emits `lfo1-rate`'s display).**
  This is a display-only concern that the polling pump handles inside
  `collect_param_diffs`. In the bitset pump it becomes: if a sync-flag
  id is in the dirty set, also emit its rate partner's current value
  (even though the partner's plain value didn't change). Same logic,
  different host.

## Tickets

Tracked under [E005 — Dirty-bitset diff pump](../epics/open/E005-dirty-bitset-pump.md).
