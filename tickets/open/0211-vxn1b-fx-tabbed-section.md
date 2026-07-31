---
id: "0211"
product: vxn-1b
title: "FX tabbed section UI — Chorus/Phaser/Delay/Reverb/Dynamics tabs, per-effect on/off"
priority: high
created: 2026-07-29
epic: E038
depends: ["0209"]
---

> **Scope change (2026-07-30):** **Dynamics is broken out of the tab strip into
> its own bottom-row panel** (landed early in 0209 during the layout pass — 7
> faders + a header on/off toggle, row 3: Voice · FX · **Dynamics** · Master).
> This diverges from ADR 0001 §8 (which specced Dynamics as the 5th FX tab); the
> ADR needs a §8 amendment. So this ticket now covers the **4-tab** FX section
> only (Chorus / Phaser / Delay / Reverb). The 4-tab shell already exists from
> the 0209 fork; remaining work here is wiring/polish + tests, not Dynamics.

## Summary

Build the **tabbed FX section**: one panel with a tab strip selecting which
effect's controls show (Chorus / Phaser / Delay / Reverb), plus a per-effect
header on/off toggle. Tabs are **pure UI** — no signal routing (the serial chain
is fixed in the engine, ticket 0207). Binds to the E037 FX params.
[ADR 0001](../../vxn-1b/adrs/0001-vxn1b-overall-design.md) §8 (amend for the
Dynamics split); [[E038]].

## Design

Single FX panel on faceplate row 3 ([[0209]]). Tab strip: 4 tabs (Dynamics is a
separate panel), one active at a time; selecting a tab swaps the visible control
set (pure view state, no param writes on tab switch).

**Params** ([params.rs](../../vxn-1b/crates/vxn1b-engine/src/params.rs), E037):

- **Chorus** — `chorus_on` + `chorus_rate` / `chorus_depth` / `chorus_mix`
- **Phaser** — `phaser_on` + `phaser_rate` / `phaser_depth` / `phaser_feedback`
  / `phaser_mix`
- **Delay** — `delay_on` + `delay_time` / `delay_feedback` / `delay_mix`
- **Reverb** — `reverb_on` + `reverb_size` / `reverb_decay` / `reverb_damp` /
  `reverb_mix`
- **Dynamics** — *moved to its own panel (see scope note); not a tab.*

**On/off.** Per-effect header toggle — orange title-bar toggle + body dim (VXN1
idiom). Toggling writes the effect's `*_on` bool param. Tab selection is
independent of on/off (you can view an off effect's controls).

**MVC.** Same dirty-bitset dispatch as [[0209]]; view never reads model.

## Acceptance

- One FX panel with a 4-tab strip (Chorus/Phaser/Delay/Reverb); each tab shows
  its effect's controls bound to the real E037 params. (Dynamics is its own
  panel, done in 0209.)
- Per-effect header on/off toggles the `*_on` param + dims the body; tab
  selection is pure UI (no param write).
- All five effects reachable.
- Token/contract tests pass (tab→control-set map, param bindings).
