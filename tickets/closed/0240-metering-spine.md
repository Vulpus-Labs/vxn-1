---
id: "0240"
product: vxn-1b
title: "Metering spine — lock-free peak bus, audio→UI transport, JS meter widget"
priority: high
created: 2026-08-03
epic: E039
depends: ["0219"]
---

## Summary

Nothing in the monorepo meters today. Build the **shared metering spine** —
audio-thread capture, lock-free transport, view-event delivery, and a JS meter
widget with ballistics — and prove it end-to-end with one tap: **master out,
stereo**, on the FX/Global tab's Master panel.

This unblocks the per-layer mixer meters ([[0220]]) and the dynamics in / gain
reduction meters ([[0241]]). Designed as a shared spine so vxn-2 and vxn-3
inherit it rather than re-rolling.

## Design

### Capture — `MeterBus` (vxn-core-utils)

A fixed table of `AtomicU32` (f32 bits), one slot per tap **channel**. Not a
queue — metering is latest-value-wins, and a queue would need a size policy
nobody wants on the audio thread.

- **Audio thread**: atomic-max the block's peak into the slot (relaxed CAS loop
  over the bit pattern; f32 peaks are non-negative so the bit order is monotone).
- **Main thread**: `swap(0.0)` — read-and-clear.

The pair gives **peak-since-last-read**, which is the correct primitive here: it
is independent of block size vs. UI tick rate and cannot miss a transient
between ticks (a plain "last block's peak" store can, and a running average
smears the attack). No allocation, no lock, no contention — a meter tap costs
one atomic per channel per block.

Slots are addressed by a `MeterTap` index enum, sized with room for the taps
below. Reduction-style taps (gain reduction) publish atomic-**min** on a slot
initialised to `0.0 dB`, same read-and-clear discipline.

### Transport

Rides the existing 60 Hz `on_timer` → one `evaluate_script` per tick
([vxn1b-clap/src/lib.rs](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L280)).
The frame ships as `ViewEvent::Custom(MeterFrame)` through the
`serialise_custom_view` hook in
[vxn1b-ui-web](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L119), which is
currently `None`. No new bridge channel and no change to `vxn-core-app`'s
`ViewEvent` enum — `Custom` is exactly this escape hatch.

One `MeterFrame` per tick, so the batch dedupe (which keys on `ParamChanged` id)
is unaffected.

### Ballistics — JS side

The audio side publishes **raw peak only**. All ballistics render-side in a new
`panels/meter.js`:

- instant attack, decay ~20 dB/s
- 1 s peak-hold marker, then falls with the decay
- scale −60…0 dBFS, clipped-red above −0.1 dBFS

Keeping ballistics in JS holds the audio thread to one atomic per channel and
lets the curve be tuned without touching DSP.

### Tap table (this ticket wires the last row only)

| Tap | Channels | Ticket |
| --- | --- | --- |
| Layer 1 post-fader | L, R | [[0220]] |
| Layer 2 post-fader | L, R | [[0220]] |
| Dynamics in | L, R | [[0241]] |
| Dynamics out | L, R | [[0241]] |
| Dynamics gain reduction | 1 (stereo-linked detector) | [[0241]] |
| Master out | L, R | this ticket |

(`MeterTap::COUNT` is 11: the dynamics **out** pair was added during 0220's
layout pass — see [[0241]].)

## Acceptance criteria

- [x] `MeterBus` in `vxn-core-utils`: atomic-max publish, read-and-clear drain,
      `MeterTap` index enum. Unit tests cover peak-hold across a
      slower-than-block read, clear-on-read, and a 4-thread publish race.
- [x] Audio thread taps master out (post master volume, post finite-guard) in
      `Engine::process_block`. **Allocation-free** — `alloc_free.rs` extended,
      with a sanity assert so a zero alloc count cannot mean "never ran".
- [x] `MeterFrame` ships as `ViewEvent::Custom` via `serialise_custom_view`;
      one frame per controller tick, one `evaluate_script`. Idle-suppressed
      after the first all-zero frame.
- [x] `panels/meter.js` renders a stereo meter with the ballistics above;
      vitest covers decay, peak-hold and GR release as pure functions.
- [x] Master panel on the FX/Global tab shows a live stereo out meter.
- [x] Meter path is inert when the editor is closed (no GUI ⇒ no drain cost).
- [x] Contract/token tests pass; loads without JS errors.
- [x] Opens in a DAW — verified in Reaper 2026-08-26.

## Web-build compatibility

The page-side half is **already portable**; the Rust-side transport needs a
forwarding step at port time, but **no change to the design as built**.

**Portable as-is.** `panels/meter.js` is pure DOM + math and rides
`PANELS_FILES`, which the native and web faceplates both splice through
`assemble_faceplate` — so the web page gets the widget, the ballistics and the
`ev.kind === 'meters'` dispatch branch for free. The frame is plain JSON, which
is what the web bridge's `dispatch(batch)` already carries. Cadence is fine too:
`FaceplateBridge.start()` is a **free-running** rAF pump (it ticks every frame,
not only when dirty), so a meter frame per tick needs no new loop.

**The constraint.** Natively the bus is an `Arc<MeterBus>` shared over the Rust
heap. In the browser the controller (main thread) and the `Host` (AudioWorklet)
are separate wasm instances, and — checked, not assumed — **wasm linear memory
is not shared between them**: there is no shared-memory wasm build, and JS
shuttles bytes between dedicated `SharedArrayBuffer`s and linear memory (the
event ring copies in; audio output is read out). So an `Arc` cannot cross, and
neither can Rust simply write "into the SAB" — the SAB is not in the wasm
address space.

