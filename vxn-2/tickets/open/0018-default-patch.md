---
id: "0018"
title: Default illustrative patch
priority: medium
created: 2026-06-05
epic: E002
---

## Summary

Bake an illustrative default patch into engine init so the plugin
sounds like an intentional sound on its first note, not like a single
sine carrier playing every key. The goal is "first note is musical and
exercises every block" — FM, stacking, both LFOs, both extra
envelopes, mod matrix, delay, reverb.

Lives in `vxn2-engine::default_patch` (a function that fills a
`ParamTable` snapshot + a default matrix table). Reachable from
`SharedParams::new` (overrides per-param defaults), from test
harnesses, and later from the preset loader as the on-disk default.

## Acceptance criteria

- [ ] `vxn2_engine::default_patch::default_param_values() ->
      [f32; TOTAL_PARAMS]` returns a hand-tuned set of plain-units
      values, one per CLAP id. Replaces `SharedParams::new()`'s
      per-descriptor default seeding.
- [ ] `vxn2_engine::default_patch::default_matrix() ->
      PatchMatrix` populates the per-layer matrix tables with the
      illustrative routings below.
- [ ] Patch character: DX-EP-flavoured electric piano with a slow
      vibrato breath and a wide, decorrelated stack.
      Specific values:
      - `voicing_mode = Whole`. Lower-layer params inert.
      - `algo = 5` (two parallel 2-op chains + carrier sum — classic
        DX-EP topology).
      - Per-op (Upper):
        - Op 1 (carrier): ratio 1.0, level 99, EG (R1=99, R2=50,
          R3=35, R4=60, L1=99, L2=80, L3=70, L4=0), pan −0.2,
          vel_sens 4.
        - Op 2 (modulator → 1): ratio 14.0, level 72, EG with fast
          decay (R1=99, R2=80, R3=20, R4=70, L1=99, L2=50, L3=0,
          L4=0), vel_sens 6 (bell-attack brightness).
        - Op 3 (carrier): ratio 1.0, level 88, EG slightly slower
          than op 1 (L3=78), pan +0.2.
        - Op 4 (modulator → 3): ratio 1.0, level 64, EG (R1=99,
          R2=60, R3=30, R4=60, L1=99, L2=70, L3=40, L4=0), vel_sens 5.
        - Op 5 (carrier): ratio 1.0, level 0 (off — algo 5 sums 3
          carriers; we leave one quiet to taste).
        - Op 6 (modulator → 5): ratio 1.0, level 0, off.
      - LFO 1 (global): Sine, 0.6 Hz, depth 0.40, sync off.
      - LFO 2 (per-voice): Sine, 5.1 Hz, delay 240 ms, fade 320 ms,
        trig Free.
      - Pitch EG: zero levels (no pitch movement by default — keep
        the envelope reachable but inert).
      - Mod Env: A=2 ms, D=480 ms, S=0.60, R=320 ms, Lin.
      - Assignment: Poly, glide 0 ms.
      - Stacking: density 4, detune 7 ct, spread 0.55, phase 0.50,
        distrib Linear.
      - Delay: on, time 3/8 sync, sync on, feedback 0.30, mix 0.18,
        ping-pong on.
      - Reverb: on, size 0.55, decay 2.4 s, damp 0.50, mix 0.18.
      - Master: tune 0 ct, volume −6 dB.
- [ ] Default matrix (Upper layer, slots 1–4 used; rest `None`):
      - Slot 1: `lfo2` → `global_pitch`, depth 0.03, lin curve
        (subtle vibrato).
      - Slot 2: `voice_rand` → `lfo2_phase`, depth 1.0, lin curve
        (decorrelate stack's LFO2 phases).
      - Slot 3: `velocity` → `op2_level`, depth 0.45, exp curve
        (brighter bell modulator on hard hits).
      - Slot 4: `mod_wheel` → `lfo1_rate`, depth 0.6, lin curve
        (mod wheel speeds the global vibrato).
- [ ] `SharedParams::new()` calls `default_param_values()` to seed
      the atomic table.
- [ ] Engine init seeds both layer matrix tables from
      `default_matrix()`. (Lower layer carries the same matrix — it's
      inert in Whole mode but ready for a switch to Layer / Split.)
- [ ] Listening test (manual, documented in commit message):
      held middle-C decays with the expected EP character, vibrato
      onset feels organic across a 4-note chord, stack is
      decorrelated (no comb-filter pulsing), delay + reverb feel
      present but unobtrusive.
- [ ] Automated test: render 4 seconds of audio with note-on at t=0
      and note-off at t=2; assert RMS in [−24, −9] dBFS during the
      attack/sustain (sound is present and not clipping), and ≤ −60
      dBFS at t=3.5 s (tail has decayed below audibility, accounting
      for reverb).

## Notes

This is a sound-design ticket, not a wiring one — values matter.
Start from the matrix above plus the per-op values and tweak in a
running plugin (after 0019 lands and the bundle is loadable). Commit
the final values once the sound is right; the listening test in the
ACs is the gate.

DX EP (DX7's `E.PIANO 1`) is a useful reference point both because
it's the canonical FM EP and because algo 5 specifically suits it.
The factory tuning isn't meant to replicate that patch; it's meant
to demonstrate that VXN2 *can* live in that idiom plus VXN2-specific
character (per-op FB, stack, LFO2).

Keep the function deterministic and side-effect-free. No
randomness, no time-of-day. Stack `voice_rand` is per-note-on
randomness from the engine — the patch values don't seed it.

A future preset epic will load this same patch from disk as
`Init.toml` in the factory bank. Implementing it as a function now
means both code paths can converge on a single source of truth then.
