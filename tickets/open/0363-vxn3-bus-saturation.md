---
id: "0363"
product: vxn-3
title: "vxn-3 bus saturation ahead of the master limiter — the glue both classic machines get from their output stage"
priority: medium
created: 2026-09-04
epic: E034
---

## Summary

There is no nonlinearity anywhere in vxn-3 except the Driven family's per-voice cubic
soft-clip ([kick_tone.rs:325-327](../../vxn-3/crates/vxn3-engine/src/engines/kick_tone.rs#L325-L327)),
which defaults to off. The master chain is: per-track gain/pan → delay send/return →
master volume → limiter
([engine.rs:318-335](../../vxn-3/crates/vxn3-engine/src/engine.rs#L318-L335)). Eight
independently clean voices summed with nothing to fuse them.

Both the 808 and the 909 get a great deal of their identity from their output stage — the
gentle saturation that makes a kick and a clap on the same bar sound like one machine
rather than two files played at once. A limiter is not that: it is a level protector, and it
only engages on peaks.

## Design

- **A `Saturator` in `vxn3-dsp`**, alongside `Limiter` and `Delay` — stereo, allocation-free,
  a drive-and-compensate soft-clip (the cubic `d * (1.5 - 0.5*d*d)` shape the Driven engine
  already uses is a reasonable starting point; a `tanh` approximation is the alternative if
  authoring wants a softer knee).
- **Placed between the master volume and the limiter** at
  [engine.rs:332-335](../../vxn-3/crates/vxn3-engine/src/engine.rs#L332-L335) — after the
  delay return, so throws saturate too, and before the limiter so the limiter still
  guarantees the ceiling.
- **Output gain compensation** so raising drive does not simply raise level — otherwise
  every A/B is confounded and the limiter does the work. `drive = 0` must be a true bypass,
  bit-for-bit.
- **Two host params**, following the ADR 0003 master-param pattern the delay and master
  volume already use: **`Drive`** (0..1, default **0**) and **`Bus Tone`** if a pre/post
  tilt proves necessary — start with `Drive` alone and add the tilt only if play asks.
- **Faceplate exposure** on the master strip next to the existing delay/limiter controls.

## Acceptance criteria

- [ ] `Drive = 0` is a bit-for-bit bypass — a full-kit render matches the pre-ticket output
      exactly (extend the `fx.rs` integration test).
- [ ] `Drive > 0` adds harmonics without raising RMS beyond a small tolerance (HF-fraction
      up, level flat) — proving the compensation works.
- [ ] The limiter still holds the ceiling with drive at maximum on a hot kit; no sample
      exceeds `LIMITER_CEILING`.
- [ ] Reported latency (PDC) is unchanged — the saturator adds no look-ahead
      ([engine.rs:502](../../vxn-3/crates/vxn3-engine/src/engine.rs#L502)).
- [ ] Host param round-trips through `clap.state`; `value_to_text` renders it.
- [ ] Allocation-free — alloc-trap extended to a driven bus.
- [ ] `cargo test -p vxn3-engine -p vxn3-clap` green; clippy clean; `clap-validator` 0
      failures.

## Notes

- **Per-track** drive is the obvious follow-up and deliberately not here: the track mix path
  is p-lockable (`LockParam`), so a per-track drive wants a lock lane, which is a bigger
  question about how many lanes the pattern engine should carry. Bus first — it is the
  cheaper half of the benefit.
- Aliasing: a static waveshaper on a drum bus at 48 kHz will fold some harmonics. Acceptable
  at moderate drive and arguably part of the character; if it proves objectionable, the
  oversampled-region work in [E042](../../epics/open/E042-oversampled-region.md) is the
  existing home for a fix rather than a bespoke one here.
