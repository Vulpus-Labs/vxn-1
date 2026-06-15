---
id: "0054"
product: vxn-1
title: "MIDI decode → E015 ring events + timestamp→sample-offset map"
priority: high
created: 2026-06-15
epic: E017
depends: ["0053"]
---

## Summary

Decode raw MIDI bytes into `WebHost` producer calls (note on/off + velocity,
pitch bend, mod wheel CC1, sustain CC64) and convert each message's
`DOMHighResTimeStamp` to a sample offset 0..Q-1 within the upcoming render
quantum, so notes land at the right sub-block position rather than smearing to
block start like a postMessage-only path.

## Design

- **Decode** (`decodeMidiMessage(host, data, offset)` in `midi-input.mjs`):
  - `0x90` NoteOn → `host.noteOn(note, vel/127, offset)`; **velocity 0 == NoteOff**
    (running-status convention) → `host.noteOff`.
  - `0x80` NoteOff → `host.noteOff(note, offset)`.
  - `0xE0` PitchBend → 14-bit `(msb<<7)|lsb`, centre 8192, mapped to `[-1,1]`
    (asymmetric: 0→-1, 8192→0, 16383→+1) → `host.pitchBend`.
  - `0xB0` CC: CC1 → `host.modWheel(v/127)`; CC64 → `host.sustain(v>=64)`. Other
    CCs ignored.
  - Everything else (aftertouch, program change, SysEx, clock) ignored
    (forward-compatible).
- **Timestamp→offset** (`makeOffsetMapper(host, quantum)`): bridge the
  performance clock (`event.timeStamp`, ms) to the AudioContext clock via
  `ctx.getOutputTimestamp()` `{ contextTime, performanceTime }`:
  `contextTimeOfEvent = contextTime + (timeStamp - performanceTime)/1000`, then
  `offset = clamp(round((contextTimeOfEvent - ctx.currentTime) * sampleRate),
  0, Q-1)`.
- **Honest limits (documented in-module).** `ctx.currentTime` advances in
  quantum steps and lags the worklet's true next block by an unknown output
  latency, so absolute phase isn't guaranteed — we're accurate to the SPACING
  of events within a quantum, not their absolute frame. Past stamps clamp to 0
  ("ASAP", == the postMessage path, never a stale block); >1-quantum-future
  stamps clamp to Q-1 (no cross-quantum buffering on main); `timeStamp === 0` /
  no `getOutputTimestamp` → 0 (degrade, never throw).

## Acceptance criteria

- [ ] Note on/off + velocity, vel-0-as-off, pitch bend (−1/0/+1 endpoints),
      CC1 mod wheel, CC64 sustain all decode to the correct producer calls and
      land in the ring (asserted via `drainInto`).
- [ ] Unhandled messages (other CCs, program change) produce no ring record.
- [ ] An `event.timeStamp` in the near future maps to a positive offset
      (1 ms ≈ 48 frames @ 48 k); a past stamp clamps to 0; a far-future stamp
      clamps to Q-1; missing context / stamp → 0.

## Notes

- Main-thread offset is approximate BY DESIGN (epic risk §"Timestamp accuracy").
  The degenerate (offset 0) is the same correct behaviour the postMessage path
  would have; the win is sub-block spacing when the clocks cooperate.
- Channel nibble ignored (single-timbral).

## Close-out (2026-06-15)

- **Decode.** `decodeMidiMessage(host, data, offset)` in `midi-input.mjs`:
  NoteOn (vel/127; vel 0 → NoteOff), NoteOff, PitchBend (14-bit → asymmetric
  [-1,1]), CC1 ModWheel, CC64 Sustain (≥64). Other CCs / program change /
  aftertouch / system messages ignored. Running-status (no leading status byte)
  unsupported by design — every Web MIDI message carries its status byte.
- **Timestamp→offset.** `makeOffsetMapper(host, quantum)` bridges
  `event.timeStamp` (performance clock) to the context clock via
  `ctx.getOutputTimestamp()`, then `clamp(round(deltaSec * sampleRate), 0,
  Q-1)`. Past stamps → 0 (ASAP, == postMessage path, never a stale block);
  far-future → Q-1; `timeStamp === 0` / no `getOutputTimestamp` / no context →
  0 (degrade, never throw). The approximate-by-design limits are documented in
  a block comment at the top of the module per the epic's timestamp risk.
- **Tests.** `web/midi-input.test.mjs` §1 (note on/off+velocity, vel-0-as-off,
  bend −1/0/+1 endpoints, CC1 mod wheel, CC64 sustain on/off, unhandled
  messages produce no record → 9 records from 11 messages), §2
  (timestamp→offset: no-context→0, stamp-0→0, at-currentTime→0, +1ms→48 frames,
  past→0, +1s→127, no-getOutputTimestamp→0). Asserts via `EventRing.drainInto`.
- **Headless run note.** Same as 0053 — harness written + self-reviewed, manual
  `node` run pending (script execution blocked in this sandbox).
