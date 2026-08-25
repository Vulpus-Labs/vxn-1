---
id: "0288"
product: vxn-1b
title: "Telemetry return channel — meter + scope frames from the worklet to the page"
priority: medium
created: 2026-08-25
epic: E045
depends: ["0286", "0287"]
---

## Summary

Fourth ticket of [E045](../../epics/open/E045-vxn1b-web-wasm-browser-port.md),
and **the one with no prior art in either shipped browser port**. Everything so
far moves data main→worklet. This is the first thing that has to come back.

VXN1b's faceplate shows live audio: six meter bars (0240) and an oscilloscope
strip. Natively that is free — `MeterBus` and `ScopeBus` are `Arc`-shared between
the audio thread and the ~60 Hz `on_timer`, and the frames ride the existing
`ViewEvent` batch, costing no extra bridge call. On the web the engine is a
**separate wasm in a separate thread with separate linear memory**, so the buses'
read side is simply unreachable from the controller. Neither vxn-1 nor vxn-2 has
an audio→main data path at all; their only reverse traffic is a CPU-load number
on `port.postMessage`, which is the wrong shape for 60 Hz frames of audio data.

## Design

### A return SAB, not postMessage

`port.postMessage` allocates per message on the audio thread. At 60 Hz with a
384-sample scope window that is real GC churn in the render callback, and Safari's
JSC stalls the render thread on collection — the documented cause of VXN1's audio
blips ([[vxn1-web-safari-audioworklet]]). So: a second SharedArrayBuffer, written
by the worklet and read by the main thread on rAF.

Layout, one buffer:

```text
i32[0] meterSeq        seqlock counter, even = stable, odd = writing
i32[1] scopeSeq        ditto
i32[2] scopeLen        samples valid in the scope region (0 = no frame yet)
i32[3] reserved
f32[4 .. 4+11)         meter frame, MeterTap order
f32[.. +SCOPE_WINDOW)  scope window, oldest -> newest
```

### Why a seqlock

The param store gets away with plain per-slot atomics because each slot is
independently meaningful — a reader seeing a mix of old and new params is fine.
A **frame is not like that**: a scope window stitched from two different captures
shows a discontinuity, which reads as a glitch in the trace rather than as
slightly stale data. So each region gets a seqlock: the writer bumps the counter
to odd, writes, bumps to even; the reader takes the counter, reads, re-takes it,
and retries if it changed or was odd.

This is the standard SAB seqlock idiom, and it is the right trade here: the
writer never blocks (two atomic stores, no CAS, no waiting — mandatory on the
render thread), and the reader is the main thread, which is allowed to retry. The
reader retries a bounded number of times and then keeps the previous frame — a
dropped visual frame is not worth spinning rAF for.

### Rate division, and why it is not "every quantum"

`MeterFrame::drain` is **read-and-clear**: it reports the extreme since the
previous drain. Natively that drain happens on the ~60 Hz timer, so each frame
covers the whole interval the UI is about to display.

Draining every quantum would break that: at 48 kHz that is ~375 drains a second
against a 60 Hz reader, so the SAB would hold only the *last quantum's* peak and
the other ~5 would be discarded unseen. A transient landing in a discarded
quantum would simply never show on the meter.

So the worklet divides: drain and publish every `round(sampleRate / 128 / 60)`
quanta, and the scope every second publish (~30 Hz), matching the native
`SCOPE_TICK_DIVISOR`. Each published frame then covers the same span the native
tick would have.

`ScopeFrame` deliberately does *not* clear — the ring is a moving window and
overlapping reads are fine — so it needs no such care, only the rate division to
avoid pointless work.

### Silence suppression stays on the main thread

Native skips pushing a frame once a silent one has already been sent, so an idle
plugin doesn't stream 60 identical frames a second across the bridge. On the web
the SAB write is nearly free and the reader polls regardless, so the audio thread
stays dumb and unconditional; the *main* thread applies the
send-one-silent-frame-then-stop rule before dispatching into the page. Same
observable behaviour, none of the policy on the render thread.

### Allocation

`ScopeFrame::read` allocates a `Vec` per call, which is not RT-safe. The host
calls `ScopeBus::read_window` into a `Host`-owned buffer instead, so after the
first call there is no allocation in the render path.

