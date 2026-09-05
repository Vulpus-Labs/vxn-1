---
id: "0360"
product: vxn-3
title: "vxn-3 Driven click tone + decay (the 808 'Tone' knob) and a two-stage amp decay for punch"
priority: high
created: 2026-09-04
epic: E034
---

## Summary

Two gaps in the Driven family, both about the *front* of a hit.

**The click has no tone and no length.** It is raw white noise on a hardcoded 3 ms decay —
`CLICK_DECAY_S` at [kick_tone.rs:33](../../vxn-3/crates/vxn3-engine/src/engines/kick_tone.rs#L33),
mixed unfiltered at
[kick_tone.rs:330](../../vxn-3/crates/vxn3-engine/src/engines/kick_tone.rs#L330). On the
808 the kick's **Tone** control *is* the lowpass on its click layer; that filter is most of
the kick's front-end identity, and the same filtered-noise attack is the "skin" of a tom or
a conga. The Tom flavour's own doc comment concedes the gap:
*"the noise-attack layer has no sine-engine home"*
([kick_tone.rs:88](../../vxn-3/crates/vxn3-engine/src/engines/kick_tone.rs#L88)).

**The amp envelope has no punch/body split.** It is a one-pole attack times a *single*
exponential decay. An 808 kick is a fast transient over a long resonant tail; a 909 kick
sets its attack-click level against its body separately. One exponential is the reason a
synthesised kick reads as a flat beep rather than as a hit.

## Design

Three new params (`DRIVEN_P` 6 → 9), all defaulting to today's behaviour:

- **`Click Tone`** (`Hertz`, 200..12000, default **12000** ≈ open) — one-pole lowpass on
  the click's noise, engine-level (the click source is already shared across lanes at
  [kick_tone.rs:307](../../vxn-3/crates/vxn3-engine/src/engines/kick_tone.rs#L307), so one
  filter instance suffices and the lane loop is untouched). At the max cutoff the filter is
  effectively transparent — assert bit-for-bit, or set the cutoff param's max high enough
  that it is, and gate on an explicit `>=` bypass if float exactness needs it.
- **`Click Decay`** (`Seconds`, 0.0005..0.05, default **0.003**) — replaces the constant,
  same value, so defaults are unchanged.
- **`Punch`** (`Percent`, 0.0..1.0, default **0.0**) — a second, faster decay stage summed
  with the body: `amp = peak * atk * (dec + punch * dec_fast)`, where `dec_fast` is a
  per-lane state on a cooked short coefficient (a fixed fraction of `amp_decay_s`, or its
  own `Punch Time` param if authoring shows one value doesn't cover kick *and* tom). At
  `Punch = 0` the term vanishes exactly.

All three stay in the branchless SoA loop: `dec_fast` is one more `[f32; LANES]` array with
one multiply per sample.

**Re-author the flavours** so the shipped kit changes, not just the reachable space: `Kick`
(base `[0.001, 0.175, 41.0, 0.01, 0.0, 0.3]`,
[kick_tone.rs:55](../../vxn-3/crates/vxn3-engine/src/engines/kick_tone.rs#L55)) wants a
darker click and real punch; `Tom` and `Conga`
([kick_tone.rs:90](../../vxn-3/crates/vxn3-engine/src/engines/kick_tone.rs#L90),
[kick_tone.rs:97](../../vxn-3/crates/vxn3-engine/src/engines/kick_tone.rs#L97)) finally get
their noise skin via a mid-cutoff click.

## Acceptance criteria

- [ ] At the new params' defaults, every authored Driven flavour renders bit-for-bit
      identical to the pre-ticket engine (extends `drive_and_click_inert_at_zero`).
- [ ] A test proves `Click Tone` darkens the attack: at a low cutoff the HF-energy fraction
      of the first few ms is materially below the same window at the max cutoff. Reuse
      `hf_fraction` and the `b - a` isolation trick from `click_adds_onset_energy` (the
      shared `rng` advances identically either way).
- [ ] A test proves `Punch` adds early energy without lengthening the tail: the first
      ~20 ms RMS rises while a late window is unchanged.
- [ ] `Kick`, `Tom`, `Conga` re-authored; `authored_flavours_are_distinct` still passes.
- [ ] Round-trip + truncated-patch tolerance updated for `DRIVEN_P = 9`.
- [ ] `cargo test -p vxn3-engine -p vxn3-clap` green; clippy clean; alloc-trap passes.

## Notes

- The click filter is **engine-level, not per-lane** — deliberate, and consistent with how
  `Noise` puts its bandpass on the summed noise
  ([noise.rs:311](../../vxn-3/crates/vxn3-engine/src/engines/noise.rs#L311)). Per-lane
  click filtering would mean `LANES` filter states and a non-vectorising loop for no
  audible gain, since all lanes share one noise sample anyway.
- Out of scope: routing the click *through* the body resonator (the literal bridged-T
  topology) rather than alongside it. Struck is the resonator family; if the sine-engine
  kick still lacks weight after this, that is a Struck-flavour question, not a Driven one.
