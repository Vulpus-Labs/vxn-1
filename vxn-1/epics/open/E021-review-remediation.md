---
id: E021
title: Structural-review remediation (vxn-1) — host-boundary safety, smoothing, CI, hygiene
status: open
created: 2026-06-10
---

## Goal

Remediate the vxn-1 findings of the 2026-06-10 holistic
structural review. The synth's architecture came through
clean — layering matches ADR 0007, the audio thread is
allocation-free, the param model is single-source — so this
epic is not a redesign. It fixes the one UB-class defect
(a `wry` build panic that can unwind across the C ABI into
the host), closes the one audible gap (FX and mixer params
with no smoothing policy), puts the existing 380 Rust + 143
JS tests into CI, covers the untested JS orchestration
layer, resolves the `LocalParams` fork ambiguity against
`vxn-core-clap`, and then sweeps the accumulated hygiene
debt: stale docs, open epics whose tickets all closed, and
a batch of small code-quality items the review enumerated.

Companion to vxn-2's E006 (same review, same day). Items
that belong to the monorepo root or to vxn-2 are covered
there, not here.

## In scope

- Convert the `wry` WebView build panic in
  `vxn-ui-web::open_editor` into a `Result` surfaced through
  `set_parent`, and audit the remaining `expect`/panic paths
  reachable from CLAP entry points.
- Smoothing policy for ChorusRate/Depth/Mix, DelayTime/
  Feedback/Mix, LayerLevel and Spread — currently absent
  from the glide tables and unramped in the DSP, so
  automation zippers (DelayTime audibly clicks; Spread
  steps pan on all 8 voices).
- CI job running `cargo test --workspace` plus the gated
  vitest suite (`VXN_JS_TESTS=1`); coordinate with vxn-2
  ticket 0070 so one workflow covers the whole workspace.
- Direct JS tests for `rebindAllForLayer`, the sync/cutoff
  override factories, `locateSyncPartners`, and the
  `init()` → `applyViewEvents` flow.
- `LocalParams`: lift gesture-bracket emission into the
  generic `vxn-core-clap::LocalParams<N>` and consume it
  from `vxn-clap`, or document the fork as permanent.
- Docs/epic-state sweep: vxn-1 README self-contradiction
  (vizia vs wry), ADR 0001 §8 amendment, ADR 0006 §3/§4
  withdrawal notes, close epics E007/E009/E010/E013/E014/
  E017 whose tickets are all in `closed/`.
- Rust hygiene batch: `from_index` transmute → generated
  match, `factory()` parse caching, FTZ no-op fallback arm,
  broken rustdoc links, exp-ADSR test, smaller items.
- UI hygiene batch: remove `prototypes/`, dead CSS ruleset,
  palette hex literals, `onOpenChange` single-slot note,
  cosmetic `expect`s.

## Out of scope

- Root-level docs (root README workspace claims, root ADR
  0001 status) — covered by vxn-2 E006 ticket 0072.
- vxn-2 findings of the same review — E006.
- `EditorBackend::open` trait redesign in vxn-core-ui-web —
  shared-crate API change affecting both synths; ticket it
  at root level when the next backend is attempted.
- VST3 work — E020, unrelated.
- New features or perf work; this epic only closes review
  findings.

## Phasing

- **0115** Host-boundary panic safety (wry expect → Result).
- **0116** CI: workspace tests + JS suite.
- **0117** Smoothing policy for FX + mixer params.
- **0118** JS orchestration test coverage.
- **0119** LocalParams gesture unification with vxn-core-clap.
- **0120** Docs and epic-state sweep.
- **0121** Rust hygiene batch (engine/dsp/app/xtask).
- **0122** UI-web hygiene batch (JS/CSS/assets).

## Dependency order

```text
0115 (panic safety)  ── independent
0116 (CI)            ── land early; protects the rest
0117 (smoothing)     ── independent
0118 (JS tests)      ── before 0122 (tests pin behaviour
                        the cleanup then must not change)
0119 (LocalParams)   ── coordinate with vxn-2 0065 (gesture
                        port); whoever lands first shapes
                        the shared type
0120 (docs sweep)    ── independent
0121 (rust hygiene)  ── independent
0122 (ui hygiene)    ── after 0118
```

## Tickets

| # | Ticket | Priority |
|---|--------|----------|
| 1 | [0115 — Host-boundary panic safety](../../tickets/open/0115-host-boundary-panic-safety.md) | high |
| 2 | [0116 — CI workspace test job](../../tickets/open/0116-ci-workspace-tests.md) | high |
| 3 | [0117 — FX + mixer param smoothing](../../tickets/open/0117-fx-mixer-param-smoothing.md) | high |
| 4 | [0118 — JS orchestration tests](../../tickets/open/0118-js-orchestration-tests.md) | medium |
| 5 | [0119 — LocalParams gesture unification](../../tickets/open/0119-localparams-gesture-unification.md) | medium |
| 6 | [0120 — Docs and epic-state sweep](../../tickets/open/0120-docs-epic-state-sweep.md) | low |
| 7 | [0121 — Rust hygiene batch](../../tickets/open/0121-rust-hygiene-batch.md) | low |
| 8 | [0122 — UI-web hygiene batch](../../tickets/open/0122-ui-web-hygiene-batch.md) | low |

## Acceptance

- No panic path reachable from a CLAP host call in vxn-clap
  or vxn-ui-web; a forced WebView build failure surfaces as
  `PluginError`, the plugin stays alive, audio keeps
  rendering.
- Automating DelayTime, DelayMix, ChorusMix, LayerLevel and
  Spread produces no clicks or zipper artifacts (test +
  manual listen); the smoothing decision for each param is
  recorded in `smoothing.rs` comments either way.
- CI runs `cargo test --workspace` and the vitest suite on
  every push and fails the build on any test failure.
- `rebindAllForLayer` and each override factory have direct
  vitest coverage that fails if their observable behaviour
  changes.
- Exactly one production `LocalParams` implementation serves
  vxn-1 (shared generic or documented fork — no silent
  parallel copy).
- vxn-1 README and ADRs contain no live reference to the
  retired vizia editor as current; every epic in
  `epics/open/` has at least one ticket genuinely open.
- `cargo test --workspace` green at epic close; audio
  baseline (`tests/baseline.rs` hash) unchanged except where
  0117 deliberately alters smoothed-param renders, with the
  new hash justified in the ticket.

## Notes

Review reports (nine agent sweeps + hand verification,
2026-06-10) are the source for all file:line references in
the tickets. Where line numbers drift from HEAD, symbol
names are authoritative. One review claim was withdrawn
during verification: `CutoffTuned` is not a dead param — it
is a UI-only display-mode toggle the engine deliberately
never reads (0121 adds the comment that prevents the next
reviewer repeating the mistake).
