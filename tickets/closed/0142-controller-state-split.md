---
id: "0142"
product: vxn-1
title: vxn-web-controller — split ControllerState, dedup NaN-diff loop
priority: medium
created: 2026-06-23
epic: E027
---

## Summary

`vxn-1/crates/vxn-web-controller/src/lib.rs` (1736 lines, no
sub-modules) centres on one god struct plus a duplicated diff
loop. Behaviour-preserving.

1. **`ControllerState` god struct** (`lib.rs:421-467`) owns
   13 fields fusing a RAM mirror, a binary protocol layer, a
   diff pump, and JSON serialisation: the controller, the
   model arc, two channel ends, `values_out` / `last_seen`,
   seven staging byte buffers (`view_out`, `factory_in`,
   `corpus_json`, `arg_in`, `journal_out`, `state_out`,
   `toml_buf`), and two shared Arcs. Extract a
   `StagingBuffers` sub-struct (the seven byte buffers) and a
   `ParamMirror` sub-struct (`values_out`, `readback_in`,
   `last_seen`); `ControllerState` becomes ~three fields +
   channels. No cross-crate movement.

2. **Duplicated NaN-seeded diff** — `pump_readback`
   (`lib.rs:636-649`) re-implements the param diff loop that
   `vxn-app/src/diff.rs:24` (`diff_params`) already does; the
   web path silently omits the sync-toggle→rate-partner
   forced refresh. Extract a `NanDiff` helper into `vxn-app`
   taking a per-changed-slot callback; both sites delegate,
   and the web path opts into the sync-partner refresh in one
   line.

3. **Corpus-JSON rebuild scattered across 3 entry points**
   (`take_journal` / `load_factory` / `hydrate_done` all call
   `rebuild_corpus_json` inline; only the view-event drain
   dedups via `corpus_dirty` at `:607`). Add a
   `corpus_json_dirty` flag set by all paths; rebuild once at
   end of `tick`.

## Acceptance criteria

- [ ] `ControllerState` holds `StagingBuffers` + `ParamMirror`
      sub-structs instead of 12 flat buffer/mirror fields.
- [ ] One `NanDiff` helper in `vxn-app` is the single diff
      implementation; `pump_readback` and `diff_params`
      delegate to it; the web readback path performs the
      sync-partner refresh.
- [ ] `rebuild_corpus_json` is called at most once per `tick`
      via a single dirty flag; the three former call sites set
      the flag instead of rebuilding inline.
- [ ] `cargo test -p vxn-web-controller -p vxn-app` green;
      web preset-corpus and autosave/restore behaviour
      unchanged (existing tests pass; add one if a rename
      re-key path lacks coverage).

## Notes

`diff.rs`'s sync-toggle→rate-partner rule (Lfo2Sync→Lfo2Rate,
DelaySync→DelayTime) is vxn-1-specific and correctly lives in
`vxn-app` — keep `NanDiff` generic and the rule a vxn-1
caller opt-in; do **not** let it leak into `vxn-core-app`.
The duplicate factory/user Arc fields on `ControllerState`
(`:438-441`, also cloned into `WebPresetStore`) are a related
tangle — fold into the struct split if low-risk, else note
for follow-up. `static mut STATE` (`:655`) is fine for the
cdylib but flag the forward-compat note (`UnsafeCell` /
`OnceCell`) in the close-out; not required this ticket.

## Close-out (2026-06-30)

- **AC1** — `ControllerState` now holds `ParamMirror`
  ([lib.rs:425](../../vxn-1/crates/vxn-web-controller/src/lib.rs#L425):
  `values_out`/`readback_in`/`last_seen`) + `StagingBuffers`
  ([lib.rs:448](../../vxn-1/crates/vxn-web-controller/src/lib.rs#L448):
  the seven byte buffers), each with a `new()`. Flat
  buffer/mirror fields gone; struct is ctrl + model + 3
  channels + factory/user/corpus arcs + `mirror`/`staging` +
  `corpus_json_dirty`.
- **AC2** — single diff loop `vxn_app::nan_diff`
  ([diff.rs:24](../../vxn-1/crates/vxn-app/src/diff.rs#L24)),
  generic with a `sync_partner: bool` opt-in + per-slot
  callback. `diff_params` delegates (opt-in true); web
  `pump_readback`
  ([lib.rs:684](../../vxn-1/crates/vxn-web-controller/src/lib.rs#L684))
  delegates and now opts into the sync-toggle→rate-partner
  refresh it previously omitted. Rule stays a vxn-1 caller
  opt-in — does not leak into `vxn-core-app`.
- **AC3** — `rebuild_corpus_json` → `flush_corpus_json`
  ([lib.rs:619](../../vxn-1/crates/vxn-web-controller/src/lib.rs#L619)),
  gated on a single `corpus_json_dirty` flag. The drain sets
  the flag (no inline rebuild); `tick` flushes once at end.
  Boot opcodes `load_factory`/`hydrate_done` set+flush (JS
  reads `corpus_json` synchronously, not after a tick), so the
  rebuild still runs at most once per call site.
- **AC4** — `cargo test -p vxn-web-controller -p vxn-app`
  green (21 + 17). Added
  `tests::pump_readback_refreshes_sync_rate_partner` locking in
  the AC2 web sync-partner refresh. `cargo clippy -p vxn-app`
  clean.
- **Deferred (per Notes):** duplicate factory/user Arc vs
  `WebPresetStore` left in place — dedup needs a typed store
  accessor (downcast), higher-risk; comment marks it follow-up.
  `static mut STATE` `UnsafeCell`/`OnceCell` forward-compat not
  done (pre-existing clippy `deref_addrof` lints on the raw-ptr
  block, untouched) — follow-up.
