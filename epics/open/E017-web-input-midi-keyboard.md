---
id: E017
product: vxn-2
title: "vxn-1 web port — input (Web MIDI + computer keyboard)"
status: open
created: 2026-06-14
depends-on: E015
---

> **Depends on E015.** Input sources write note/CC/bend events *into* the
> E015 event ring; this epic is the plumbing from browser input APIs to
> that ring. It can run in parallel with E016/E018 once the ring and
> codec exist.

## Goal

Let the user play the web synth: Web MIDI input mapped to the E015 event
ring with correct timestamp→sample-offset conversion, plus a
computer-keyboard fallback for users without a MIDI device. Includes the
non-automatable key-mode/split control path.

When this epic closes:

- A connected MIDI keyboard plays the synth — note on/off, velocity,
  pitch bend, mod wheel, sustain (CC64) — with timing driven by MIDI
  event timestamps, not `postMessage` jitter.
- A QWERTY on-screen/computer-keyboard maps to notes for device-less play.
- MIDI device hotplug (connect/disconnect) is handled.
- Key mode (Whole/Dual/Split) and split point route correctly, via the
  E015 non-automatable-state path.

## Why separate from the core

The event ring (E015) is source-agnostic — it shouldn't know whether an
event came from MIDI, the UI, or automation. Keeping input adapters in
their own epic preserves that separation and lets MIDI and keyboard be
built and tested against a stable ring contract.

## Background

MIDI maps onto the existing dispatch semantics
([vxn-core-clap/src/events.rs:43-89](../../crates/vxn-core-clap/src/events.rs#L43-L89)):
note on/off → `Synth::note_on/off`, pitch-bend/mod-wheel/sustain →
global setters, key-mode/split → `UiEvent::Custom` shared state set once
per block. Web MIDI provides `DOMHighResTimeStamp`s that must be
converted to a sample offset within the upcoming render quantum to keep
sample-accuracy through the ring.

## Scope

**In:**

- Web MIDI API: request access, enumerate inputs, subscribe, decode MIDI
  bytes → E015 codec events. Note on/off + velocity, pitch bend, mod
  wheel (CC1), sustain (CC64).
- Timestamp→sample-offset mapping against the AudioContext clock, so
  notes land at the right sub-block position.
- MIDI device hotplug (statechange) handling.
- Computer-keyboard input map (QWERTY → note numbers, octave shift).
- Key-mode / split-point controls writing the E015 non-automatable-state
  path (not params).

**Out:**

- MIDI *output*, MIDI clock/transport sync (host tempo on the web is a
  later question).
- MPE / per-note expression.
- The UI widgets that *display* key mode (E018) — this epic provides the
  control path, the faceplate provides the buttons.

## Planned tickets

> Ids assigned at scaffold time. Provisional set:

- [ ] Web MIDI access + input enumeration + hotplug.
- [ ] MIDI decode → E015 codec events, with timestamp→sample-offset map.
- [ ] Computer-keyboard note input (QWERTY map, octave shift, held-note
      tracking).
- [ ] Key-mode / split-point control path over the E015 non-automatable
      state channel.

## Risks

- **Web MIDI support is uneven.** Not available in all browsers (Safari
  historically lacking / behind a flag); the keyboard fallback is the
  safety net, and the cross-browser matrix (E020) records the truth.
- **Timestamp accuracy.** MIDI timestamps and the AudioContext clock are
  different time bases; the offset conversion needs care or notes smear.
- **Permission UX.** MIDI access prompts; handle denial gracefully.

## Acceptance

- A connected MIDI keyboard plays the synth with velocity, bend, mod
  wheel, and sustain working.
- Note timing follows MIDI timestamps (audibly tighter than a
  `postMessage`-only path).
- Computer keyboard plays notes with no MIDI device present.
- MIDI hotplug works without reload.
- Key mode and split route notes correctly.
