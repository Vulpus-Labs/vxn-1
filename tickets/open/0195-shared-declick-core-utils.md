---
id: "0195"
product: monorepo
title: "Extract declick raised-cosine weight into vxn-core-utils; de-dup vxn-2's inline copy"
priority: low
created: 2026-07-21
---

## Summary

The equal-gain raised-cosine crossfade weight `0.5 − 0.5·cos(π·t)` now exists in
**three** places across the workspace, all identical in intent:

- `vxn-1/crates/vxn-engine/src/smoothing.rs` — `raised_cosine_rise(t)` +
  `BypassXfade` (the clean, reusable form landed by [[E035]] / 0190).
- `vxn-2/crates/vxn2-engine/src/engine.rs` — inline in
  `render_block_filter_xfade` (`0.5 - 0.5 * (PI * t).cos()`), plus a second
  inline copy in the OS-crossfade path (`advance_os_span` neighbourhood).

`crates/vxn-core-utils/src/smoothing.rs` already exists and already exports the
neighbouring `ms_to_samples` / `one_pole_coeff` — so vxn-1's engine even carries
a *second* `ms_to_samples` that duplicates the core one. This is the natural home
for the shared declick primitive.

Origin: follow-on from [[E035]] close-out (2026-07-21). Lineage: [[E027]]
shared-primitive dedup.

## Design

Move into `crates/vxn-core-utils/src/smoothing.rs`:

- `pub fn raised_cosine_rise(t: f32) -> f32` — the pure scalar weight (zero slope
  at both endpoints). Single source of truth.
- `pub struct BypassXfade` — the deterministic edge-armed dry↔wet countdown
  (`arm` / `active` / `weights_at` / `prime` / `advance`). It has no vxn-1
  specifics; it's a generic bypass crossfade any master-FX chain can own.

Then:

- **vxn-1** — re-export from core-utils; drop the local `raised_cosine_rise`,
  `BypassXfade`, and the duplicate `ms_to_samples` in `vxn-engine/src/smoothing.rs`
  (use `vxn_core_utils::{raised_cosine_rise, BypassXfade, ms_to_samples}`).
- **vxn-2** — replace the two inline `0.5 - 0.5*cos(π·t)` sites in `engine.rs`
  with `raised_cosine_rise`. Do **not** move vxn-2's `render_block_filter_xfade`
  structure itself — the dual-render-from-one-tick / OS-blend machinery is
  engine-specific and stays put; only the scalar weight is shared.

Keep it dependency-free (`math.rs`/`smoothing.rs` in core-utils use `std`, no
`libm`) — the weight is one `cos` call, no new deps.

## Acceptance criteria

- [ ] `raised_cosine_rise` + `BypassXfade` live in `vxn-core-utils`; vxn-1 uses
      the re-export and no longer defines its own (nor a duplicate `ms_to_samples`).
- [ ] Both vxn-2 inline cosine-weight sites call `raised_cosine_rise`; the
      engine-specific xfade structure is unchanged.
- [ ] `cargo test -p vxn-engine` (vxn-1) and `-p vxn2-engine` green; the
      declick tests (`tests/declick.rs`) still pass byte-for-byte (weight is
      numerically identical, so no baseline re-capture needed — verify).
- [ ] No behaviour change: `baseline_render_is_stable` and the vxn-2 render-hash
      baseline both unchanged.

## Notes

- Pure refactor / dedup — no audible change, no new DSP. Low priority; do it when
  next touching either engine's FX chain.
- Watch the vxn-2 render-hash baseline ([[vxn2-architecture]] test suite): a
  bit-different `cos` evaluation order could shift it. Expect identical since the
  expression is copied verbatim, but confirm rather than assume.
- If the two-place vxn-2 usage wants slightly different framing (gain-only OS
  fade vs dry/wet), keep `raised_cosine_rise` as the shared primitive and let
  each site build its own weighting on top — don't over-abstract the struct.
