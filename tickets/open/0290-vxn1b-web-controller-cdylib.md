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
- [0289](0289-vxn1b-worklet-coordinator.md) deliberately left a gap: a render
  trap rebuilds the engine and loses every piece of non-automatable state. The
  re-broadcast has to come from whoever holds the authoritative model — this.

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

### Param-change detection follows vxn-1, not vxn-2

vxn-2's controller is the structural reference — both it and VXN1b compose the
shared `Controller<SharedParams>` directly, where vxn-1 wraps its bespoke
`vxn-app`. But its *change-detection* must not be copied.

`vxn2-engine`'s `SharedParams` carries per-param **dirty bitsets**
(`take_dirty_values`, 20 references), so its controller disables auto-echo and
drains those bits once per tick. `vxn1b-engine`'s `SharedParams` has **no value
bitset at all** — only a `key_dirty` flag and the `reload` flag. VXN1b detects
param drift the way vxn-1 does: auto-echo on for UI writes, plus a main-thread
diff against a NaN-seeded `last_seen` mirror for audio-thread automation
([vxn1b-clap `push_param_diffs`](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L303)).

Porting vxn-2's drain here would not fail to compile — it would fail to *notice*
that half its inputs are missing, and the page would go quiet on host automation
while looking fine under UI edits.

The diff must also carry the sync-partner refresh: a sync toggle flipping does
not change its rate param's value, but it flips the readout between Hz/seconds
and a subdivision label, and the faceplate repaints only from what it is sent.
`vxn1b_engine::sync::{sync_aware_display, rate_partner_clap_id}` are already
public and are what the native path uses.

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
- [ ] Audio-thread param drift surfaces via the NaN-seeded diff, not a dirty
      bitset vxn1b-engine does not have — with a test that writes the store
      directly (the automation path) and asserts a `ParamChanged` appears.
- [ ] Flipping a sync toggle re-pushes its rate partner's display.
- [ ] VXN1b's custom opcodes are handled, and a test pins which are
      controller-only vs which the bridge must also put on the ring.
- [ ] `copy_layer` duplicates patch params and topology, leaving the mixer strip
      alone (matching `SharedParams::copy_layer`).
- [ ] Factory bank parses from baked bytes; corpus JSON lists it.
- [ ] User save/load/rename/delete/move + folder ops mutate the cache
      synchronously and journal the persistence op.
- [ ] State snapshot/restore and TOML export/import round-trip.
- [ ] A full re-broadcast reproduces every param plus the non-automatable state —
      the trap-recovery path 0289 left open.
- [ ] `cargo test -p vxn1b-web-controller` and `cargo test --workspace` green.

## Notes

- Reference: `vxn-2/crates/vxn2-web-controller/src/{lib.rs,user_store.rs}` (1590 +
  512 lines) — the closest relative.
- `vxn1b-engine` exposes `sanitize_name` / `preset_filename` /
  `unique_folder_name`? vxn-2 had to make those `pub` for its web store; check
  and do the same rather than re-rolling them.
- No `cargo fmt` — [[vxn-no-cargo-fmt]]. One `cargo test` at a time —
  [[vxn-no-parallel-cargo-test]]. Stage explicit paths —
  [[vxn-concurrent-vxn2-work-no-git-add-all]].
- Blocks 0291.
