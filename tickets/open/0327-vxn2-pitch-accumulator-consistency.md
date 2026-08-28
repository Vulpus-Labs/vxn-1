---
id: "0327"
product: vxn-2
title: "vxn-2: fold ratio and detune onto the semitone accumulator in compute_base_hz"
priority: low
created: 2026-08-29
epic: E048
depends: []
---

## Summary

Ticket of [E048](../../epics/open/E048-log-domain-level-pipeline.md).
Consistency, not correction — there is no known bug here.

`apply_pitch_mult` already does what [ADR 0010](../../vxn-2/adrs/0010-log-domain-level-pipeline.md)
asks of the amplitude chain: it sums bend, glide, pitch EG, global and per-op
pitch mod and stack detune **in semitones**, then applies a single
`2^(st/12)`. This is why the pitch chain has never had a calibration bug.

`compute_base_hz` is the one part off that rail — it applies the ratio as a
linear Hz multiply and detune as a separate `2^(cents/1200)`:

```rust
note_to_hz(key as f32) * (num_eff / denom) * 2_f32.powf(cents / 1200.0)
```

Mathematically equivalent. Worth folding onto the accumulator so the pattern is
uniform and the next pitch contributor has one obvious place to go.

## Acceptance criteria

- [ ] Ratio and detune enter as semitone offsets on the same accumulator as the
      rest of the pitch chain.
- [ ] Cooked `phase_inc` unchanged to within 1 ULP across the key range and the
      full ratio table — this is a refactor and must be provably inert.
- [ ] `Fixed` ratio mode still bypasses the key entirely.

## Notes

Low priority and independent of the 0323→0326 chain. Do it when the amplitude
work is landed, or not at all — the value is uniformity, and it is only worth
having if the amplitude side actually gets there.
