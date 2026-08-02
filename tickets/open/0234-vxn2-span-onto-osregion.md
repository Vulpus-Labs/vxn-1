---
id: "0234"
product: vxn-2
title: "vxn-2 span plumbing rewritten onto OsRegion (render hash must not move)"
priority: medium
created: 2026-08-02
epic: E042
depends: ["0233"]
---

## Summary

Second ticket of [E042](../../epics/open/E042-oversampled-region.md). Rewire
vxn-2's ~900 lines of span machinery onto the extracted `OsRegion`: the loose
fields (`decim_l/r`, `span_delay`, `os_l/r`/`bus_l/r` scratch, inline cosine
fade countdowns) become one `OsRegion`; the `OsSpan` FSM
([engine.rs:240](../../vxn-2/crates/vxn2-engine/src/engine.rs#L240)) +
`advance_os_span` stay in the engine verbatim as *policy driving* the region.
`DynamicsBlock` runs over `region.bus()` after the per-stack accumulate, in
all three legs (Filtered step 4b, the f==1 fallback, DynOnly's
`run_dynamics_os`).

## Acceptance criteria

- [ ] All three render legs (`render_block_filtered`, `render_block_off`,
      `render_block_filter_xfade`) and `run_dynamics_os` use the region; no
      loose span fields remain on `Engine`.
- [ ] vxn-2 render hash **unchanged** — the load-bearing check. If it moves,
      the refactor is wrong: fix, never recapture.
- [ ] filter_integration, dynamics_integration, note on/off click,
      filter-toggle declick, zipper_regression all green unmodified.
- [ ] `filter_path` + `stack` benches within noise; asm-check unchanged.

## Notes

The duplicated block-rate filter smoothing (`FILTER_SMOOTH_MS` one-pole,
verbatim in `render_block_filtered` :1718-1735 and
`render_block_filter_xfade` :2083-2100) may be deduped into one helper while
in here — must stay float-identical or the fade end-handoff pops.
