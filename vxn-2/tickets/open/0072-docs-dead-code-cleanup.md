---
id: "0072"
title: "Docs + dead-code cleanup: README, ADR notes, dead CSS, orphaned event variant"
priority: low
created: 2026-06-10
epic: E006
depends: []
---

## Summary

Twelfth and final ticket of
[E006](../../epics/open/E006-review-remediation.md). Sweep of the
review's staleness and dead-code findings. Each item small; batch
them in one pass.

## Items

**Docs**

1. `README.md` — still says "design phase" / "See ADRs (forthcoming)".
   Rewrite to current state: kernel + CLAP shell + faceplate shipped,
   five epics closed, link the three ADRs and PARAMETERS.md.
2. `adrs/0001-vxn2-overall-design.md` §11 — claims vxn-1/vxn-2 are
   sibling workspaces; false since the flat-workspace migration
   (commit `46ddddf`). Add a forward-note (same style as §8's ADR 0002
   note), don't rewrite history.
3. `epics/closed/E002-clap-shell.md` lines 80-87 — eight ticket links
   point at `tickets/open/`; targets live in `tickets/closed/`. Fix
   paths.
4. PARAMETERS.md matrix-destination list (line ~174) — still
   enumerates v2 `op{N}_ratio`/`op{N}_detune` dests and omits
   `Feedback`. Update to the v3 dest set (18 op pitch/level/fb-able
   dests + `Feedback` + globals). (0061 touches the LFO1 section of
   the same file; coordinate.)
5. Document the ticket-numbering convention: vxn-1 and vxn-2 keep
   separate counters that historically overlap (both have 0055-0060).
   One sentence in each project's tickets/ README or the epic
   template — enough that the next ticket author doesn't guess.

**Dead code**

6. `vxn2-ui-web/assets/style.css` — delete dead blocks: `.mm-list` /
   `.mm-row` / `.mm-cell.*` / `.mm-add` (~lines 617-659, pre-0028
   matrix design), `.op-tab-level` / `.op-tab-level-fill` (~332-343),
   `.graph-curve-fill` (~559). Add the missing `.vxn-mm-badge-spacer`
   rule (or confirm grid-column sizing makes it unnecessary and
   delete the spacer span instead).
7. `Vxn2ViewCustom::MatrixRowChanged` — never emitted since E005.
   Remove the variant, its `serialise_custom_view` arm, and the
   `main.js` receive handler (whose stale-render hazard the review
   flagged: it updates shadow state without `renderAll()`). If kept
   for a future per-row diff, replace the JS handler body with a
   `console.warn` and comment the variant as reserved — don't leave a
   live divergent handler.
8. `vxn2-clap/src/lib.rs:1-12` module doc — "minus the Controller /
   ViewEvent / GUI / timer machinery" is false; the crate has all
   four. Rewrite.
9. Test fixture `vxn2-ui-web/src/lib.rs:336` — duplicate `"op"` key in
   the `json!` literal; rewrite so the fixture says what it tests.
10. Delete the orphaned pre-flat-workspace `vxn-2/target/` directory
    (stale artifacts from before `46ddddf`; the real target dir is the
    workspace root's). Confirm it's untracked/ignored first.
11. `op-row.js makeRatioButtonGroup` — renders Ratio/Fixed buttons
    with no listener and no `data-vxn-param`; dead UI. Remove, or file
    the wiring as its own ticket if the feature is wanted — don't ship
    inert buttons.

## Acceptance criteria

- [ ] Each item above done or explicitly deferred with a reason noted
  against the item in this ticket at close.
- [ ] `cargo test --workspace` green (items 7-9 touch tested code).
- [ ] Faceplate visually unchanged after CSS deletions (manual look +
  the HTML acceptance test).
- [ ] Greps clean: `MatrixRowChanged` gone (or stub-only), no
  `forthcoming` in README, no `mm-row` in style.css.

## Notes

The review's remaining JS findings — `vxn._opRow` side-channel,
capture-phase `stopImmediatePropagation` in the algo-picker overlay
wiring, KS-graph drag missing rAF coalescing, `var`→`const`
normalisation in mod-matrix.js/preset-bar.js — are real but
behaviour-touching; they belong in a UI-focused pass, not this doc
sweep. Listed here so they aren't lost: pull them into their own
ticket if/when the next UI epic opens.
