---
id: "0290"
product: vxn-1b
title: "vxn1b-web-controller: the main-thread controller wasm over a C-ABI opcode surface"
priority: medium
created: 2026-08-25
epic: E045
depends: ["0286", "0287"]
---

## Summary

Sixth ticket of [E045](../../epics/open/E045-vxn1b-web-wasm-browser-port.md): the
main-thread half of the model. A raw C-ABI `cdylib` — no wasm-bindgen — wrapping
the **same** `vxn_core_app::Controller<SharedParams>` that
[vxn1b-clap](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L186) drives, so there is
one arbiter for model mutation across native and web rather than two that can
disagree.

The engine wasm (0286) renders in the worklet; this one runs on the main thread.
They share the param SAB, not linear memory.

Everything built so far is transport. This is the first piece that owns *state*:
what a param means (descriptor taper, display strings), what presets exist, and
what the page is told when any of it changes. Ports `vxn2-web-controller`, which
is the closest relative — vxn-1's wraps its bespoke `vxn-app`, whereas both vxn-2
and VXN1b compose the shared `Controller` directly.

## Why this comes before the faceplate rewire

[0291](0291-vxn1b-faceplate-rewire.md) has nothing to talk to without it.
Concretely, three things are stubbed or absent today and all of them live here:

- `param-store.mjs`'s `paramChanged()` passes `plain` through as `norm` and
  stringifies it as `display`. Both are descriptor-derived and wrong for any
  tapered param; the page's readouts depend on them.
- There is no corpus JSON, so the preset browser has nothing to render.
- The preset corpus and every user-preset operation.

## Design

### Scope split with 0293

