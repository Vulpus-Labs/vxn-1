---
id: "0294"
product: vxn-1b
title: "Input adapters (Web MIDI incl. MPE + computer keyboard) and ship"
priority: medium
created: 2026-08-25
epic: E045
depends: ["0291", "0292"]
---

## Summary

Last ticket of [E045](../../epics/open/E045-vxn1b-web-wasm-browser-port.md). The
bundle builds, boots and paints, and there is **no way to play it**: VXN1b has no
on-screen keyboard, so until an input adapter is attached the browser build makes
no sound at all. That also blocks the manual browser passes
[0290](../closed/0290-vxn1b-web-controller-cdylib.md), [0291](0291-vxn1b-faceplate-rewire.md)
and [0292](0292-vxn1b-xtask-web-pipeline.md) are each waiting on, which is why
this is being taken ahead of [0293](0293-vxn1b-browser-persistence.md).

Two halves, and they are independent:

1. **Input** — Web MIDI + computer keyboard → the event ring, over 0284's shared
   adapters. Plus the BPM control (E045 delta 5), since the browser has no host
   transport and `sync.rs` resolves subdivisions against tempo.
2. **Ship** — a `deploy-web.sh` that does not clobber vxn-1's or vxn-2's
   `_headers` blocks ([[vxn-web-publish-flow]]), a hosting note, and the
   DAW-free browser smoke.

## Design

### The shared MIDI decoder needs three capability checks, not a fork

`crates/vxn-core-web/assets/midi-input.mjs` is shared with vxn-1 and vxn-2
(ticket 0284, whose whole point was not forking a third time). It does not fit
VXN1b as written, in three ways — all small, and all fixable by capability
detection rather than a per-synth copy:

- **It calls `host.sustain(...)` on CC 64.** VXN1b has no CC 64 path at all:
  `EV_SUSTAIN_RESERVED` decodes to `None` deliberately, so that the web build
  cannot behave differently from the plugin, and
  [`coordinator.mjs`](../../vxn-1b/crates/vxn1b-wasm/web/coordinator.mjs#L360)
  exposes no `sustain`. Today that is a `TypeError` the first time anyone
  touches a sustain pedal.
- **It folds every channel onto one engine.** VXN1b's dispatch is deliberately
  MPE-aware, and its producer takes a channel:
  `noteOn(note, velocity, offset, channel)`.
- **It ignores aftertouch.** VXN1b has `polyPressure` / `channelPressure` and the
  engine has the surface behind them (`EV_POLY_PRESSURE` / `EV_CHANNEL_PRESSURE`,
  added in 0286 for exactly this).

The fix keeps one module: pass the channel nibble as a trailing argument (JS
drops extra arguments, so vxn-1's and vxn-2's three-argument hosts are
unaffected), and gate sustain and the two pressure messages on
`typeof host.<method> === "function"`. No behaviour change for either shipped
port — which their suites must confirm, not just this one's.

**Scope limit worth stating:** MPE here is per-note **pressure**, not per-note
pitch. `EV_PITCH_BEND` carries no channel field, so bend stays global, matching
the plugin. Per-note bend would be a wire change and is not in this ticket.

### Keyboard

`attachKeyboard` calls `noteOn` / `noteOff` only, so it fits as-is. It is the
fallback when Web MIDI is denied or absent (Safari), which the shared adapter
already resolves gracefully rather than throwing.

### BPM

No host transport, so a UI control sends `EV_TEMPO`. `coordinator.setTempo(bpm)`
already exists; what is missing is somewhere to put the control and a default of
`DEFAULT_TEMPO_BPM`.

## Acceptance criteria

- [ ] A MIDI keyboard plays the browser build: notes, velocity, pitch bend, mod
      wheel.
- [ ] Channel rides note events; a second channel's notes are not folded onto
      the first. A test decodes a channel-3 note-on and asserts the channel
      reaches the producer.
- [ ] Poly and channel aftertouch reach `polyPressure` / `channelPressure`, and
      a test pins that a host without those methods (vxn-1's, vxn-2's) is not
      called.
- [ ] **A sustain-pedal message does not throw on VXN1b** and still works on
      vxn-1 / vxn-2 — the regression this ticket is most likely to cause.
- [ ] The computer keyboard plays, and is reachable when Web MIDI is denied.
- [ ] A BPM control sends `EV_TEMPO`; a synced LFO follows it audibly.
- [ ] `vxn-1` and `vxn-2` web suites stay green — the shared module is edited,
      so both shipped ports are re-checked, not just this one.
- [ ] `deploy-web.sh` publishes without disturbing the other two synths'
      `_headers` blocks ([[vxn-web-publish-flow]]).
- [ ] Browser smoke: boot, play, load a preset, copy a layer, watch meters and
      scope move — the pass 0290/0291/0292 are waiting on.

## Notes

- The input adapters are already in the bundle's dependency reach only once they
  are imported; [0292](0292-vxn1b-xtask-web-pipeline.md)'s copy list must gain
  `midi-input.mjs` and `keyboard-input.mjs` from `crates/vxn-core-web/assets`,
  and its closure test will fail loudly until it does.
- Editing a shared module touches two shipped products. Land it alone, ahead of
  the VXN1b-only wiring, so a bisect can separate them ([[E045]]'s 0284 risk
  note).
- No `cargo fmt` — [[vxn-no-cargo-fmt]]. One `cargo test` at a time —
  [[vxn-no-parallel-cargo-test]]. Stage explicit paths —
  [[vxn-concurrent-vxn2-work-no-git-add-all]].

## Close-out (2026-08-27)

- Channel rides note events: `midi-input.test.mjs` — *"the channel nibble rides
  note events for an MPE-aware host"*, *"a second channel is not folded onto the
  first"*, *"note-on with velocity 0 is a note-off, and keeps its channel"*.
- Aftertouch: *"poly and channel aftertouch reach an MPE-aware host"* and
  *"aftertouch is NOT sent to a host without those methods"* — vxn-1's and
  vxn-2's hosts have neither, so both messages stay off their wire.
- The regression this ticket was most likely to cause is pinned from both sides:
  *"a sustain pedal does not throw on a host with no pedal path"* and *"…and
  still reaches a host that has one"*. `CC_SUSTAIN` is gated on
  `typeof host.sustain === "function"`
  ([midi-input.mjs:204](../../crates/vxn-core-web/assets/midi-input.mjs#L204)).
- *"a single-timbral host still sees plain three-argument notes"* — the trailing
  channel argument is dropped by JS arity, so neither shipped port changes shape.
- Computer keyboard attaches before audio exists and is reachable when Web MIDI
  is denied: `faceplate-bridge.test.mjs` — *"boot attaches the computer keyboard
  before audio exists"*; the adapter resolves rather than throws on a denied
  permission ([midi-input.mjs:234](../../crates/vxn-core-web/assets/midi-input.mjs#L234)).
- BPM sends tempo on the ring: *"set_tempo is ring-only and refuses a nonsense
  BPM"* ([faceplate-bridge.mjs:241](../../vxn-1b/crates/vxn1b-wasm/web/faceplate-bridge.mjs#L241)).
- The shared module is edited, so both shipped ports were re-run, not just this
  one: vxn-1 29 passed, vxn-2 89 passed, VXN1b 151 passed — all 0 skipped.
- [deploy-web.sh](../../vxn-1b/crates/vxn1b-wasm/deploy-web.sh) exists alongside
  vxn-1's and vxn-2's; vxn-1's `_headers` clobber was fixed separately
  (`e5badae`) — [[vxn-web-publish-flow]].
- **Not verified here:** the hardware-MIDI pass (notes / velocity / bend / mod
  wheel from a real keyboard), the audible synced-LFO-follows-BPM check, and the
  browser smoke that 0290/0291/0292 were also waiting on.
