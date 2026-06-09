---
id: E004
title: VXN2 single-layer collapse
status: closed
created: 2026-06-09
---

## Goal

Implement [ADR 0002 — Drop dual-layer voicing](../../adrs/0002-drop-dual-layer.md):
rip Whole / Layer / Split voicing and the Upper/Lower parameter pair out of
VXN2 across engine, params, CLAP shell, app, UI, and docs. After this epic
closes, a patch is a single parameter set, the CLAP table is **180 IDs**
(down from 345), and the JS panel layer no longer threads `editLayer`
through panel binding.

## Scope

**In:**

- Engine kernel — delete `voicing.rs`, flatten `Patch`, drop layer-aware
  dispatch from `alloc.rs` + `engine.rs`.
- Matrix — collapse `PatchMatrix` to a single `MatrixTable`, drop `Layer`
  enum + `stack_layer()`.
- Params — flatten the CLAP ID space (drop `upper-` / `lower-`), drop
  `voicing-mode` + `split-point`, rebuild the macro that generates the
  param table.
- CLAP shell — refit `vxn2-clap` to flat IDs, drop layer demux.
- App + UI — drop `Layer` enum, edit-layer toggle, voicing-mode picker,
  split-point control; flatten panel-binding param lookups.
- Docs — rewrite `PARAMETERS.md`, supersede ADR 0001 §8, archive
  ticket 0009, prune README + `ui-mockup/index.html`.

**Out (explicit non-goals):**

- Replacing Layer with any other multi-voice mechanism. The mod matrix +
  algorithm space is the sound-design surface (per ADR 0002 rationale).
- Migration shims for "old patches". No patches exist in the wild.
- Reworking the mod matrix shape beyond removing the upper/lower split
  in storage. Slot count, source set, dest set unchanged.
- Reworking stacking. `stack_density` etc. were per-layer in name only;
  they become per-patch with the same defaults.

## Tickets

- [x] [0033 — Engine: collapse `Patch`, delete `voicing.rs`](../../tickets/closed/0033-engine-collapse-patch.md)
- [x] [0034 — Matrix: flatten `PatchMatrix`, drop `Layer` enum](../../tickets/closed/0034-matrix-flatten.md)
- [x] [0035 — Params: flatten CLAP ID space (drop `upper-` / `lower-`)](../../tickets/closed/0035-params-flatten-ids.md)
- [x] [0036 — CLAP shell: refit to flat IDs](../../tickets/closed/0036-clap-shell-flat-ids.md)
- [x] [0037 — App + events: drop `Layer` enum and edit-layer events](../../tickets/closed/0037-app-events-drop-layer.md)
- [x] [0038 — UI: drop voicing-mode + edit-layer, flatten param binding](../../tickets/closed/0038-ui-drop-voicing.md)
- [x] [0039 — Docs: rewrite `PARAMETERS.md`, supersede ADR 0001 §8, archive 0009](../../tickets/closed/0039-docs-and-archive.md)

## Dependency order

```text
0033 (engine kernel) ──┬─> 0034 (matrix flatten) ──┐
                       │                            ├─> 0036 (CLAP shell) ──> 0038 (UI) ──> 0039 (docs)
                       └─> 0035 (params flatten) ──┘
                                                    │
                                                    └─> 0037 (app + events)
```

- **0033** is the foundation: removing `voicing.rs` and flattening `Patch`
  forces every downstream module to compile against the new shape.
- **0034** (matrix) and **0035** (params) are independent of each other
  but both depend on 0033 — they each pick up the now-illegal `Patch.lower`
  references and flatten their own surface.
- **0036** (CLAP shell) consumes the flat ID space from 0035 and the
  flat matrix from 0034.
- **0037** (app + events) consumes the flat engine surface from 0033
  and is parallel-safe with 0036.
- **0038** (UI / JS) depends on the flat events from 0037 and the flat
  param IDs from 0036.
- **0039** (docs) lands last: PARAMETERS.md totals come from the actual
  flat table after 0035 ships, ADR 0001 §8 supersession is timestamped
  with the merge.

Each ticket lands as one PR. Tests stay green at every ticket boundary —
no half-state where the engine compiles but the CLAP shell doesn't, or
the JS still requests upper-* IDs from a flat table.

## Acceptance

- `cargo build --workspace` + `cargo test --workspace` green at HEAD.
- `cargo bench --workspace` runs to completion (no kernel regressions
  from removing the layer dispatch — single-layer was the fast path).
- `vxn2-clap` exposes 180 params (163 per-patch + 17 patch-level
  globals). No `upper-` or `lower-` ID prefix anywhere in the codebase.
- `Patch` struct is one parameter set. `voicing.rs` is deleted.
  `PatchMatrix` is deleted. `Layer` enum is deleted.
- The HTML faceplate (`vxn2-ui-web/assets/index.html`) has no
  voicing-mode group, no edit-layer toggle, no split-point control.
- The mockup at `ui-mockup/index.html` matches the production faceplate
  (no voicing UI).
- `PARAMETERS.md` recounts as 180 CLAP IDs; "per-layer vs patch-level"
  section deleted.
- ADR 0001 §8 carries a "superseded by ADR 0002" forward-note.
- Closed ticket 0009 carries a "superseded by ADR 0002 / E004" marker
  at the top.
- No regression in the saved-patch loader: a stub TOML at the new flat
  shape round-trips through `Patch::load`/`Patch::save`.
