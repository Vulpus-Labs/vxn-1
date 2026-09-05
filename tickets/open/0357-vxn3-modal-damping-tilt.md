---
id: "0357"
product: vxn-3
title: "vxn-3 frequency-dependent modal damping (Metal + Struck) — high partials must die first"
priority: high
created: 2026-09-04
epic: E034
---

## Summary

Both modal families ring every partial at **one shared decay coefficient**: Metal's eight
modes at [metal.rs:305-312](../../vxn-3/crates/vxn3-engine/src/engines/metal.rs#L305-L312)
all multiply by `d = self.cur_decay`, and Struck's four at
[struck.rs:294](../../vxn-3/crates/vxn3-engine/src/engines/struck.rs#L294) all multiply by
`dec`. The consequence: the spectrum at 10 ms and at 500 ms into a hit is *identical, only
quieter*. Real struck metal and real membranes damp their high partials fastest — that
frequency-dependent damping is the largest single reason a modal bank reads as "a chord of
detuned sines" rather than as a cymbal, a ride, or a tom.

This is the highest-leverage sound fix in the roster review: it changes the character of
every Metal and Struck flavour without touching their param values, and it is a cook-time
change only — the per-sample lane loop keeps its shape.

## Design

- **Per-mode decay coefficients.** Replace the scalar `cur_decay` / `decay_coef` in the
  lane loop with a cooked `[f32; METAL_MODES]` / `[f32; STRUCK_MODES]` array:

  ```
  decay[k] = base_decay.powf(1.0 + tilt * (ratio[k] - 1.0))
  ```

  `ratio[k] > 1` for every partial above the fundamental, so `tilt > 0` shortens the
  highs. `tilt = 0` reproduces today's output **bit-for-bit** (`powf(1.0)` is exact for
  the identity exponent — assert this).
- **One new family param each**: `Damp` (`MacroUnit::Percent`, 0..1, default **0** so
  every existing authored flavour is unchanged until re-authored). `METAL_P` 10 → 11,
  `STRUCK_P` 8 → 9.
- **Metal's choke keeps working.** `cur_decay` currently switches between open/closed and
  is also read by the XOR + noise envelopes ([metal.rs:320](../../vxn-3/crates/vxn3-engine/src/engines/metal.rs#L320),
  [metal.rs:332](../../vxn-3/crates/vxn3-engine/src/engines/metal.rs#L332)). Cook **two**
  tilted arrays (open + closed) and select between them on trig; the XOR/noise envelopes
  keep the *fundamental's* coefficient so their behaviour is unchanged. `choke()` must
  still collapse all modes to the ~5 ms release.
- **Cost.** `powf` per mode at cook time (per trig, already off the per-sample path).
  The lane loop gains one array read per mode — still SoA, still vectorises.
- **Re-author the flavours.** Ride, Crash, Cymbal, Tom want real tilt; Closed Hat wants
  little (it is noise-dominant already). Do this in the same ticket so the change is
  audible on the shipped kit, not just reachable.

## Acceptance criteria

- [ ] `Damp = 0` renders bit-for-bit identical to the pre-ticket engine for every authored
      Metal and Struck flavour (`assert_eq!` on the sample buffers, as
      `drive_and_click_inert_at_zero` does today).
- [ ] A test proves the tilt: at `Damp > 0`, the HF-energy fraction of a **late** window
      (say 300–500 ms) is materially lower than that of an early window, while at
      `Damp = 0` the two are comparable. Reuse the existing `hf_fraction` helper.
- [ ] Metal's note-split choke and cross-track `choke()` still damp an open ring — existing
      choke and re-hit tests kept green.
- [ ] Ride, Crash, Cymbal, Tom re-authored with non-zero `Damp`; `metal_flavours()` /
      `struck_flavours()` pairwise-distinct tests still pass.
- [ ] Flavour serialize/deserialize round-trips at the new `P` (cross-engine
      `flavour_engine_round_trips_through_rebuild` covers it); truncated-patch rejection
      updated for the new param counts.
- [ ] `cargo test -p vxn3-engine -p vxn3-clap` green; clippy clean; alloc-trap passes.

## Notes

- Deliberately **out of scope**, tracked separately if play shows they matter: Struck is
  monophonic (`active: bool`, [struck.rs:181](../../vxn-3/crates/vxn3-engine/src/engines/struck.rs#L181))
  and re-zeroes every mode's phase on trig
  ([struck.rs:322](../../vxn-3/crates/vxn3-engine/src/engines/struck.rs#L322)), so a roll
  restarts the body instead of adding energy to a ringing one — Metal already does the
  right thing by injecting into live state. Also out of scope: per-hit jitter on
  tune/decay/band, which would break the machine-gun sameness of repeated hits.
- Mind [[vxn3-lane-ui-concept]] — nothing here changes the lane/macro surface.