## Acceptance criteria

- [ ] New C-ABI exports: drain meters into a host-owned buffer, read a scope
      window into a host-owned buffer, plus the pointers and the two length
      constants (`meter len`, `SCOPE_WINDOW`) so JS never hard-codes them.
- [ ] No allocation in the scope read path after the first call.
- [ ] `telemetry.mjs`: SAB alloc, a worklet-side writer, a main-side reader, and
      the seqlock, with the region layout declared once.
- [ ] A test proves a reader retries and rejects a torn frame: with the writer
      mid-update (odd counter), the reader must not return partial data.
- [ ] A test proves rate division — the meter frame published covers every
      quantum since the last publish, not just the most recent one.
- [ ] A test proves the silence rule: one silent frame is delivered, subsequent
      identical silent frames are suppressed, and the next non-silent frame
      resumes delivery.
- [ ] End-to-end through the real wasm: a sounding note produces a non-zero
      master meter reading and a non-flat scope window on the main side.
- [ ] Scope respects the tap — nothing is captured while the tap is Off.
- [ ] `cargo test -p vxn1b-wasm` and the web suite green, 0 skipped.

## Notes

- Meter values are **linear peak magnitudes**; dB mapping and ballistics belong
  to the view ([[vxn-metering-spine]]), so nothing here converts.
- The scope tap is pointed from the page over the 0287 ring (`EV_SCOPE_TAP`);
  this ticket only reads what the tap captures.
- Out of scope: wiring the frames into `panels/meter.js` / `panels/scope.js` —
  that is 0290's faceplate rewire, which keeps the page's existing
  `ev.kind === 'meters' | 'scope'` contract.
- No `cargo fmt` — [[vxn-no-cargo-fmt]]. One `cargo test` at a time —
  [[vxn-no-parallel-cargo-test]].

## Close-out (2026-08-25)

- Six new C-ABI exports on [host.rs](../../vxn-1b/crates/vxn1b-wasm/src/host.rs):
  `vxn1b_host_drain_meters` / `_meters_ptr`, `vxn1b_host_read_scope` /
  `_scope_ptr`, `vxn1b_meter_len`, `vxn1b_scope_window` — the last two so JS
  sizes its SAB regions from the engine, not a literal.
- Scope reads go through `ScopeBus::read_window` into a host-owned buffer rather
  than `ScopeFrame::read`, which allocates a `Vec` per call;
  `tests::repeated_scope_reads_do_not_reallocate` holds capacity constant across
  16 reads.
- [telemetry.mjs](../../vxn-1b/crates/vxn1b-wasm/web/telemetry.mjs): SAB layout,
  `TelemetryWriter` (worklet) and `TelemetryReader` (main) with a per-region
  seqlock. Writer never blocks — two atomic stores, no CAS; reader retries a
  bounded number of times and otherwise keeps its previous frame.
- Torn-read rejection proven:
  `a reader will not return a frame while the writer is mid-update` (odd counter
  plus a planted value the reader must never surface).
- Rate division proven by consequence, not by counting:
  `a published frame covers every quantum since the last publish` plants a
  transient in an early quantum and asserts it survives the read-and-clear drain
  to the UI. `tick()` publishes every 6 quanta at 48 kHz, scope every second
  publish.
- Silence rule: one silent frame delivered (the view needs the zero that starts
  its decay), subsequent ones suppressed, audio resumes delivery — all in
  `one silent frame is delivered, then silence is suppressed`.
- Scope respects the tap: `the_scope_captures_the_selected_tap_and_nothing_while_off`
  renders a full window's worth with the tap Off and gets 0 samples.
- End-to-end through the real wasm: a sounding note yields a non-zero master
  meter and a non-flat window on the main side.
- **Bug found by that e2e test**: the reader's `_seen` counters were seeded to
  `-1`, so the first read saw `0 !== -1`, concluded something was new, and handed
  back its own zeroed region as though the engine had published silence — burning
  the single silent frame the suppression rule allows, so the engine's real first
  frame was the one dropped. Seeded to `0` now, with a regression test.
- `cargo test -p vxn1b-wasm` 28 pass; web suite 55/55, 0 skipped;
  `cargo test --workspace` 1571 pass, 0 fail.
