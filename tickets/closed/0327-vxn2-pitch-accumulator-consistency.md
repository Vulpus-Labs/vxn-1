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

## Outcome — done in part, and the other part deliberately not

**Detune folded onto the note.** It is a pitch offset, so it joins the note
argument rather than forming its own exponential: one `powf` instead of two, and
cents end up in the same domain as every other pitch contributor.

**Ratio left as a rational multiply.** Measured before writing it, and folding
the ratio onto the semitone accumulator costs something real — a `log2`/`exp2`
round trip returns 45:1 as `45.000003815` and 3:2 as `1.500000119`. The beating
is inaudible (~0.001 Hz at C4), but a modulator on the 14th harmonic must sit on
it *exactly*, and an FM ratio **is** a rational. That is the correct
representation, not an inconsistency to be tidied away. Uniformity was this
ticket's whole justification and it does not outweigh exactness here.

**The 1-ULP acceptance bar was unreachable and has been replaced.** Every
reordering perturbs rounding, because one `powf` does not round like two:

    f32 accumulator (as specced)   10 ULP    ratios lose exactness
    f64 accumulator                 2 ULP    ratios lose exactness
    detune-only (shipped)           7 ULP    ratios stay exact

Bounded instead by the musical quantity: worst case 2.03e-4 cents across the
full key range and the ratios in use, ~5000× below a 1-cent JND. Two tests pin
it — one on the divergence bound, one asserting `assert_eq!` exactness of the
rational ratios, which is the property that kept the ratio off the accumulator.

## Acceptance criteria

- [x] Detune enters as a semitone offset on the note, sharing the pitch domain.
- [~] Ratio does not — deliberately, see above. Superseded.
- [~] "Cooked `phase_inc` unchanged to within 1 ULP" — unreachable by any
      reordering; replaced by the 2.03e-4-cent bound above.
- [x] `Fixed` ratio mode still bypasses the key entirely.
- [x] Ratios remain exactly rational (`assert_eq!`, not a tolerance).

## Notes

Low priority and independent of the 0323→0326 chain. Do it when the amplitude
work is landed, or not at all — the value is uniformity, and it is only worth
having if the amplitude side actually gets there.

## Close-out (2026-08-29)

- Detune folded onto the note in `compute_base_hz`
  ([op.rs](../../vxn-2/crates/vxn2-dsp/src/op.rs)): one `powf` instead of two,
  cents sharing the domain `apply_pitch_mult` already sums in.
- Ratio deliberately **not** folded. Measured first: a `log2`/`exp2` round trip
  returns 45:1 as `45.000003815` and 3:2 as `1.500000119`. Inaudible
  (~0.001 Hz beat at C4) but an FM ratio *is* a rational and must be exact —
  that is the correct representation, not an inconsistency to tidy away.
  `ratio_is_exactly_rational` guards it with `assert_eq!`, not a tolerance.
- The "within 1 ULP" bar was unreachable by any reordering (10 ULP for the
  specced f32 accumulator, 2 with an f64 intermediate, 7 for what shipped —
  one `powf` does not round like two). Replaced by the musical bound: 2.03e-4
  cents worst case over the full key range and every ratio in the bank, ~5000×
  under a 1-cent JND, pinned by
  `detune_on_note_matches_the_separate_exponential`.
