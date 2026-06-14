---
id: "0018"
product: vxn-1
title: Docs and epic-state sweep — vizia ghosts, stale opens
priority: low
created: 2026-06-10
epic: E011
---

## Summary

The docs layer has not kept pace with the work. The vxn-1
README contradicts itself in one file (line 19: "Vizia GUI
embedded via CLAP's gui extension"; line 30: "vxn-ui-web —
wry-WebView plugin GUI"). ADR 0001 §8 still specifies the
retired vizia editor with status `Accepted` and no
amendment. ADR 0006 §3/§4 spec preset tags that were
deliberately dropped mid-0002 (memory: vxn1-no-preset-tags)
with no withdrawal marker. And six epics sit in
`epics/open/` with every referenced ticket closed, eroding
the open/ folder's meaning as a work queue.

One sweep, no code changes.

## Acceptance criteria

- [ ] `vxn-1/README.md`: vizia claim removed; editor
      described once, correctly (wry WebView / HTML
      faceplate).
- [ ] ADR 0001: `## Amendment — 2026-06-10` section noting
      §8 (vizia editor) superseded by the ADR 0007 Phase B
      outcome (HTML faceplate via wry); crate list updated
      (`vxn-ui` → `vxn-ui-web`); vizia repo link removed or
      marked historical.
- [ ] ADR 0006: §3 (tag token filter) and §4 (tag editing)
      annotated withdrawn — tags dropped from `Meta`,
      category is the only discriminator; §8's vizia
      mouse-model caveats marked obsolete (WebView editor).
- [ ] Epics E002, E004, E005, E006, E007, E008 moved to
      `epics/closed/` (verify each has no genuinely open
      ticket first — E003 stays open while 0002, 0003, 0032 are
      open). Stale `tickets/open/...` cross-links inside
      moved epics repointed to `tickets/closed/`.
- [ ] `epics/open/` afterwards contains only epics with at
      least one open ticket (expected: E001, E003, E015,
      E016, E018, E019, E010, E011).
- [ ] `from_index`-style ticket/epic links inside remaining
      open epics spot-checked for `open/` → `closed/` drift
      and fixed where found.

## Notes

Root-level doc drift (root README's "each subdir is its own
workspace" / "vxn-2 in design", root ADR 0001 stuck at
`Proposed` with broken epic links) is already ticketed in
vxn-2 E012 0072 — do not duplicate it here; just verify it
landed when closing this ticket.

Keep amendments additive: ADRs are decision records, so
amend with dated sections rather than rewriting history.
