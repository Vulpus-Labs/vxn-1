---
id: "0261"
product: vxn-1b
title: "Separate Osc 1 / Osc 2 PWM matrix destinations"
priority: medium
created: 2026-08-07
epic: E036
depends: []
---

## Summary

`DestId::Pwm` is a single destination applied to **both** oscillators — one
smoothed offset added to `osc1_pw` and `osc2_pw` alike
([bank.rs:551-552](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L551-L552)). That
makes the two pulse widths move in lockstep, which is the one thing you do not
want from two detuned pulse oscillators: PWM's thickness comes from the two
widths sweeping *independently*, so their beat is not locked to a single LFO
phase. Right now the only way to get that is Osc 2 Fine, which is a different
effect.

Adds `Osc1Pwm` and `Osc2Pwm` as separate destinations, keeping the existing
combined `Pwm` for the common case.

## Design

**Dests.** `DestId::Osc1Pwm` and `DestId::Osc2Pwm` appended after whatever ids
0249 takes (Pan = 9 → these are 10 and 11). `DEST_GAIN` = 0.5 for both, matching
the existing `Pwm` gain — ±0.5 of pulse-width fraction at full depth. No
`cook_depth` taper, as now.

**`Pwm` stays.** Renamed in the UI to `"PWM (Both)"`, machine id unchanged
(`"pwm"`). Removing it would cost two slots of sixteen for the single most common
PWM patch — one LFO into both widths — which is a bad trade for the matrix
budget. The three dests sum: osc 1's offset is `dests[Pwm] + dests[Osc1Pwm]`,
osc 2's is `dests[Pwm] + dests[Osc2Pwm]`.

**Smoothing.** [ModSmoothers](../../vxn-1b/crates/vxn1b-engine/src/mod_smoothing.rs#L78)
holds one `pwm: [f32; N]` one-pole per lane. It becomes two — `pwm1`/`pwm2` —
with `snap_slow`, `pwm_active`, `tick_pwm` and `pwm_current` taking (or
returning) both. Summing the dests *before* the one-pole keeps this to two
smoothers rather than three, and a patch using only the combined `Pwm` behaves
identically to today because both lanes then see the same target.

`pwm_active[v]` gates on either lane being live, so the per-quantum re-cook at
[bank.rs:686-689](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L686-L689) fires if
either oscillator's width is moving. A patch with no PWM route keeps the
block-constant widths and pays nothing, unchanged.

VXN1's `.clamp(0.05, 0.95)` applies per oscillator as now.

**UI.** Dropdowns come from the Rust vocab
([ui-web/src/lib.rs:244](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L244)), so the
new entries appear for free; only fixtures asserting vocab length need touching.

## Acceptance criteria

- [ ] `Osc1Pwm` / `Osc2Pwm` exist, round-trip through `from_u8`, and the
      exhaustive dest round-trip test covers the new `N_DESTS`.
- [ ] Engine test: LFO1 → `Osc1Pwm` moves osc 1's pulse width and leaves osc 2's
      at its patch value.
- [ ] Engine test: combined `Pwm` still moves both, identically to before this
      ticket (no behaviour change for existing patches).
- [ ] Engine test: `Pwm` and `Osc1Pwm` on the same patch **sum** on osc 1, while
      osc 2 sees `Pwm` alone.
- [ ] Both widths clamp to `[0.05, 0.95]` independently — a route driving osc 1
      to the rail must not clip osc 2's width.
- [ ] The `pwm_active` gate fires when *either* lane is live, and a patch with no
      PWM route still takes the block-constant path (assert the existing
      static-patch fast path is not perturbed).
- [ ] Two independent LFOs into `Osc1Pwm` / `Osc2Pwm` at different rates produce
      the intended non-locked beat — confirm by ear in Reaper
      ([[verify-audio-in-reaper]]).
- [ ] Matrix panel lists all three PWM dests with `"PWM (Both)"` relabelled.

## Notes

- Independent of 0248/0249 except for dest id allocation — whichever lands second
  takes the higher ids. No shared code beyond `DestId`.
- The combined `Pwm` dest's wire name stays `"pwm"`, so existing presets and state
  blobs decode unchanged; only its display label moves.
- Out of scope: per-oscillator PW *params* (`osc1_pw`/`osc2_pw` already exist and
  are already separate) and a sub-oscillator PWM dest.
