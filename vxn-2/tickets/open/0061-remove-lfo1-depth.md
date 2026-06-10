---
id: "0061"
title: "Remove lfo1-depth: param table, snapshot, UI fader, blob v4 migration"
priority: high
created: 2026-06-10
epic: E006
depends: []
---

## Summary

First ticket of [E006](../../epics/open/E006-review-remediation.md).
The review found `lfo1_depth` is snapshotted
([shared.rs:774](../../crates/vxn2-engine/src/shared.rs#L774)) but never
multiplied into the LFO1 signal — `PatchSources::from_modblock`
([matrix.rs:477-483](../../crates/vxn2-engine/src/matrix.rs#L477)) copies
`mb.lfo1` raw. Design decision: **remove the param, don't wire it.**
Per-route send amplitude is the mod matrix depth column's job; a global
LFO1 depth macro is redundant. LFO1 enters the matrix at full scale.

## Scope of removal

- `params.rs:505` — `fl("lfo1-depth", ...)` table entry. Param count
  drops 180 → 179; LFO1 section becomes shape/rate/sync. Update the
  module-routing test expectations and any `PATCH_BASE + OFF_*`
  arithmetic that assumed 4 LFO1 entries.
- `shared.rs` — `EngineParams.lfo1_depth` field (line 697), its
  default (line 732), the `snapshot_from` read (line 774), and the
  `OFF_LFO1 + 2` offset shift for `lfo1-sync` and everything after.
- `modulation.rs:75` — the doc comment promising depth multiplication
  at source-eval time.
- `index.html:89` — the `data-vxn-param="lfo1-depth"` fader and its
  label markup. Check `main.js` bind tables for any by-name reference.
- `PARAMETERS.md:78` — the `lfo1_depth` row; adjust the param-count
  totals (163 per-patch / 180 total become 162 / 179).

## Blob migration

Removing a mid-section param shifts every offset after `OFF_LFO1 + 1`.
Bump `BLOB_VERSION` to 4. `load_bytes` for `version <= 3` must skip the
stored lfo1-depth value and remap old offsets to new. Follow the
existing v2→v3 migration pattern in
[shared.rs:558-575](../../crates/vxn2-engine/src/shared.rs#L558).

## Acceptance criteria

- [ ] `id_of("lfo1-depth")` returns `None`; param count constant is 179
  and all `params.rs` tests pass against it.
- [ ] No `lfo1_depth` symbol remains anywhere in `vxn-2/crates/`
  (grep clean, comments included).
- [ ] A v3 blob (fixture: snapshot taken at current HEAD with
  non-default values either side of the removed param) loads under v4
  code: lfo1-depth value silently dropped, `lfo1-sync` and all later
  params land on their correct new offsets. Round-trip test added
  alongside the existing v2→v3 test.
- [ ] Faceplate renders without the LFO1 Depth fader; the
  `faceplate_html_shell_meets_0025_acceptance` test updated.
- [ ] PARAMETERS.md totals consistent (162 per-patch + 17 patch-level
  = 179).
- [ ] `cargo test --workspace` green.

## Notes

The matrix `Lfo1` source semantics change implicitly: routes that
relied on default depth 0.30 now receive full-scale LFO1. The default
patch's matrix depths may need re-tasting — check
`default_patch.rs` for routes sourced from LFO1 and scale their slot
depths by 0.30 to preserve the shipped sound.
