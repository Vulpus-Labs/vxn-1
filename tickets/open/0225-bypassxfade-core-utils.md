---
id: "0225"
product: monorepo
title: "BypassXfade + raised_cosine_rise → vxn-core-utils (absorbs ticket 0195)"
priority: medium
created: 2026-08-02
epic: E040
depends: ["0222"]
---

## Summary

Fourth ticket of [E040](../../epics/open/E040-vxn-core-dsp-foundations.md);
executes and absorbs open ticket
[0195](0195-shared-declick-core-utils.md) unchanged. Move
`raised_cosine_rise` + `BypassXfade` from
[vxn-engine/src/smoothing.rs:47-150](../../vxn-1/crates/vxn-engine/src/smoothing.rs#L47-L150)
to `vxn-core-utils::smoothing`; point vxn-2's two inline raised-cosine sites
in [engine.rs](../../vxn-2/crates/vxn2-engine/src/engine.rs) at the shared
helper; drop vxn-1's duplicate `ms_to_samples`.

After E041, `BypassXfade` is no longer used for per-FX enables (WetFade
replaces it) — it remains the primitive for **whole-span** switches: vxn-1's
oversample-change crossfade (`OutputStage`) and vxn-2's span fades.

## Acceptance criteria

- [ ] `BypassXfade` + `raised_cosine_rise` in `vxn-core-utils::smoothing`;
      vxn-engine imports them; single `ms_to_samples`.
- [ ] vxn-2's inline `0.5 - 0.5*cos(π·t)` sites call the shared fn — copy the
      expression verbatim; vxn-2 render hash must not drift (eval-order risk
      flagged in 0195).
- [ ] vxn-1 [tests/declick.rs](../../vxn-1/crates/vxn-engine/tests/declick.rs)
      byte-identical — verify, don't recapture.
- [ ] Ticket 0195 closed as absorbed (close-out points here).

## Notes

Pure move. E041 later repurposes this primitive; keep its API unchanged here.
