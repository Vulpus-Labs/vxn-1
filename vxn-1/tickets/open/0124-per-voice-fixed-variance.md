---
id: "0124"
title: Fixed per-voice variance — env, sustain, base cutoff, resonance
priority: medium
created: 2026-06-13
epic: E022
---

## Summary

Every per-voice parameter that matters for analog character is
currently written identically to all N voices: `set_envelopes`
loops the same A/D/S/R + shape onto each `AdsrCore`
(`voice.rs:461-468`), and `set_coeffs` uses the shared
`ctx.cutoff` / `ctx.resonance` (`voice.rs:880`). The only
per-voice spread we model is the pitch drift *walk*.

Real hardware also has **fixed per-voice tolerance** — a
constant offset frozen at power-on from component spread (cap
tolerance on envelope RC, trim spread on VCF cutoff/Q). It is
not a walk; it is a per-lane constant that makes voice 3's
decay a touch longer and its filter a hair brighter than voice
5's, consistently. This ticket adds that layer.

Introduce a per-voice fixed-trim table, seeded deterministically
per lane (reusing the per-lane seeding style of `drift_walks`,
`poly.rs:258`), generated at construction/reset and **not**
reset on note-on (it is a property of the lane, like the drift
seed — `poly.rs:230`). Apply it to:

Targets and recommended spread:

- **Env A/D/R** — ±5–15% multiplicative — timing wobble
  across the keybed.
- **Env sustain** — ±1–5% — level wobble.
- **Resonance** — ±5–10% — self-osc onset varies voice to
  voice.
- **Base cutoff** — **≤ ±5 cents (aim ±3)** — gentle beating
  between voices only; see the in-tune constraint below.

## Acceptance criteria

- [ ] A per-voice fixed-trim table exists, seeded per lane,
      deterministic, generated at construction/reset, constant
      across note-ons (assert: same voice → same trim after
      multiple retriggers).
- [ ] Env A/D/R times carry a per-voice multiplicative trim;
      sustain a per-voice additive/multiplicative trim. Two
      voices given the same patch envelope render measurably
      different (but bounded) A/D/R timings.
- [ ] Resonance carries a per-voice trim; near self-osc
      threshold, voices cross into oscillation at slightly
      different resonance settings (one can whistle while a
      neighbour does not, by design).
- [ ] **Base-cutoff variance is bounded to keep self-reso
      whistles in tune.** With the filter self-oscillating and
      two voices sounding the same note, the cutoff trim
      produces *mild beating only* — assert the per-voice
      cutoff offset stays within ≤ ±5 cents (target ±3). A
      unit test pins the max trim in cents; a render test
      confirms two self-osc voices beat slowly rather than
      sounding detuned. (3 cents ≈ ~1.7 Hz beat at 1 kHz —
      pleasant; 10+ cents reads as out of tune — rejected.)
- [ ] **`drift_amount` is the master amount** (decided): the
      same knob scales both the pitch walk and this fixed
      variance — one "analog" control. At `drift_amount = 0`
      the variance must short-circuit to bit-exact
      identical-across-voices behaviour, so the layer-sum
      equivalence tests (already pinning `drift_amount = 0`)
      pass unchanged with no edit.
- [ ] `tests/baseline.rs` hash updated if the chosen default
      amount is non-zero for factory patches, with a commit
      note attributing the delta.

## Notes

**Master amount — decision needed.** Options:

1. **Reuse `drift_amount`** as a single "analog amount" knob
   driving both the pitch walk and the fixed variance. One
   knob, elegant, and the equivalence tests already zero it so
   the short-circuit is free. Downside: couples walk depth and
   tolerance depth — can't have one without the other.
2. **New dedicated param** (e.g. "Analog" / "Voice Variance").
   Independent control; must be explicitly zeroed in the
   equivalence tests. Param table is clean and id-stability is
   no longer a constraint (memory:
   vxn1-id-stability-dropped), so adding one is cheap, but it
   costs a UI cell.

Recommend option 1 unless independent control is wanted —
"drift" already reads as the analog-character knob.

**Seeding.** Derive trim seeds from the per-lane base like the
drift walks, with a distinct salt so trims don't correlate
with the pitch walk. Fixed trims need only one draw per lane
at init (no per-block tick), so this is near-zero hot-path
cost — unlike the walk, nothing runs in `render_block`.

**Cutoff trim vs internal ramp.** `PolyOtaLadder` ramps
coefficients internally (`smoothing.rs:17`). A fixed per-voice
cutoff offset is constant, so it just shifts each voice's
target — no extra ramp churn, no silent-skip interaction
(memory: silent-skip-filter-state). Apply it as a constant
semitone add at the `voice.rs:877` cutoff site, alongside the
0123 drift-keytrack term.

**Env application.** `set_envelopes` currently writes the same
tuple to every `AdsrCore`. Multiply each voice's A/D/R by its
trim inside that loop (the trim table is indexed by voice).
Confirm `AdsrCore::set_params` is cheap enough to call
per-voice — it already is (called per voice on every param
change today).

Depends on / pairs with 0123 (shared per-voice cutoff site).
Land 0123 first or together so the cutoff path is touched once.
