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

## Close-out (2026-06-15)

- **Delay send bus.**
  [delay.rs](../../vxn-3/crates/vxn3-dsp/src/delay.rs): stereo ping-pong delay,
  `tanh` saturator in the feedback path (feedback `>1` self-oscillates but stays
  bounded), one-pole damping, pre-allocated to 2 s. Test
  `delay::feedback_past_unity_self_oscillates_bounded`; engine-level
  `fx::delay_self_oscillates_past_unity_and_stays_finite`.
- **Dub throw.** Per-track send amount is `LockParam::Send` — p-lockable (0050).
  Routing: `tracks → dry mix + send → delay → return → limiter → out`
  ([engine.rs](../../vxn-3/crates/vxn3-engine/src/engine.rs),
  [track.rs](../../vxn-3/crates/vxn3-engine/src/track.rs) `mix_into`). Test
  `fx::send_plock_throws_a_hit_into_the_delay` — a revert send-lock throws that
  hit into the tail while the dry path (send base 0) stays clean (>8× tail rms).
- **Tempo sync.** Delay time = synced subdivision (`delay_sync_beats / bps`)
  recomputed each block. Test `fx::delay_time_tracks_host_tempo` — echo at ~18000
  samples @120 BPM, ~36000 @60 BPM.
- **Master limiter.**
  [limiter.rs](../../vxn-3/crates/vxn3-dsp/src/limiter.rs): stereo look-ahead
  brick-wall; the emerging sample's gain `≤ threshold/max(window)` and it's in
  the window → output provably `≤ threshold`. Tests
  `limiter::output_never_exceeds_threshold`, `quiet_signal_passes_through`, and
  engine-level `fx::master_limiter_prevents_clipping` (0.95 ceiling held under 8
  hot tracks).
- **Latency / PDC.** Constant 64-sample look-ahead reported via
  `Engine::latency_samples` → `PluginLatency` in
  [vxn3-clap](../../vxn-3/crates/vxn3-clap/src/lib.rs). Tests
  `fx::reports_limiter_latency`; `clap-validator` exercises the latency extension
  (0 failures). Always-on limiter → constant latency, no dynamic PDC churn.
- **Allocation-free.** Send/wet scratch + delay/limiter buffers pre-allocated.
  Test `fx::fx_path_is_allocation_free` — 0 allocs over ~300 blocks with send +
  delay + limiter active.
- **Faceplate.** Per-track Send knob + a master strip (delay time/feedback/
  return, limiter indicator) — re-enabling 0052's deferred send/master controls.
- 69 vxn3 tests green; vxn3 crates clippy-clean; `clap-validator` 0 failures;
  redeployed to `~/Library/Audio/Plug-Ins/CLAP/vxn3.clap`.
