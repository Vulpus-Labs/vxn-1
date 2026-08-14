---
id: "0171"
product: vxn-1
title: DAW validation matrix — Windows VST3 (Cubase, Reaper, Live)
priority: high
created: 2026-07-02
epic: E010
depends: []
---

## Summary

Windows half of the DAW validation matrix, split out of
[0013](../closed/0013-daw-validation-matrix.md) when its macOS half passed
(Reaper / Bitwig / Live). E010 acceptance is not complete until Windows
passes — this ticket gates the epic. No code changes expected; the
deliverable is the validation log.

Per ADR 0008 §3, epic E010 acceptance.

## Acceptance criteria

For each (host, OS) below, confirm and record in the close-out:

- [ ] **Windows — Cubase**
- [ ] **Windows — Reaper**
- [ ] **Windows — Ableton Live**

For each host:

- [ ] Plugin scans cleanly (no error in the host's scan log).
- [ ] Loads on an instrument track; MIDI notes audible.
- [ ] Editor opens; HTML faceplate renders, knobs respond, preset bar visible.
- [ ] Resize (where the host permits) reflows without crash or glitch.
- [ ] Touch every param category (osc, filter, env, LFO, mod matrix, FX,
      master); each move shows as automation.
- [ ] Save project; close + reopen; patch identical, values restored, editor
      reopens.
- [ ] Second instance on a second track; edit B, A unaffected; both editors
      open simultaneously and independent.
- [ ] No crash or hang in 5 minutes of normal use.

Cross-cutting:

- [ ] Param ID coverage: wrapper exposes every CLAP param to the VST3 host —
      count Reaper's Parameter list against `TOTAL_PARAMS`.
- [ ] Preset round-trip: load a factory preset in the DAW; save; reopen;
      still selected and identical.
- [ ] CPU baseline: 16-voice poly VST3 CPU within 5% of CLAP on the same host
      (Reaper loads both).

## Notes

Cubase's VST3 sandbox is the strictest validator and historically most likely
to flag view-lifecycle issues — if only one host can be tested first, pick
Cubase. Watch: Cubase recreating the view on bypass/unbypass; Live destroying
the view on device collapse; multi-instance state leaking through a shared
webview process (confirm `vxn-ui-web` keeps per-instance state).

Do not close with a known failing host — re-scope E010 first and file a repro
(symptom, host version, minimum repro).

Logic (macOS, AU-only) remains out of scope per ADR 0008 §3.

## Close-out (2026-08-11)

**Verified on Windows by the maintainer; the matrix passed.** As the Summary
anticipated, no code changes were required — the deliverable is the validation
result itself.

- **Cubase, Reaper, Ableton Live** each: clean scan, loads on an instrument
  track with MIDI audible, editor opens with the HTML faceplate rendering and
  responding, resize reflows without glitch, every param category moves and
  registers as automation, project save/close/reopen restores the patch and
  reopens the editor, a second instance edits independently of the first, and no
  crash or hang in normal use.
- **Cross-cutting.** Param-ID coverage, factory-preset round-trip through a save
  and reopen, and the VST3-vs-CLAP CPU comparison all passed.

No host failed, so the ticket's "do not close with a known failing host" bar is
met and E010's Windows gate is satisfied. Logic (macOS, AU-only) remains out of
scope per ADR 0008 §3.

As with [0025](0025-windows-editor-verify.md), the per-host logs were produced
interactively and are not committed to the repo; this close-out records the
outcome, not the raw evidence.

