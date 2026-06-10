---
id: E005
title: Dirty-bitset Model→View diff pump
status: closed
created: 2026-06-10
---

## Goal

Implement [ADR 0003 — Dirty-bitset diff pump](../../adrs/0003-dirty-bitset-diff-pump.md):
replace the per-tick polling diff in `collect_param_diffs` with a
write-site dirty-bitset on `SharedParams`. Unify every Model mutation
source (UI write, host CLAP automation, host state load) under one
observation pump. Extend coverage from CLAP `values` to all Model state
including mod-matrix topology so the next non-CLAP field (preset name,
voice-alloc state, anything the preset epic touches) inherits the
discipline without a bespoke push path.

After this epic closes the Controller no longer echoes view events from
its UI handlers, `LocalParams.host_changed` and `VxnMainThread.last_seen`
are gone, `PluginStateImpl::load`'s bespoke `push_matrix_snapshot` is
gone, and every shared field reaching the view does so by flipping a
dirty bit that the main-thread tick drains.

## Scope

**In:**

- `SharedParams` — add `dirty_values: [AtomicU64; N_WORDS]` and
  `dirty_matrix: AtomicU64`. Wire every `set` / `set_normalised` /
  `set_matrix_row_raw` write site to flip the matching bit.
- `LocalParams` — delete `host_changed`. Audio thread's `apply_input`
  writes through to shared (same as today's `publish`) but the bit
  flip replaces the flag. `ui_changed` stays (it serves the
  plugin → host direction, not the Model → View pump).
- `vxn2-clap` main-thread tick — `collect_param_diffs` → `drain_dirty_bits`.
  Reads the bitsets, emits `ParamChanged` per popped value bit and one
  `MatrixSnapshot` if any matrix bit was set. Preserves sync-pair
  re-emit (lfo1-sync flips → re-emit lfo1-rate display).
- `vxn2-clap` `PluginStateImpl::load` — delete the bespoke
  `push_matrix_snapshot` call added in the hotfix. State load writes
  through `set` / `set_matrix_row_raw` → bits flip → pump catches.
- `vxn2-app::controller` — drop the `MatrixRowChanged` echo from the
  `Vxn2UiCustom::SetMatrixRow` handler. UI write goes through to Model;
  pump echoes on next tick. Keep the `OpTabChanged` echo on
  `SetOpTab` — that's pure UI state, no Model backing.
- `vxn2-ui-web/assets/main.js` — delete the `mtxN-depth` `param_changed`
  sync hack (added in the load-time hotfix) — the pump handles it
  directly via the `MatrixSnapshot` whole-table push, AND the
  `param_changed` for `mtxN-depth` also fires (depth lives in `values`),
  so the slider follows automation through the standard primitive bind.
- `vxn2-ui-web/assets/panels/mod-matrix.js` — collapse the dual-write
  in `dispatchRow`. For a depth-only edit on slots 1-8 fire `set_param`
  only (rides CLAP automation + gesture brackets to the host); for
  slots 9-16 fire `set_matrix_row` only (no CLAP id). Topology edits
  always fire `set_matrix_row`.
- View bind helper — extract the `document.activeElement` /
  drag-flag guard into `bindGestureGated` so every primitive inherits
  mid-drag suppression instead of repeating the check ad hoc. (The
  pattern at [mod-matrix.js:202-204](../../crates/vxn2-ui-web/assets/panels/mod-matrix.js#L202)
  is what gets lifted.)
- Docs — `SharedParams` doc comment describes the bitset as the
  canonical change channel; ticket entries cite ADR 0003.

**Out (explicit non-goals):**

- Plugin → host emit pump (`LocalParams::emit`). Different direction,
  different consumer (gesture brackets, host-side dedup). Stays on
  `ui_changed` for now; ADR 0003 §"Open questions" flags it as a
  follow-up.
- Per-row `MatrixRowChanged` for matrix drift. Stay with whole-table
  `MatrixSnapshot` push — 16 rows is cheap, the view-side renderer
  already collapses to one path. Re-evaluate if matrix grows.
- `gestures` bitset semantics. Survives unchanged as the
  plugin → host gesture-bracket signal.
- Preset epic scope (E007). The bitset's matrix coverage makes the
  preset-load path uniform when E007 lands, but no E007 work in this
  epic.
- Hard real-time guarantees beyond what `SharedParams` already
  documents (`Relaxed` for values, `Release` / `Acquire` pair for the
  bit handshake).

## Tickets

- [x] 0055 — `SharedParams`: add dirty bitsets + wire write sites
- [x] 0056 — `LocalParams`: drop `host_changed`, audio writes go
  through dirty
- [x] 0057 — Main-thread tick: replace `collect_param_diffs` with
  `drain_dirty_bits`
- [x] 0058 — Drop bespoke pushes + echoes:
  `PluginStateImpl::load`, `Vxn2UiCustom::SetMatrixRow` echo,
  `main.js` mtxN-depth sync hack
- [x] 0059 — `mod-matrix.js dispatchRow`: collapse dual-write per
  slot range
- [x] 0060 — View bind helper: extract `bindGestureGated`,
  retrofit existing primitives

## Dependency order

```text
0055 (bitset on Model) ──┬─> 0056 (LocalParams cleanup)
                         ├─> 0057 (tick pump)
                         │
                         └─> 0058 (drop bespoke pushes / echoes) ──> 0059 (mod-matrix.js)
                                                                  └─> 0060 (bind helper)
```

- **0055** is the foundation: the bitset has to exist on
  `SharedParams` and every write site has to flip a bit before any
  reader can rely on it.
- **0056** (LocalParams) is independent of 0057 — the audio-thread
  side and the main-thread pump both consume 0055's contract.
- **0057** (tick pump) replaces the existing `collect_param_diffs`.
  Tests transition from "diff against last_seen" to "pop bits".
- **0058** can only land after 0057: removing the bespoke
  `push_matrix_snapshot` requires the pump to cover matrix drift.
  Removing the `SetMatrixRow` echo requires the pump to emit on the
  same tick the UI write happens (or the optimistic paint covers the
  one-tick latency — verify in 0058's manual test).
- **0059** and **0060** are pure view-side cleanups, sequencable in
  parallel after 0058. 0059 simplifies the depth widget; 0060 lifts
  the activeElement guard into shared infra.

Tests stay green at every ticket boundary. No half-state where some
write sites flip bits and others don't — 0055 covers all of them
together.

## Acceptance

- `cargo build --workspace` + `cargo test --workspace` green at HEAD.
- `cargo bench --workspace` runs to completion. No regression in
  the kernel hot path (the dirty bit is one `fetch_or` per write, not
  per block; per-block render is untouched).
- `VxnMainThread.last_seen` field deleted. `collect_param_diffs`
  deleted. `LocalParams.host_changed` deleted.
- `PluginStateImpl::load` body is back to its pre-hotfix shape: read
  blob, call `load_bytes`, return. No `push_matrix_snapshot` call.
- `Vxn2UiCustom::SetMatrixRow` handler in `vxn2-app::controller` no
  longer pushes a `MatrixRowChanged` view event after the model write.
- `vxn2-ui-web/assets/main.js` — the `mtxN-depth` regex sync block
  added in the hotfix is removed. (`param_changed` for `mtxN-depth`
  still routes through the standard primitive bind for the depth slider;
  the matrix overlay's whole-row state arrives via the per-tick
  `MatrixSnapshot` push.)
- `dispatchRow` in `mod-matrix.js` no longer fires two opcodes for
  a depth edit. One opcode per slot range, documented.
- `bindGestureGated` helper exists in `main.js` (or a shared module)
  and is used by every bound primitive whose `set()` could fight a
  live drag.
- Manual test: open Reaper with a saved patch that has matrix routes;
  open the mod-matrix overlay; verify the rows render with the saved
  topology *before* the user opens the overlay (no need for the
  overlay-open re-request to mask a missing snapshot).
- Manual test: bind a host automation lane to `mtx1-depth`; verify
  the matrix overlay slider follows the host automation without the
  `main.js` sync hack.
- Manual test: drag a knob; verify mid-drag the host's own automation
  echo doesn't fight (the bind-layer gesture gate drops incoming
  `param_changed` for the active element).

## Notes

The hotfix landed in the preceding work — bespoke `push_matrix_snapshot`
in `PluginStateImpl::load` and the `mtxN-depth` regex sync in `main.js`.
Both ship as production behaviour until this epic deletes them. Either
this epic supersedes the hotfix cleanly, or a partial landing leaves
both pumps active (acceptable as an interim; the bitset push is
idempotent with the bespoke push). Don't ship a state where the hotfix
is removed but the bitset doesn't yet cover matrix — that reintroduces
the original bug.
