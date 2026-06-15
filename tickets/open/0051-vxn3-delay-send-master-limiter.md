---
id: "0051"
product: vxn-3
title: "vxn-3 delay send bus (p-lockable = dub throw) + master limiter"
priority: medium
created: 2026-06-15
epic: E021
depends: ["0050"]
---

## Summary

The MVP FX cut: one delay send bus and a terminal master limiter — enough for
the headline dub throw and safe output. A tiny subset of ADR 0002 (no roster,
no inserts, no external loops, no compressor/EQ).

## Design

- **Delay send bus.** One internal stereo delay (tape/BBD feel, feedback past
  unity for self-oscillation, tempo-syncable time). Lanes sum into it via a
  per-track send amount.
- **Dub throw.** The per-track send amount is a p-lockable param (0050):
  locking it high on a step throws that hit into the delay tail; rhythmically
  locking it gates the lane into the loop (dub gating) — no dedicated gate
  module needed.
- **Master limiter.** Terminal stage on the master bus, lookahead. It is the
  *only* limiter (master-only per ADR 0002). Report its lookahead latency to
  the host (CLAP `latency` / PDC).
- **Routing.** `tracks → mix ─(send)→ delay → return → mix → limiter → out`.
  Stereo throughout (uniform-stereo per ADR 0002 §4).

## Acceptance criteria

- [ ] A step p-lock on a track's send amount throws that hit into the delay
      while dry steps stay clean.
- [ ] Delay feedback past unity self-oscillates controllably; time can sync to
      host tempo.
- [ ] The master limiter catches peaks and prevents output clipping.
- [ ] Limiter lookahead latency is reported to the host and verified
      (round-trip null / PDC check).
- [ ] Process callback remains allocation-free.

## Notes

- Everything else in ADR 0002 (8 other modules, inserts, 4 buses, external
  send/return ports, bus compressor/EQ/gate) is deferred post-MVP.
- Design: `vxn-3/adrs/0002` (delay bus, master limiter, latency rationale);
  `vxn-3/adrs/0001` §3 (dub throw).
