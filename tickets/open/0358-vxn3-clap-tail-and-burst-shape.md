---
id: "0358"
product: vxn-3
title: "vxn-3 clap tail + burst shaping — the 808/909 clap is 3 bursts plus a long tail, not 4 equal taps"
priority: high
created: 2026-09-04
epic: E034
---

## Summary

`Noise · Clap` fires its taps at identical level, at exactly even spacing, with the same
decay each time — the branchless gate at
[noise.rs:292-297](../../vxn-3/crates/vxn3-engine/src/engines/noise.rs#L292-L297) re-seeds
`noise_env[k]` to `1.0` and restarts the same countdown. Four equal bursts is a machine
gun, not a clap.

Both the 808 and the 909 clap are **three short bursts plus a fourth, much longer decaying
tail** (~100–200 ms) through the same band — the tail is the "room" and it is what makes
the sound read as hands rather than as a stutter gate. The bursts also ramp in level and
are not perfectly evenly spaced.

## Design

Three new Noise family params (`NOISE_P` 8 → 11), all defaulting to values that reproduce
today's behaviour so `Snare` and the neutral `Noise` flavour are untouched:

- **`Tail`** (`Seconds`, 0.0..0.4, default **0.0**) — after the last tap fires, the noise
  envelope switches to this longer decay instead of the burst decay. `0` = today. Cook a
  second coefficient; the per-lane state needs one flag (or reuse `tap_left` reaching 0)
  to select which coefficient that lane's envelope multiplies by. Keep it branchless with
  the existing mask idiom: `coef = burst + is_tail * (tail_coef - burst)`.
- **`Ramp`** (`Percent`, 0.0..1.0, default **0.0**) — per-tap level scaling, so the taps
  build (or fall) rather than all hitting at 1.0. Seed the re-fire to
  `1.0 - ramp * (tap_index / tap_count)` instead of a flat `1.0`.
- **`Spread`** (`Percent`, 0.0..1.0, default **0.0**) — pseudo-random jitter on the tap
  spacing, driven off the engine's existing xorshift so it costs nothing extra and stays
  deterministic per seed.

Then **re-author `flavour_clap()`** to use them: three bursts, a ~150 ms tail, a modest
ramp, a little spread. Its current base is `[0.03, 0.02, 0.0, 550.0, 2.0, 0.15, 4.0, 0.012]`
([noise.rs:82-84](../../vxn-3/crates/vxn3-engine/src/engines/noise.rs#L82-L84)).

## Acceptance criteria

- [ ] `Tail = 0, Ramp = 0, Spread = 0` renders bit-for-bit identical to the pre-ticket
      engine for every authored Noise flavour.
- [ ] A test proves the tail: with `Tail` set, the RMS of a late window (say 60–200 ms)
      is materially above the same window with `Tail = 0`, while the burst region is
      unchanged. Extends `multitap_refires_the_burst`.
- [ ] A test proves the ramp changes per-tap levels (compare per-tap peak envelope).
- [ ] `Spread` is deterministic for a fixed engine seed (two renders `assert_eq!`), and
      measurably shifts tap onsets versus `Spread = 0`.
- [ ] `flavour_clap()` re-authored; `noise_flavours_are_distinct` still passes.
- [ ] Round-trip + truncated-patch tolerance updated for `NOISE_P = 11`.
- [ ] `cargo test -p vxn3-engine -p vxn3-clap` green; clippy clean; alloc-trap passes.

## Notes

- The lane loop must stay branchless — the tap gate is the one place in the Noise engine
  where a per-lane `if` would defeat vectorisation. The existing compare-to-mask idiom
  extends cleanly to the tail-coefficient select.
- Related: 0359 gives the same family a second tuned body and independent layer levels;
  the two touch adjacent code in `render` but not the same lines. Land either order.
