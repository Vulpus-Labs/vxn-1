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

## Close-out (2026-06-22)

Docs-only sweep. No code changes.

- **`vxn-1/README.md`**: line 19 "Vizia GUI" → "HTML faceplate GUI (wry
  WebView)"; the crate table already described `vxn-ui-web` correctly, so
  the editor is now described once, consistently.
- **ADR 0001**: added `## Amendment — 2026-06-10` recording §8 (Vizia
  editor) superseded by the ADR 0007 Phase B outcome (HTML faceplate via
  wry); §8 carries an inline superseded banner; the §2 crate block now
  lists `vxn-ui-web` (+ `vxn-core-ui-web`) instead of `vxn-ui`; the §2
  body ref `vxn-ui`→`vxn-ui-web`; the Vizia References link marked
  historical. Also fixed two stray-character typos (`e#` title prefix, a
  dangling `</content>` tag).
- **ADR 0006**: added a dated Amendment withdrawing tags (§3 `#tag`
  filter, §4 tag editing, and the tag-carrying bits of §5–§7) — category
  is the only discriminator (memory `vxn1-no-preset-tags`); §3/§4 carry
  inline withdrawn banners; §8's vizia mouse-model caveats marked obsolete
  (WebView editor).
- **Epic state**: the bulk move the ticket described (E002, E004, E005,
  E006, E007 → `epics/closed/`) had **already landed** before this session
  — those, plus E001/E003/E015–E019/E021–E024, are in `epics/closed/`.
  **Deviation from the ticket:** E008 is **not** moved — its planned
  tickets (0086–0091 in the epic body) were never scaffolded (the numbers
  were taken by unrelated work), so E008 is genuine open future work, not
  a stale empty epic. Verified every current `epics/open/` entry has open
  or planned-but-unscaffolded work: E008 (js primitives), E010 (vst3,
  0008–0014 open), E011 (this epic, 0019/0020 open), E013 (0022–0026
  open), E014 (0027–0033 open), E020 (web-ship checklist, unscaffolded).
  None require moving.
- **Link drift**: repointed E011's own ticket table (0115/0116/0015/0016/
  0017/0018) from `tickets/open/` → `tickets/closed/` as those closed;
  0019/0020 stay `open/`. No other open epic had `open/`→`closed/` drift.
- **Root-level drift** (root README "vxn-2 in design" / "each subdir is
  its own Cargo workspace", root ADR 0001 status `Proposed`): per the
  ticket this belongs to **vxn-2 E012 0072** — verified it has **not**
  landed yet, left untouched here to avoid duplicating/colliding with
  that ticket's concurrent vxn-2 work.
