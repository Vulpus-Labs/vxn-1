---
id: "0036"
title: CLAP shell — refit to flat IDs, drop layer demux
priority: medium
created: 2026-06-09
epic: E004
---

## Summary

Fourth ticket of [E004](../../epics/open/E004-single-layer-collapse.md).
Update the `vxn2-clap` shell to consume the flat parameter ID space
landed in **0035**. Drop every `upper-` / `lower-` ID lookup and the
demux logic that routed writes through a layer prefix.

Per [ADR 0002](../../adrs/0002-drop-dual-layer.md).

## Acceptance criteria

- [ ] `crates/vxn2-clap/src/lib.rs`:
  - [ ] All `upper-` / `lower-` ID lookups (e.g. the dual
        `lfo2-sync` lookup at lines 314-319) collapse to a single
        flat lookup.
  - [ ] Any `param_value()` / `set_param_value()` helper that
        prefix-demuxed by layer is simplified to a direct ID match.
  - [ ] The plugin's process loop reads matrix-depth params from a
        single flat `mtx_depths: [f32; 8]` slice (no `[upper][i]` /
        `[lower][i]` indexing).
- [ ] `crates/vxn2-clap/src/local.rs`:
  - [ ] `upper-op3-num` test references (line 239) flatten to
        `op3-num`.
  - [ ] Any other `upper-` / `lower-` constants flatten.
- [ ] `crates/vxn2-clap/tests/smoke.rs`:
  - [ ] Smoke fixture (lines ~1039-1048) no longer sets
        `lower-*` params (they don't exist).
  - [ ] No `voicing-mode` / `split-point` set calls.
- [ ] `crates/vxn2-clap/tests/editor_smoke.rs`:
  - [ ] Any iteration that walks `upper-*` + `lower-*` IDs walks the
        flat ID set.
  - [ ] Layer-mode assertions deleted.
- [ ] `cargo build -p vxn2-clap` green.
- [ ] `cargo test -p vxn2-clap` green.
- [ ] Manual host check: load the plugin in a CLAP-host smoke harness
      (the one in `crates/vxn2-clap/tests/smoke.rs` or the
      `clack-host` harness — whichever ships), enumerate params, confirm
      179 entries, no `upper-` / `lower-` prefix.

## Notes

The CLAP shell is the thinnest crate in the layer-rip — the engine and
param-table tickets do the structural work; this ticket just refits the
adapters. Expect ~50-100 LoC removed.

Sequence after 0035. If 0035 is in flight on the same branch the build
will be broken; the easy ordering is to land 0033 → 0034 → 0035 → 0036
as a four-PR train, each green at the boundary.

The shell's job is to map CLAP `clap_id` → param descriptor → engine
write. Once the param table is flat, the shell's only remaining concern
is the 1:1 mapping. Specifically:

- The 8 CLAP-exposed matrix-depth params (`mtx1-depth` …
  `mtx8-depth`) write directly into `SharedParams.mtx_depths[0..8]`.
  No layer indirection.
- The 154 per-patch params (6 ops × 21 fields = 126, plus 5 LFO2,
  5 mod env, 5 stacking, 3 assignment, 1 algo, 9 pitch EG) write
  directly into the flat `Patch`.

The `editor_smoke.rs` test that loads the HTML faceplate stays green if
the JS side has already been updated (or skipped). This ticket can
land before **0038** as long as the JS panel doesn't actively request
`upper-*` IDs that the shell no longer exposes — coordinate the merge
order with 0038 to keep the editor smoke test green throughout.

No new CLAP extension to wire. No state-extension format change beyond
the parameter ID renames (the state extension serialises by ID — the
flat IDs replace the prefixed ones, and the persisted-state format
diverges from any preview build, which is fine for pre-release).