This ticket is the **Rust** side, including the store implementation: a
`WebPresetStore` over a baked factory bank plus an in-memory user cache with a
write journal (`user_store.rs`, ported from vxn-2's). The **JS** side —
IndexedDB, autosave, patch-io wiring over 0284's shared modules — is
[0293](0293-vxn1b-browser-persistence.md). The journal is the seam between them:
the controller mutates its cache synchronously and records what to persist, and
the JS drains it off the tick.

`EnginePresetStore` cannot be reused: it is `std::fs`, which on wasm compiles to
stubs that silently fail rather than to an error. The **record format** is reused
verbatim (`vxn1b-engine`'s sparse-TOML codec), because web and desktop must not
drift on what a preset file contains.

### VXN1b's custom opcodes

The native editor routes these through
[`parse_custom_op`](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L96). The web
controller needs the same vocabulary, but split by destination — and the split is
the interesting part:

| opcode | goes to | why |
|---|---|---|
| `set_key_mode`, `set_split_point`, `set_lfo2_link` | **both** | the model owns them for state + UI echo; the *engine* needs them too, and it is a separate wasm |
| `set_matrix` | **both** | ditto — topology is model state AND audio-path state |
| `copy_layer` | controller only | it rewrites params + topology in the model; the results reach the engine as ordinary param writes and matrix edits |
| `set_scope_tap` | ring only | pure audio-thread state, nothing for the model to remember |

Native gets the "both" cases free because one `SharedParams` is visible to both
threads. Here the controller updates its model and the **bridge** (0291) also
pushes the event onto the ring. This ticket exposes the controller half and
documents the pairing; 0291 wires it.

### Param-change detection: echo on, no bitset drain

vxn-2's controller is the structural reference — both it and VXN1b compose the
shared `Controller<SharedParams>` directly, where vxn-1 wraps its bespoke
`vxn-app`. But its *change-detection* must not be copied.

`vxn2-engine`'s `SharedParams` carries per-param **dirty bitsets**
(`take_dirty_values`), so its controller sets `echo_param_writes(false)` and
drains those bits once per tick as the single Model→View emitter.
`vxn1b-engine`'s `SharedParams` has **no value bitset at all** — only
`key_dirty` and `reload`.

So VXN1b leaves `echo_param_writes` at its default `true`
([controller.rs:125](../../crates/vxn-core-app/src/controller.rs#L125)), which is
what emits `ParamChanged` for every model write. Copying vxn-2's setup would
compile and then emit **nothing**: echo disabled, and a bitset drain with no bits
to drain.

The echo carries the display work with it. A sync toggle flipping does not change
its rate param's value, but it flips the readout between Hz/seconds and a
subdivision label, and the faceplate repaints only from what it is sent —
`vxn1b_engine::sync::{sync_aware_display, rate_partner_clap_id}` are public and
are what the native path uses for exactly this.

### There is no host automation here, and the readback is confirmation-only

vxn-1's native shell also runs a NaN-seeded `last_seen` diff over `SharedParams`,
because its `process()` writes params directly when the CLAP host automates them.
**The browser has no host**, and that diff has no analogue here: tracing the web
path, the only writer of the param SAB is the coordinator (`setParam`,
`setParamsBulk`, the defaults seed), and the worklet's `applyStoreToEngine`
publishes into the readback region exactly the values it just read out of the
store.

Two consequences:

1. This controller needs **no** value-diff poll. Every param value originates in
   the model, so the echo already covers it. A diff would only re-report what the
   controller just sent.
2. The readback region and `pollDiffs` in
   [param-store.mjs](../../vxn-1b/crates/vxn1b-wasm/web/param-store.mjs) are
   therefore **confirmation, not information** — they tell the main thread the
   worklet caught up, which nothing currently needs. Left in place (it is a
   cheap debugging affordance and keeps the three ports' stores the same shape),
   but 0291's bridge should not poll it, and `paramChanged()`'s `norm`/`display`
   stubs there become dead rather than needing a fix. Worth revisiting as its own
   call if it stays unused.

### Opcode surface

Follows vxn-2's `vxnc_*` naming so the two ports' JS glue stays recognisable:
construction, param set (plain + norm), gestures, editor-ready, tick + a
serialised `ViewEvent` batch, the values/readback pointers, factory bank load +
corpus JSON, the user-preset ops, journal drain, hydrate, state snapshot/restore,
and TOML export/import.

## Acceptance criteria

- [ ] `vxn-1b/crates/vxn1b-web-controller` exists as a `cdylib` in the workspace,
      builds for `wasm32-unknown-unknown --release`, 0 imports.
- [ ] Wraps `Controller<SharedParams>` — the same type `vxn1b-clap` drives — with
      no controller-logic fork.
- [ ] `vxnc_total_params()` agrees with the engine; a test asserts it.
- [ ] Param set by plain and by normalised value, with the descriptor taper
      applied on the norm path — proven against a tapered param, not a linear one.
- [ ] `ViewEvent` batches serialise and round-trip, including `ParamChanged`'s
      `norm` and `display` (the fields `param-store.mjs` currently stubs).
- [ ] `echo_param_writes` is left at its default `true`, and a test asserts a
      model write produces exactly one `ParamChanged` — not zero (vxn-2's setup,
      which has no bitsets to drain here) and not two.
- [ ] Flipping a sync toggle re-pushes its rate partner's display.
- [ ] VXN1b's custom opcodes are handled, and a test pins which are
      controller-only vs which the bridge must also put on the ring.
- [ ] `copy_layer` duplicates patch params and topology, leaving the mixer strip
      alone (matching `SharedParams::copy_layer`).
- [ ] Factory bank parses from baked bytes; corpus JSON lists it.
- [ ] User save/load/rename/delete/move + folder ops mutate the cache
      synchronously and journal the persistence op.
- [ ] State snapshot/restore and TOML export/import round-trip.
- [ ] A full re-broadcast reproduces every param plus the non-automatable state
      (the editor-attach path — a fresh page needs seeding).
- [ ] `cargo test -p vxn1b-web-controller` and `cargo test --workspace` green.

## Notes

- Reference: `vxn-2/crates/vxn2-web-controller/src/{lib.rs,user_store.rs}` (1590 +
  512 lines) — the closest relative. Port with the demo posture of [[0297]] in
  mind: this build's failure story is "reload the page", so anything there that
  exists to survive a long-lived DAW session is a candidate to leave out.
- **The echo-based change detection here is provisional.** Four things this
  ticket hand-builds — the explicit `broadcast_all_params()` after
  `restore_state` / `import_toml` / `copy_layer`, and the pack-time
  `sync_aware_display` recompute — exist only because `vxn1b-engine`'s
  `SharedParams` has no dirty bits. [[E046]] adds them; [[0303]] then deletes all
  four from this file. Comment them as such rather than as settled design.
- **No `factory.bin`.** vxn-1 baked one to keep the engine crate out of a lean
  controller wasm (ticket 0062); that reason does not transfer, since this crate
  links `vxn1b-engine` for `SharedParams` anyway, so `EnginePresetStore`'s
  filesystem-free factory half is already in the binary. VXN1b's bank is 8
  presets / 32 KB against vxn-2's 206 / 828 KB. Verified: the `include_dir!`
  strings survive linking into the release wasm, which comes out *smaller* than
  vxn-2's controller (773 KB vs 831 KB). 0292 therefore needs no bake step.
- `vxn1b-engine` exposes `sanitize_name` / `preset_filename` /
  `unique_folder_name`? vxn-2 had to make those `pub` for its web store; check
  and do the same rather than re-rolling them.
- No `cargo fmt` — [[vxn-no-cargo-fmt]]. One `cargo test` at a time —
  [[vxn-no-parallel-cargo-test]]. Stage explicit paths —
  [[vxn-concurrent-vxn2-work-no-git-add-all]].
- Blocks 0291.

## Close-out (2026-08-25)

- **Crate.** `vxn-1b/crates/vxn1b-web-controller` — `crate-type = ["cdylib", "rlib"]`
  (rlib so the tests link), registered in the workspace members + path deps.
  `cargo build --target wasm32-unknown-unknown --release`: **48 exports, 0 imports,
  772 762 bytes**. Deps are `vxn1b-engine` + `vxn-core-app` only.
- **One arbiter, no fork.** Wraps `Controller<SharedParams>`
  ([lib.rs:265](../../vxn-1b/crates/vxn1b-web-controller/src/lib.rs#L265)) — the same
  type `vxn1b-clap` constructs at
  [lib.rs:186](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L186). No controller logic
  copied; `apply_custom_ui` mirrors the native downcast chain.
- **`vxnc_total_params`** agrees with the engine and decomposes as
  `2 × PATCH_COUNT + GLOBAL_COUNT` — `tests::total_params_agrees_with_the_engine`.
  `vxnc_patch_count` added (not in vxn-2's surface): the JS side needs the layer
  split back for the two-layer map.
- **Taper on the norm path.** `set_normalized` → `from_fader`, not
  `from_normalized`. `tests::norm_path_applies_the_descriptor_taper` proves it on
  Cutoff (`Exp { mid: 800 }`) by asserting the value is >25 % off the *linear*
  midpoint, so a regression to the 0243 bug fails the test.
- **Echo left at default `true`.** `grep -c set_echo_param_writes` = **0** in the
  crate. `tests::set_param_surfaces_exactly_one_param_changed` asserts exactly one
  record — not zero (vxn-2's echo-off setup, no bits to drain here) and not two.
- **`norm` + `display` round-trip** — the two fields `param-store.mjs` stubs:
  `tests::view_batch_round_trips_norm_and_display`, which also asserts `display`
  is not a raw stringify.
- **Sync-aware display + partner refresh.** The echo's own string comes from
  `descriptor.display()` and is wrong for a synced rate, so
  [`pack_param_changed`](../../vxn-1b/crates/vxn1b-web-controller/src/lib.rs#L440)
  recomputes it via `sync_aware_display`, and the drain synthesises a partner
  record for any emitted sync flag. `tests::sync_flip_refreshes_its_rate_partner`
  asserts the partner surfaces, its label changes, and its **value** does not.
- **Three writes behind the Controller's back re-broadcast explicitly** —
  `restore_state`, `import_toml` and `PatchOp::CopyLayer` (which moves ~80 params
  through `SharedParams::set`). Covered by
  `tests::snapshot_restore_round_trips_and_rebroadcasts`,
  `tests::export_import_toml_round_trips` and
  `tests::copy_layer_duplicates_patch_and_topology_but_not_the_mixer_strip`, each
  asserting all `TOTAL_PARAMS` ids re-broadcast.
- **`copy_layer`** duplicates patch params + topology and leaves the mixer strip
  alone (same test asserts `LayerLevel` on the target is untouched), matching
  `SharedParams::copy_layer`'s `COPY_LAYER_EXCLUDED`.
- **Non-param echoes.** Matrix topology and key state have no view-side change
  flag, so both ride memo diffs ported from the native shell
  ([`push_matrix_echo`](../../vxn-1b/crates/vxn1b-web-controller/src/lib.rs#L560) /
  `push_key_echo`), re-armed on `take_editor_ready_flag`.
  `tests::first_tick_seeds_the_non_param_echoes_then_quiesces`,
  `tests::matrix_edit_reaches_the_model_and_echoes`,
  `tests::key_ops_reach_the_model_and_echo`.
- **Full re-broadcast on attach** — `tests::editor_ready_rebroadcasts_params_and_non_param_state`
  asserts all 185 params plus the matrix and key records.
- **User presets + folders.** Cache mutates synchronously and journals:
  `tests::user_save_journals_and_republishes_the_corpus`,
  `tests::user_save_load_rename_move_delete_round_trip`,
  `tests::folder_ops_mutate_the_cache_and_journal`, plus 11 in
  `user_store::tests`. Hydration replays without journalling
  (`tests::hydrate_seeds_the_cache_without_journalling`).
- **Record format reused verbatim**, per the ticket's non-drift requirement.
  `user_store::tests::stored_record_is_the_desktop_toml_format` parses the
  journal's bytes with the engine's own `read_preset` — not the store's decoder —
  and asserts the `PluginState` survives. `sanitize_name` / `preset_filename` /
  `unique_folder_name` reused from
  [preset_io.rs:34-72](../../vxn-1b/crates/vxn1b-engine/src/preset_io.rs#L34-L72)
  (already `pub`; the Notes' question answered — nothing needed exporting).
- **State + TOML round-trip**, with rejection paths pinned:
  `tests::restore_rejects_a_bad_blob_without_mutating` (also asserts a rejected
  restore emits nothing) and `tests::import_rejects_garbage_without_mutating`.
- **Tests green.** `cargo test -p vxn1b-web-controller`: 34 pass.
  `cargo test --workspace`: **101 suites, 1605 passed, 0 failed**, 5 ignored (all
  pre-existing deliberate diagnostics — the per-algo feedback dump, the two long
  audibility sweeps, an HTML dump, an `ftz` doctest).

### Two criteria that did not close as written

- **"Factory bank parses from baked bytes"** — **superseded, outcome met by a
  different mechanism.** There are no baked bytes: the store delegates its
  factory half to `EnginePresetStore`, whose factory methods touch no filesystem.
  vxn-1's reason for baking (0062: keep the DSP engine out of a lean controller
  wasm) does not transfer, because this crate links `vxn1b-engine` for
  `SharedParams` regardless. Verified rather than assumed — the `include_dir!`
  preset names are present in the release wasm, which is **smaller** than vxn-2's
  controller (772 762 vs 831 258 bytes) while carrying its bank inline; VXN1b's
  bank is 8 presets / 32 KB against vxn-2's 206 / 828 KB. The corpus JSON lists
  it at construction (`tests::factory_bank_is_embedded_and_listed_in_the_corpus`)
  and loading works (`tests::factory_load_rebroadcasts_and_reports`).
  **Consequence for [0292](0292-vxn1b-xtask-web-pipeline.md): no `bake-factory`
  step and no `factory.bin` in `dist/`.**
- **"A test pins which opcodes are controller-only vs which the bridge must also
  put on the ring"** — **half done.** The controller-side half is pinned:
  key ops and matrix edits reach the model, `copy_layer` is controller-only, and
  `tests::scope_op_is_ring_only_and_never_reaches_the_model` snapshot-compares the
  patch across a `ScopeOp` so a future refactor that routes the tap through the
  model fails loudly. The *ring* half cannot be tested here — this crate has no
  ring. The pairing is documented on each opcode; **[0291](0291-vxn1b-faceplate-rewire.md)
  must pin that the three "both" ops actually reach the ring.**

### Provisional by design

The sync-aware recompute, the partner synthesis and the three explicit
re-broadcasts exist only because `vxn1b-engine`'s `SharedParams` has no dirty
bits. [[E046]] adds them and [[0303]] deletes all four from this file; they are
commented as such in the source rather than as settled design.

### Not closed here

Browser verification (a real page loading a preset, copying a layer, importing a
patch) belongs to [0291](0291-vxn1b-faceplate-rewire.md) — nothing in this
ticket's criteria required it, and there is no JS consumer of these opcodes yet.
