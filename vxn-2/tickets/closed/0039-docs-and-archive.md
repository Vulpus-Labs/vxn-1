---
id: "0039"
title: Docs — rewrite `PARAMETERS.md`, supersede ADR 0001 §8, archive 0009
priority: low
created: 2026-06-09
epic: E004
---

## Summary

Final ticket of [E004](../../epics/open/E004-single-layer-collapse.md).
Bring the documentation surface in line with the flat shape that
0033-0038 landed. Update `PARAMETERS.md`, add forward-notes to ADR
0001 §8, archive closed ticket 0009, prune README mentions.

Per [ADR 0002](../../adrs/0002-drop-dual-layer.md).

## Acceptance criteria

- [ ] `vxn-2/PARAMETERS.md`:
  - [ ] "Scope: per-layer vs patch-level" section (lines 16-30)
        deleted.
  - [ ] Every `(per-layer)` tag in section headers replaced with
        `(per-patch)` or simply removed.
  - [ ] "Voicing" section (lines 154-161) deleted in full — the
        `voicing_mode`, `split_point`, `edit_layer` rows disappear.
  - [ ] "Mod matrix" section (lines 180-200): "per-layer" qualifier
        removed; slot-count language unchanged ("16 slots per patch").
  - [ ] "Parameter count summary" (lines 252-302) rewritten:
    - per-patch subtotal = 162 (was per-layer subtotal 162 × 2)
    - patch-level subtotal = 17 (was 19 — drop voicing-mode +
      split-point)
    - CLAP total = **179** (was 343)
    - the "non-CLAP fields" tally (matrix topology selectors) drops
      its `× 2 layers` multiplier
  - [ ] Closing paragraph compares to DX7 (~155): VXN2 is now 179,
        increment justified by per-op feedback + extra envelopes +
        stacking + matrix automatable depths.
- [ ] `vxn-2/adrs/0001-vxn2-overall-design.md`:
  - [ ] Section 8 ("Voicing modes — Whole / Layer / Split") prefixed
        with a forward-note: *"Superseded by [ADR 0002](0002-drop-dual-layer.md).
        Voicing modes removed; a patch is a single parameter set."*
  - [ ] Section 11 ("Parameter model") count updated: ~155 → **179**
        (matches new PARAMETERS.md total). The "follows VXN1's
        ADR 0007 pattern" reference unchanged.
  - [ ] Consequences section: drop the bullet about "per-layer
        infrastructure shared with VXN1" — no longer applies.
  - [ ] Date / status of the ADR unchanged — original ADR stays
        Accepted as the original record; the forward-note is the only
        edit.
- [ ] `vxn-2/tickets/closed/0009-voicing-modes.md`:
  - [ ] Top of file: *"**SUPERSEDED** by [ADR 0002](../../adrs/0002-drop-dual-layer.md)
        and [E004](../../epics/open/E004-single-layer-collapse.md).
        Voicing-mode infrastructure has been removed. The acceptance
        criteria below describe a feature that no longer ships."*
  - [ ] Original body preserved (acceptance criteria, notes) — this
        is an archive marker, not a rewrite.
- [ ] `vxn-2/epics/closed/E001-audio-kernel.md`:
  - [ ] Ticket 0009 line (line ~61) gets a `(superseded)` suffix.
  - [ ] Dependency-order diagram (lines ~67-78) updates: the arrow
        through 0009 is annotated as superseded.
- [ ] `vxn-2/README.md`:
  - [ ] Any mention of "Whole / Layer / Split", "two parallel patches",
        "keyboard split", "Upper / Lower" — removed.
  - [ ] Synth-description paragraph foregrounds operator/algorithm
        flexibility as the timbral surface (per ADR 0002 rationale).
- [ ] Move `vxn-2/epics/open/E004-single-layer-collapse.md` →
      `vxn-2/epics/closed/` once all tickets 0033-0039 are checked off.
- [ ] Move `vxn-2/tickets/open/0033-*` … `0039-*` →
      `vxn-2/tickets/closed/` as each lands.

## Notes

This ticket is mostly mechanical text editing. Land it last in the
epic — `PARAMETERS.md` totals come from the actual flat table after
0035 ships, and the README description shouldn't go out ahead of the
code shape.

The forward-note in ADR 0001 §8 is the canonical pattern from VXN1
(see vxn-1 ADRs that have been superseded by later decisions). The
original ADR text stays as-is below the forward-note — ADRs are a
historical record.

`ui-mockup/index.html` was already updated in **0038** (it ships as part
of the UI rip, not the docs rip). This ticket doesn't re-touch it.

The `closed/E001-audio-kernel.md` edit is small but important — it's
the link a reader follows from the ticket index, and the superseded
state of 0009 needs to be visible at that level.

If any other ADR (currently only 0001 exists, but the count may grow
before this lands) references voicing modes, sweep those too.

Grep before merging: `rg -i 'voicing.mode|edit.layer|split.point|upper.{1,4}lower' vxn-2/{README.md,PARAMETERS.md,adrs/,epics/}`
should return only references in the superseded-marker notes.
