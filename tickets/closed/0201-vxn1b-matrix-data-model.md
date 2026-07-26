---
id: "0201"
product: vxn-1b
title: "Matrix data model + default patch (MatrixSlot, SourceId/DestId, seeded VCA + key-track)"
priority: high
created: 2026-07-25
epic: E036
---

## Summary

Define the mod-matrix data model and the seeded default patch
([ADR 0001](../../vxn-1b/adrs/0001-vxn1b-overall-design.md) §2–§3). No evaluation
yet (that is 0202) — this ticket is types + defaults + the source/dest rosters.

- `MatrixSlot { source: SourceId, dest: DestId, depth: f32, curve: Curve,
  scale_src: SourceId }` — 16 slots. `depth` mirrors the param (0200); the other
  four fields are patch topology.
- **`SourceId`** — all ten: `Env1, Env2, Lfo1, Lfo2, Velocity, Key, ModWheel,
  PitchWheel, Aftertouch, NoteRandom, None`. (`Aftertouch` value comes from
  0198's per-voice pressure; `NoteRandom` from 0199.)
- **`DestId`** — `Pitch, XModSweep, Pwm, Cutoff, Resonance, HpfCutoff, Amp,
  CrossModAmount, None`.
- **`Curve`** — `Lin, Exp, Log, Bipolar` (per VXN2 model).
- **`default_patch`** seeds the hardwired VXN1 terms as matrix routes:
  - **Env2 → Amp @ depth 1.0** (reproduces VXN1's hardwired VCA = Env2).
  - **Key → Cutoff** at the depth that yields **exactly 1 octave of cutoff per
    octave of key relative to C4** (reproduces VXN1's key-track).

## Acceptance criteria

- [ ] `MatrixSlot`, `SourceId` (10 sources + None), `DestId`, `Curve` defined in
      `vxn1b-engine`.
- [ ] `default_patch` seeds Env2→Amp @ 1.0 and Key→Cutoff at 1 oct/oct; remaining
      slots inactive (`None`).
- [ ] `SourceId`/`DestId`/`Curve` have exhaustive index<->enum conversions
      (a new source forces a compile-time decision — cf. VXN2 `is_bipolar`).
- [ ] Tests: enum round-trip; default-patch slot contents; `None` slot is inert.

## Notes

- Mirror VXN2's `SourceId` polarity classification for `scale_norm` (0202):
  unipolar = ModWheel, Aftertouch, Velocity, Key, NoteRandom; bipolar = Lfo1,
  Lfo2, Env1, Env2(?), PitchWheel — lock the exact table here and reuse in 0202.
  (`vxn-2/adrs/0009` has the reference table.)
- The seeded Key→Cutoff depth must be computed against VXN1's cutoff units so the
  parity test in 0202 matches; note the exact constant in the ticket close-out.
- Depends on 0198, 0199 (source values), 0200 (slot-depth params). Feeds 0202.
</content>

## Close-out (2026-07-26)

- `SourceId` (10 + None), `DestId` (8 + None), `Curve`, `MatrixSlot`, `MatrixTable`
  in [matrix.rs](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs); exhaustive
  `from_u8`/`idx` + `is_bipolar` (exhaustive match — new source forces a polarity
  decision). Polarity locked: bipolar = Lfo1/Lfo2/PitchWheel; unipolar = rest incl.
  envelopes. `default_patch` seeds Env2→Amp @1.0 + LFO1→Pitch vibrato (VXN1's 0.05 st
  default). **Deviation from ticket:** Key→Cutoff is *pre-wired but depth 0.0* (key-track
  off) per user direction — matches VXN1's `filter_key_track` default of 0.0;
  `KEY_CUTOFF_UNITY_DEPTH = 0.25` documents the 1-oct/oct calibration. Tests:
  `matrix::tests::*` (u8 round-trip/degrade, idx, polarity lock, inert None-slot,
  default-patch contents).
