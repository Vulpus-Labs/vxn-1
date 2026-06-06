---
id: "0099"
title: Factory presets — re-save bank for FDN reverb defaults
priority: low
created: 2026-06-06
epic: E018
---

## Summary

Re-save the factory preset bank so existing presets sound right
under the FDN reverb and the new param table. Closes E012
ticket 0059 (BBD factory tasting) as superseded.

Old keys (`reverb_type`, `reverb_depth`) are dropped from
preset TOMLs — they're ignored on load (per 0096) but the
saved files should be clean so a diff against a fresh save is
empty.

For each preset that had `reverb_on = 1` under the BBD reverb,
audition the new FDN voicing and pick `(size, decay, damp,
mix)` by ear. The macro mapping from old → new is not
mechanical (BBD Type was a structural mode, not a continuous
sweep), so do this by listening, not by table.

## Acceptance criteria

- [ ] All `crates/vxn-engine/presets/factory/**/*.toml` re-saved
      via the live plugin (load → save). No `reverb_type` /
      `reverb_depth` keys remain.
- [ ] `reverb_on = 1` presets gain explicit `reverb_size`,
      `reverb_decay`, `reverb_damp`, `reverb_mix` keys.
- [ ] `phaser_on = 0` defaults everywhere — no phaser on factory
      presets unless an audition picks one for it.
- [ ] `master_drift = 0` default everywhere (deterministic
      recall in the bank).
- [ ] Embedded factory bank rebuilt (`include_dir!()` re-snaps
      automatically on next build).
- [ ] Smoke-check each touched preset: load, hold a note,
      confirm the tail sits where the patch wants it.
- [ ] E012 moved to `epics/closed/`.
- [ ] 0059 moved to `tickets/closed/` with a one-line note
      that this ticket supersedes it.

## Notes

This ticket has no DSP / engine / UI changes — pure data audit.
Skip it on a release branch only if the BBD → FDN swap is
considered transparent (it isn't — reverb voicing differs
audibly), but the gating risk is "factory bank sounds wrong",
not "plugin crashes".

If a preset feels worse under any FDN voicing than under its
old BBD voicing, that's an honest signal that BBD had character
the FDN doesn't. Don't fight it — leave that preset with
`reverb_on = 0` and note it in the commit. A future ticket can
revisit whether the BBD-style reverb belongs back as an opt-in
flavour.
