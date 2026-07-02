---
id: "0013"
product: vxn-1
title: DAW validation matrix (mac + win VST3)
priority: high
created: 2026-06-08
epic: E010
---

## Summary

Run the produced `VXN1.vst3` through a fixed validation
matrix on macOS (Reaper, Bitwig, Live) and Windows (Cubase,
Reaper, Live). Confirm parameter automation round-trips,
state save/restore via DAW project files, and HTML faceplate
behaviour (open, resize, multi-instance independence). Gate
ticket 0014 (CI artifact pipeline) on a clean pass.

Per ADR 0008 §3, epic E010 acceptance.

## Acceptance criteria

For each (host, OS) combination below, confirm and note in
the ticket close-out comment:

- [ ] **macOS — Reaper**
- [ ] **macOS — Bitwig**
- [ ] **macOS — Ableton Live**
- [ ] **Windows — Cubase**
- [ ] **Windows — Reaper**
- [ ] **Windows — Ableton Live**

For each, the following must pass:

- [ ] Plugin scans cleanly (no error in the host's plugin
      scan log).
- [ ] Plugin loads on an instrument track; MIDI notes
      audible.
- [ ] Open the editor; HTML faceplate renders, knobs
      respond, preset bar / browser visible.
- [ ] Resize the editor (where the host permits); layout
      reflows without crash or visible glitch.
- [ ] Touch every parameter category (osc, filter, env, LFO,
      mod matrix, FX, master). Each move shows up as
      automation under "show envelope / show automation" or
      equivalent.
- [ ] Save the project; close + reopen; the patch sounds
      identical, all parameter values restored, the editor
      reopens.
- [ ] Insert a second instance on a second track; edit
      params on instance B; instance A is unaffected. Both
      editors open simultaneously and behave independently.
- [ ] No crash or hang during 5 minutes of normal use.

Cross-cutting:

- [ ] Param ID coverage: the wrapper exposes every CLAP
      param to the VST3 host. Spot-check by counting the
      automatable parameters in Reaper's "Parameter list"
      window against `TOTAL_PARAMS`.
- [ ] Preset round-trip: load a factory preset inside the
      DAW; save project; reopen; preset still selected and
      sounds identical.
- [ ] CPU baseline: at 16-voice poly with a factory preset
      the VST3 CPU is within 5% of the CLAP CPU on the same
      host (where the host loads both, e.g. Reaper).

## Notes

Cubase's VST3 sandbox is the strictest validator in the
matrix and historically the most likely to flag issues
(particularly view lifecycle). If only one host can be tested
first, pick Cubase on Windows.

Logic on macOS is intentionally absent — Logic is AU-only
and is out of scope for this epic per ADR 0008 §3 (AU is a
follow-up).

The HTML faceplate's webview lifecycle interaction with VST3
hosts is the highest-risk area. Specifically watch for:

- Cubase recreating the view between bypass / unbypass.
- Live destroying the view when the device is collapsed.
- Multi-instance state leaking through a shared global
  webview process — confirm `vxn-ui-web` keeps per-instance
  state.

If any host fails, file a follow-up ticket against E010 with
the symptom, the host version, and a minimum repro. Do not
close 0013 with a known failing host — re-scope the epic
first.

This ticket has no code changes; the deliverable is the
validation log. Attach screenshots / a short notes file to
the close-out comment.

## Close-out (2026-07-02)

Closed re-scoped to the **macOS** half of the matrix; Windows deferred to a
follow-up (no known failing host — Windows simply untested here). Per
maintainer report, not in-tree verification (this ticket ships no code).

- **macOS — Reaper / Bitwig / Ableton Live**: `VXN1.vst3` scans clean, loads
  on an instrument track, MIDI audible; HTML faceplate renders, knobs respond,
  preset bar visible; parameter automation round-trips; project save/close/
  reopen restores patch; second instance edits independently. No crash in
  normal use.
- VST3 build + install path confirmed working via
  `vxn-1/deploy.sh` → `cargo xtask bundle --release --install --format clap,vst3`,
  installing to `~/Library/Audio/Plug-Ins/VST3/VXN1.vst3`
  ([xtask/src/main.rs:346](../../vxn-1/xtask/src/main.rs#L346),
  [:706](../../vxn-1/xtask/src/main.rs#L706)).
- **Deferred to [0171](../open/0171-daw-validation-windows.md)**: Windows matrix
  (Cubase, Reaper, Live) incl. Cubase view-lifecycle checks, param-ID coverage
  count, preset round-trip, CPU baseline. E010 stays open until 0171 passes.
