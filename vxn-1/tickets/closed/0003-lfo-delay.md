---
id: "0003"
title: LFO delay / fade-in
priority: medium
created: 2026-05-24
epic: E001
---

## Summary

Fade LFO modulation in over a settable delay after each note-on (JP-8 LFO Delay
Time, 0–4 s). Delayed vibrato is the canonical use: the note speaks clean, then
the LFO swells in. The fade is **per-voice** (it tracks each note's age), even
though the LFO itself is global.

## Acceptance criteria

- [x] New param `LfoDelay` (s, 0–4, default 0 = no delay), **appended at the end
      of the `ParamId` table**; add `lfo_delay` to `BlockCtx`, populate in
      `build_ctx`.
- [x] `VoiceBank` gains a per-voice `lfo_delay_gain: [f32; N]` that starts at 0
      on `note_on` and ramps 0→1 over `lfo_delay` seconds (base rate), held at 1
      thereafter. Reset in `reset_all`.
- [x] In `render_block`, the LFO source seen by the matrix is scaled per voice:
      the `srcs[2]` / amp `ctx.lfo_val` term becomes `ctx.lfo_val *
      lfo_delay_gain[v]`, so every LFO-driven destination (pitch, cutoff, amp,
      PWM) fades in together.
- [x] `lfo_delay = 0` reproduces today's behaviour exactly (gain pinned to 1).
- [x] Tests: with a non-zero delay, an LFO-driven destination's modulation is
      ~0 immediately after note-on and reaches full depth after ~`lfo_delay`
      seconds; with delay 0 the value matches the pre-change path.

## Notes

- Per-voice ramp is the only added voice state; the global LFO phase is
  unchanged. This keeps the change contained.
- The ramp can be a simple linear increment per base frame
  (`1.0 / (lfo_delay * sample_rate)`), clamped to 1; guard `lfo_delay == 0`.
- Pitch/cutoff/PWM are resolved at block start and amp per base frame (see
  `voice.rs`); scale the LFO source in both spots consistently.
- When a second LFO lands (later epic) this becomes per-LFO; out of scope here.
- Validation: `cargo test -p vxn-engine`.
