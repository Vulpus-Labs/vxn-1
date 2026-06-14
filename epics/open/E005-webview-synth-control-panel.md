---
id: E005
product: vxn-1
title: WebView synth control panel (Phase B of MVC migration)
status: open
created: 2026-05-30
---

## Goal

Build a `vxn-ui-web` editor backend — a `wry`-backed WKWebView (and on
Windows WebView2, Linux WebKitGTK) hosting an HTML/CSS/JS faceplate
that reaches **parity with the Vizia synth control panel** (rows of
panels, mod routes, voice, master, FX). It plugs into the controller
exactly like the Vizia editor (via `EditorBackend`); from the
controller's point of view, swapping is a single feature flag.

The preset browser and key-mode picker stay on Vizia for this epic —
they get a dedicated redesign in E011 since the Vizia versions never
reached a shippable state and a straight port would lock in
ergonomics we want to revisit.

Decisions recorded in [ADR 0007 §7](../../vxn-1/adrs/0007-vxn1-mvc-architecture.md).

## Background

The Vizia faceplate works, but has accumulated input-handling bugs the
codebase routes around rather than fixes (click-slop, automation
relayout stomp, absolute overlay click-eating). The WebView prototype
(deleted, captured in conversation history) confirmed a browser engine
makes those classes disappear at root. The remaining gap — host
keyboard capture for text fields — is host-policy, not toolkit, and is
solved with a floating popup in E011.

The other ROI here is dev velocity: HTML/CSS/JS surface is what LLMs
and developers can iterate on fluently; faceplate tweaks become
edit-the-file changes, not vizia-modifier archaeology.

E004 makes this epic structurally simple: the controller is the
contract, the new editor just implements `EditorBackend`.

## In scope

- New crate `vxn-ui-web` (wry + raw-window-handle + a minimal IPC
  bridge). macOS first, Windows/Linux gated behind cargo features but
  designed in.
- HTML/CSS/JS faceplate covering every Vizia synth-control panel:
  the 4-row layout, oscillators, LFOs, envelopes, filter + filter mod,
  pitch/PWM/cross-mod routes, mod wheel, bend, voice (assign / detune
  / glide / legato), master, chorus, delay.
- Controller-driven render: editor receives `ViewEvent`s from the
  controller and updates the DOM. Editor posts `UiEvent`s via wry's
  IPC.
- Host automation moves controls (Rust → JS push on view-event
  delivery).
- Param value displays + units sync (uses descriptor display
  strings).
- Cargo feature switch (`webview` / `vizia`) at vxn-clap; deploy.sh
  `--webview` flag re-introduced (this time as a non-prototype
  permanent flag).
- Rename `vxn-ui` → `vxn-ui-vizia` to make the split explicit.

## Out of scope

- Preset browser, save-as text field, preset-name display — stay on
  Vizia for this epic. (E011.)
- Key-mode picker, Upper/Lower edit toggle, split-point selector —
  stay on Vizia for this epic. (E011.)
- Floating popup for text input — needed only by the preset bar, so
  scoped to E011.
- Retiring the Vizia crate. As long as E011 hasn't shipped, both
  editor backends must compile.

This produces a slightly ugly intermediate state where the synth
panels are HTML and the top bar is Vizia. Both run in the same NSView
during transition: the Vizia editor remains the host's `EditorHandle`,
and inside it the synth-panel area is replaced by a child WKWebView.
Awkward but bounded — E011 closes the seam.

## Phasing

- **0039** `vxn-ui-web` crate scaffold + wry embed + IPC bridge wired
  to the controller. No real UI yet — a placeholder div.
- **0040** HTML shell: 4-row grid + panel containers + the
  faceplate-style header bar and gutters. Position-accurate against
  `target/vxn-layout.jsonl`.
- **0041** Oscillator panels (Osc 1, Osc 2, Mixer) — knob + slider +
  selector primitives in JS; each control posts `UiEvent`.
- **0042** LFO panels (LFO 1 per-voice, LFO 2 global) + host-sync
  rate display.
- **0043** Envelopes (Env 1, Env 2) + VCA + filter + filter mod.
- **0044** Mod routes (Pitch Mod, PWM Mod, Cross Mod, Mod Wheel,
  Bend).
- **0045** Voice (assign mode, unison detune, legato, glide) +
  master + chorus + delay panels.
- **0046** Host automation Rust → JS push (`ViewEvent::ParamChanged`
  → JS `setControl(id, norm, display)`).
- **0047** Rename `vxn-ui` → `vxn-ui-vizia`; cargo feature switch at
  `vxn-clap`; deploy.sh `--webview` flag; default stays Vizia until
  E011 retires it.

## Acceptance

- `./deploy.sh --webview` installs a CLAP whose editor opens, renders
  the synth panels in HTML, and accepts every UI gesture the Vizia
  version does. The preset bar / key-mode panel area still renders
  via the Vizia control (transition state).
- Faceplate looks recognisably the same as the Vizia version (same
  panel positions, same control affordances). Pixel parity not
  required.
- Host automation playback moves the right control on the right panel
  for every parameter.
- DAW automation recording works end-to-end (UI drag → host event in
  the DAW's automation lane).
- Build is green on macOS; Windows/Linux compile gates pass under
  `cargo check --target` per platform (CI shouldn't block on having
  the WebView2 SDK, but the code paths must compile).
- `cargo test --workspace` passes.
