---
id: "0281"
product: vxn-1b
title: "Sub-osc: own phase accumulator, zero-referenced at note-on"
priority: medium
created: 2026-08-23
epic: null
---

## Summary

The sub-oscillator is currently *derived* rather than generated: `poly_sub_square`
computes `sp = source_phase/2 + flip/2` fresh each sample from the source
oscillator's accumulator plus a per-voice flipflop the source kernel toggles on
each wrap ([oscillator.rs:717-733](../../vxn-1/crates/vxn-dsp/src/poly/oscillator.rs#L717-L733)).
That models the Juno's divide-down sub and gives an exact, drift-free lock.

It also inherits the source's start phase. `trigger_lane` stamps osc phase with
`lane_phase(v)` — a golden-ratio value in [0,1), not 0
([bank.rs:713-716](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L713-L716)) — so at
note-on `flip = 0` and `sp = ph0/2`, and the **first sub half-cycle is truncated
to `(1 - ph0)` source cycles**: 0.056 on lane 7, 0.910 on lane 4. The first sub
*period* runs `2 - ph0` source cycles instead of 2 — a lane-dependent onset chirp.
Under Poly's rotating allocation the same repeated bass note gets a different sub
attack each time, which is precisely the repeatability the Juno divider model was
chosen for.

Replace the derived sub with an independent phase accumulator: seeded to **0** at
note-on, increment fixed at `source_inc / 2`. Same lock, deterministic onset, and
the flipflop disappears.

## Design

- New `PolySub` in `vxn-dsp` holding `phase: [f32; N]`, with `reset(v)` and
  `process(&mut self, src_inc: &[f32; N], out: &mut [f32; N])`. Owning the state
  in its own type (rather than another array on `PolyOscillator`) keeps it clear
  that the sub is no longer tied to a particular oscillator's accumulator — the
  source alternates by cross-mod mode.
- `process` advances `phase[v] += src_inc[v] * 0.5`, wraps, and emits the same
  band-limited square as today: `naive + pblep(sp, sdt) - pblep(pf, sdt)`. The
  polyBLEP maths is unchanged — it keys off distance to the edge and does not
  care where the phase came from.
- `reset(v)` sets `phase[v] = 0.0`. Called from `trigger_lane` alongside the osc
  resets. New per-voice state, so it must also clear in `reset_all` (ADR 0002,
  Consequences).
- Delete `PolyOscillator::sub_flipflop` and its seven toggle sites across four
  kernels ([:359](../../vxn-1/crates/vxn-dsp/src/poly/oscillator.rs#L359), :369,
  :380, :401, :602, :681, :701) — including the dead `other.sub_flipflop` write
  in the PM kernel, which nothing has ever read.
- Call sites keep the existing source fork, but pass only the increment:
  `if sync { &osc2.inc } else { &osc1.inc }`
  ([bank.rs:1194-1201](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L1194-L1201),
  [voice.rs:1244-1250](../../vxn-1/crates/vxn-engine/src/voice.rs#L1244-L1250)).
  Sync still keys the sub to osc2 — the audible period is the master's.

### Behaviour deltas

1. **First sub half-cycle is a full source cycle, every note, every lane.** The
   point of the ticket.
2. **Sub edges no longer coincide exactly with source wraps.** Today that is true
   by construction; with an accumulator, f32 rounding lets absolute phase slip.
   Frequency stays *exactly* source/2 — `inc` is assigned, never accumulated —
   and both are set per `CONTROL_BLOCK`, so `Σ(inc_k/2) = (Σ inc_k)/2` and pitch
   mod, portamento, drift and vibrato all track perfectly. Slip is ~4e-5 cycles
   as a random walk over 10 s at 48 kHz, 0.03 cycles worst-case systematic.
   Deliberately accepted.
3. **Mode-switch parity hazard fixed.** `osc1.sub_flipflop` is clocked by osc2 in
   the sync kernel and osc1 elsewhere, and `cross_mod_type` is live-changeable
   with no retrigger — so switching Sync↔Off/Ring/PM mid-note re-points the clock
   while carrying arbitrary parity over, flipping sub polarity under a held note.
   An accumulator changes frequency (correctly) with phase continuous.
4. **The sub stops caring what the oscillators do at note-on.** Free-running
   osc1, free-running osc2, per-note random phase, sync resets on the slave —
   none of it reaches the sub.

## Acceptance criteria

- [ ] `sub_flipflop` is gone from `vxn-dsp`; no kernel writes it.
- [ ] First sub half-cycle is one full source cycle on every lane (test: trigger
      each lane, assert first zero-crossing at `source_period` within a sample).
- [ ] Sub frequency is exactly source/2 under static pitch **and** under a pitch
      LFO / portamento glide (peak-bin test at both, as `sub_pitch_off_is_source_half`
      does today).
- [ ] Sync mode keys the sub to osc2, pitch behaviour unchanged from today.
- [ ] Switching cross-mod Sync↔Ring under a held note produces no sub polarity
      discontinuity (regression test for delta 3 — currently fails).
- [ ] Default patch has `sub_level = 0.0` in both products, so `sub_on` is false:
      vxn-engine's `baseline.rs` `GOLDEN_HASH` and vxn1b's parity oracle
      ([parity.rs](../../vxn-1b/crates/vxn1b-engine/tests/parity.rs)) both stay
      green **with no rebaseline**. If either moves, something else changed.
- [ ] Factory presets that use the sub audited by ear; any render change is
      deliberate and noted at close-out.
- [ ] `cargo test -p vxn-dsp -p vxn-engine -p vxn1b-engine` green.

## Notes

- **Shared kernel.** `poly_sub_square` lives in vxn-1's `vxn-dsp` and has exactly
  two consumers: `vxn-engine/src/voice.rs:1250` and `vxn1b-engine/src/bank.rs:1201`.
  The signature change lands both call sites in the same commit — vxn-1 will not
  compile otherwise. vxn-2 and vxn-3 do not use it. Filed under `vxn-1b` because
  that is where the need surfaced; the vxn-1 edit is mechanical.
- **Why the flipflop existed.** It models the Juno's divide-down sub, and the
  phase lock was assumed to require the source phase reset. It does not — the
  lock came from deriving `sp` from `phase`, and survives any start phase,
  including a free-running source. Only the flipflop *reset* was pinning sub
  polarity, and an accumulator seeded to 0 pins it just as well.
- **Trade-off taken.** In sync mode you can have at most two of: free-running
  osc2, sub edges coincident with master wraps, deterministic sub onset. This
  gives up edge coincidence, which nothing has asked for, and keeps the onset
  determinism, which is audible on fast bass attacks.
- **Unblocks** giving osc2 an independent start phase instead of osc1's stamped
  value — the fix for ring-mod tonality (square×square currently collapses the
  Parker diode bridge to a scaled multiply, and both oscs starting at identical
  phase makes every note's product bit-identical). With this ticket landed that
  change is safe in all four cross-mod modes rather than only Ring. Follow-up
  ticket, not scoped here.
- Discovered while investigating why Ring on two squares a fifth apart sounds
  polite next to a miniKorg 700S.

## Close-out (2026-08-23)

- `PolySub` replaces the derived sub: `phase: [f32; N]`, `reset(v)` → 0,
  `process(&src_inc, &mut out)` advancing at `src_inc/2` and emitting the same
  BLEP'd square
  ([oscillator.rs:691](../../vxn-1/crates/vxn-dsp/src/poly/oscillator.rs#L691)).
  Advance-then-emit keeps the old sample alignment, so only the start phase moved.
- `sub_flipflop` gone — field, init, reset, and the seven toggle sites across the
  four kernels including the dead `other.sub_flipflop` write in the PM kernel.
  Repo-wide grep for `sub_flipflop|poly_sub_square` returns nothing.
- Engines hold a `sub: PolySub` beside the oscillators, reset per lane in
  `trigger`/`trigger_lane` and rebuilt in `reset_all`/`reset`; the call sites pass
  only the source increment
  ([voice.rs:1251](../../vxn-1/crates/vxn-engine/src/voice.rs#L1251),
  [bank.rs:1204](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L1204)). Sync still
  keys the sub to osc2.
- Tests: `sub_first_half_cycle_is_a_full_source_cycle_on_every_lane` (all 16 lanes,
  first edge at `source_period ±1` under golden-ratio osc phases),
  `sub_pitch_tracks_source_under_pitch_modulation` (block-quantised vibrato,
  zero-crossing counts stay 2:1), `sub_survives_cross_mod_switch_without_polarity_flip`
  (Sync→Ring mid-note, delta 3), plus the five ported sub tests.
- `baseline_render_is_stable` and `default_patch_render_matches_vxn1` green with
  **no rebaseline** — `sub_level` defaults to 0 in both param tables
  ([params.rs:536](../../vxn-1/crates/vxn-app/src/params.rs#L536),
  [params.rs:608](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L608)).
- Perf (16 voices, 512-frame blocks, minimum of two runs per tree): sub **off**
  gets cheaper — `dry_1x` 84.8 → 78.5 µs (−7.4%), the flipflop was a per-voice
  per-sample FMA in every kernel whether or not the sub was audible. Sub **on**
  is flat at 1x (96.9 → 97.6 µs) and costs ~5% at 4x sync (399.6 → 418.8 µs) for
  the accumulator's store. Worst case is still ~25× real-time.
- Factory presets using the sub (vxn-1: *Bass Pressure*, *Boofy Summers*, *One
  Ringmod To Rule Them All*; vxn-1b: *Split Bass and Lead*, *Ladder Bass*, *Wide
  Sub*) audited by ear and signed off — no unwanted render change.
- Shipped in `02dab34`; this close-out follows it.
