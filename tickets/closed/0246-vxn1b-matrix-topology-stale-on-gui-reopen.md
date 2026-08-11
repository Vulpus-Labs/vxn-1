---
id: "0246"
product: vxn-1b
title: "Mod-matrix source/dest combos revert to the factory patch on GUI close/reopen"
priority: high
created: 2026-08-07
epic: E038
depends: []
---

## Summary

Close the plugin editor and reopen it: every mod-matrix row's
source/dest/curve/scale dropdown shows the **factory** routing again, not the
patch's. The depth dials are fine — they are CLAP params, so the host replays
them and the diff pump echoes them into the fresh page.

Topology is deliberately *not* a param (ADR 0001 §5 — it is patch state, carried
in the state blob and the `set_matrix` custom opcode), so nothing replays it.
The page's only source of topology is the `window.vxn.matrix` snapshot spliced
into the HTML at editor-open time, and
[`build_matrix_json`](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L208) built
that snapshot from `PluginState::factory_default()` — a constant. Every open
therefore seeded the combos from the factory patch;
[`matrix.js` `refreshForLayer`](../../vxn-1b/crates/vxn1b-ui-web/assets/panels/matrix.js#L246)
faithfully painted it, and the user's routes vanished from the UI.

The *engine* was never wrong — audio kept using the real topology from the
shared store, and the combos were only lying about it. Re-picking a source in a
lied-to row would then post a `set_matrix` that really did overwrite the route,
so the display bug could turn into data loss on the next edit.

## Design

Seed the page from the live store instead of the factory constant:

- `open_editor` takes `matrices: &[MatrixTable; 2]` and threads it through
  `build_faceplate_html` → `assemble_faceplate` → `build_matrix_json`.
- `gui::set_parent` passes `self.shared.params.matrix_snapshot()` — the same
  per-layer topology `state.save` serialises.
- The standalone web build keeps the factory table, because a browser session
  really does boot a fresh engine at the factory patch.

## Acceptance criteria

- [x] `build_matrix_json` serialises the topology it is handed; test seeds a
      route the factory patch does not have (L2 slot 7, Aftertouch → HpfCutoff,
      Exp curve, ModWheel scale) and asserts it survives into the spliced page.
- [x] `cargo test -p vxn1b-ui-web -p vxn1b-clap` green.
- [ ] **In a DAW:** wire a few routes, close the editor, reopen — the combos
      still read the patch. Save/reload the host project and check again.

## Notes

- **Known remaining gap (not fixed here):** a preset or host-state load *while
  the editor is open* still leaves the combos stale, because there is no
  engine→page topology echo — `ViewEvent` has `param_changed`,
  `key_mode_changed`, `preset_loaded`, … but nothing for matrix topology. Fixing
  it means a `kind: "matrix"` custom view payload (the `serialise_custom_view`
  hook already carries meter frames) plus a `dispatch.js` handler that re-seeds
  `window.vxn.matrix` and calls `matrixOverlay.refreshForLayer`. File as its own
  ticket if it bites before then.
- Same class of bug to watch for in any future non-automatable patch state: if
  it is not a CLAP param, opening the editor is the only chance to seed it.

## Close-out (2026-08-11)

- `build_matrix_json` serialises the topology it is handed rather than
  `factory_default()`
  ([ui-web/src/lib.rs:242](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L242)),
  threaded from `gui::set_parent`'s `matrix_snapshot()` through `open_editor` →
  `build_faceplate_html` → `assemble_faceplate`
  ([lib.rs:228](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L228)).
- Covered by `tests::matrix_json_carries_the_live_topology_not_the_factory`
  ([lib.rs:685](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L685)), which seeds a
  route the factory patch does not have and asserts it survives into the spliced
  page. Green.
- Standalone web build keeps the factory table — a browser session really does
  boot a fresh engine at the factory patch.
- The "known remaining gap" in the Notes (stale combos on a preset/state load
  under an *open* editor) was filed and fixed as
  [0247](0247-vxn1b-matrix-topology-echo.md).
- The Notes' id-collision warning is stale: the duplicate 0246 became
  [0251](0251-vxn1b-oversampling-and-limiter.md), so no renumbering is outstanding.
- Shipped in 8a403b0. Manual DAW verification waived by the user (2026-08-11).