[`web/param-store.mjs`](../../vxn-1/crates/vxn-wasm/web/param-store.mjs) is the
exact precedent for what metering wants: *one SharedArrayBuffer, i32 atomics,
each word an f32 plain value bit-cast* — structurally identical to `MeterBus`.

### Options

**A — do nothing now.** Viable precisely because option C needs no change to the
design as built. Cost zero.

**B — shared wasm memory + borrowed-slice bus.** Build the worklet wasm with
shared memory/atomics, then make `MeterBus` borrow a slice into it rather than
own its array, so Rust writes where JS reads. One implementation, no copy — but
it is a toolchain change affecting the whole wasm build for one feature's
benefit, plus lifetime churn through `Engine` (which holds `Arc<MeterBus>`).
Hard to justify on metering alone; revisit only if something else independently
wants shared wasm memory, and let metering ride along rather than drive it.

**C — drain export, JS forwards (plan of record).** `MeterBus` stays exactly as
built, in linear memory. The wasm host exports a drain entry point; the
worklet's JS reads those few words after each render and folds them into a meter
SAB with `Atomics`; the main thread does `Atomics.exchange` and builds the same
`{kind:'meters'}` frame the native path sends. Mirrors how events and audio
already cross, needs no `unsafe` and no toolchain change — and
`MeterBus::drain_into(&mut [f32; COUNT])`, which already exists, *is* the
primitive it needs. The ~11 words per quantum of copying is irrelevant beside
the audio buffers already crossing the same way.

Two smaller consequences of C, both cheap:

- `viewEventToFaceplate` in `faceplate-bridge.mjs` is a fixed `switch` with
  `default: return null`, so a custom event would be dropped. But the meter
  frame does not come from `controller.tick()` anyway — as natively, it bypasses
  the controller. The bridge drains the meter SAB itself and appends the frame:
  ~10 lines of JS, **no** Rust or wasm involvement.
- Read-and-clear from JS is `Atomics.exchange`, the direct analogue of the Rust
  `swap(0)`.

**Decision: A, with C as the plan of record.** An earlier revision of this
section claimed the storage decision had to be made *before* a web port started.
That was wrong — it assumed the main thread would read the slots directly, which
the actual memory model rules out. C works with the bus untouched, so nothing is
being deferred at a cost.

**Nothing is broken today**: vxn-1b has no wasm crate (`vxn1b-wasm` does not
exist) and no xtask web command. `build_web_faceplate_html` / `gen-web-page.rs`
are inherited scaffolding pointing at a `faceplate-bridge.mjs` that vxn-1b does
not ship, and are exercised only by unit tests.

## Notes

- **Why not RMS as well.** A mixer strip reads peak; adding RMS doubles the slot
  count and the wire for a second number the player rarely wants. If it turns
  out to be wanted, the bus is an indexed table — adding slots is additive.
- **Denormals / silence.** Read-and-clear means a silent tick publishes exactly
  `0.0`, so the JS decay drives the bar down rather than a stale value sticking.
- **Shared-spine intent**: `MeterBus` lands in `vxn-core-utils` alongside the
  existing `sync` / `smoothing` helpers, not in `vxn1b-engine`, so vxn-2's FX bus
  and vxn-3's per-track meters can adopt it without a second implementation.
  Coordinate with the [[vxn-core-dsp-extraction]] plan (E040–E044) — this is
  additive to `vxn-core-utils` and does not touch the crates that epic moves.

## Close-out (2026-08-26)

- `MeterBus` + `MeterTap` (11 slots) landed in `vxn-core-utils`, not the vxn-1b
  engine, per the shared-spine intent
  ([meter.rs](../../crates/vxn-core-utils/src/meter.rs)): atomic-max
  `publish_peak`, atomic-min `publish_reduction`, read-and-clear
  `drain_into(&mut [f32; COUNT])`. 10 unit tests, including the multi-thread
  publish race and peak-hold across a slower-than-block read.
- Master tap published post master volume / post finite-guard in
  `Engine::process_block`; `MeterFrame` assembled in
  [vxn1b-engine/src/meters.rs](../../vxn-1b/crates/vxn1b-engine/src/meters.rs) and
  shipped as `ViewEvent::Custom` through `serialise_custom_view`, one frame per
  controller tick, idle-suppressed after the first all-zero frame.
- [panels/meter.js](../../vxn-1b/crates/vxn1b-ui-web/assets/panels/meter.js) holds
  all ballistics (instant attack, ~20 dB/s decay, 1 s peak hold, −60…0 dBFS,
  red above −0.1); vitest in
  [__tests__/meter.test.js](../../vxn-1b/crates/vxn1b-ui-web/assets/__tests__/meter.test.js).
  Audio thread stays at one atomic per channel per block.
- **Web transport: option C shipped**, not deferred. The "Web-build
  compatibility" section above is written against a tree where `vxn1b-wasm` did
  not exist; E045 built it, and with it the drain-export path this ticket named
  as the plan of record — `METER_LEN` / `vxn1b_host_drain_meters` /
  `vxn1b_host_meters_ptr` in
  [vxn1b-wasm/src/host.rs](../../vxn-1b/crates/vxn1b-wasm/src/host.rs#L51), read
  out of linear memory by the worklet's telemetry writer in
  [audio-host.mjs](../../vxn-1b/crates/vxn1b-wasm/web/audio-host.mjs#L58) at its own
  ~60 Hz division. `MeterBus` was not touched to make that work, which was the
  claim option C rested on. Treat that section as history.
- Verified in Reaper: master stereo meter reads live on the FX/Global tab.
