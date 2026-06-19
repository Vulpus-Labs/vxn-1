---
id: E023
product: vxn-2
title: "Analytic bandlimited shapes + supersaw feedstock"
status: open
created: 2026-06-19
---

> Use the all-carriers algorithm (32) as an additive engine: six carriers at
> harmonic ratios with controllable per-operator phase reconstruct
> bandlimited square- and saw-ish shapes by construction. Two cheap DSP
> additions make this clean and musical — a Nyquist-approach level fade so
> high partials don't alias, and a per-operator phase offset so the additive
> sum actually forms a saw (and square) rather than a same-spectrum
> phase-scramble. The payoff is feeding those shapes into the existing stack
> decorrelation for supersaw.

## Goal

Turn algorithm 32 into a usable analytic-shape source and ship presets that
stack and decorrelate it.

When this epic closes:

- Operator level fades toward zero as its running frequency (`ratio · f0`,
  plus bend/glide) approaches Nyquist, so a swept-up additive patch stays
  alias-clean instead of folding.
- A continuous per-operator phase-offset parameter exists, reset at note-on,
  composing with the existing per-lane stack decorrelation.
- Factory presets build stacked, decorrelated square and saw from algo 32
  and demonstrate the supersaw use case.

## Background

These came out of a design discussion (2026-06-18/19). Key conclusions that
shape the scope:

- **Algo 32 is not expensive.** Every algorithm evaluates six sine lookups
  regardless; algo 32 just drops the inter-op PM adds and pays a few more
  output-sum adds. Net ~flat. No special perf budget needed beyond the usual
  density × poly driver.
- **Nyquist fade catches carrier aliasing only.** FM sidebands sit at
  `carrier ± k·mod` (k unbounded) and are unaffected by per-op level fade —
  oversampling (E007 OTA path) is the lever there. For algo 32 (all carriers,
  no PM) the fade *is* a genuine bandlimit. Scope the fade to that honest
  claim; do not market it as global anti-alias.
- **Phase only matters in specific places.** A single steady voice is
  phase-deaf (Ohm's law) — square works at phase 0 either way. Per-op phase
  matters for (a) correct saw time-domain shape (even harmonics need a π
  flip), (b) the op acting as an FM modulator (downstream nonlinear), and
  (c) attack transient. The supersaw width itself already comes from the
  existing per-lane `StackParams.phase` decorrelation.
- **Phase param is continuous, stored as fraction [0,1), ×2^32 in kernel.**
  Quantizing to `2π/N` buys nothing and would collide lanes at low density.
  Shape ergonomics (snap to 0/¼/½/¾) belong in the UI as detents, not in the
  param domain.

## Existing code touchpoints

- Phase accumulator: `StackOp::phase[8]` (Q32) — stack.rs:183.
- Note-on phase reset: `Stack::note_on` → `apply_phase_offsets` —
  stack.rs:438, stack.rs:684. Currently writes the *same* per-lane offset to
  all six ops; per-op offset adds here.
- `OpParams` (no phase field today) — op.rs:36.
- PM hot loop (3-stage SoA) — stack.rs:758; PM scale `PM_SCALE_Q32 = 2^32`
  — op.rs:95.
- Per-op `phase_inc` / running frequency — drives the Nyquist fade input.

## Planned tickets

- [ ] 0073 — Nyquist-approach per-operator level fade (analytic anti-alias).
- [ ] 0074 — Per-operator phase-offset parameter.
- [ ] 0075 — Factory presets: stacked + decorrelated square and saw (algo 32).

## Dependencies

0075 depends on both 0073 and 0074. 0073 and 0074 are independent and can
land in either order.

## Risks

- **Six partials is thin.** Algo 32 gives at most six harmonics → mellow, no
  high bite. Spectral fill must come from stack density, not more partials;
  the presets need to lean on that. Verify the saw doesn't sound like a soft
  triangle solo.
- **Fade audibility.** A level fade that bites too early dulls legitimate
  bright patches; too late and it aliases. The fade window/curve needs a
  listening pass, not just a spectrogram.
- **Phase reset assumption.** Per-op offset relies on the stack path
  resetting phase at note-on (it does). The scalar reference path does not
  reset — keep this a stack-path feature, or document the divergence.

## Acceptance

- Sweeping an algo-32 additive patch up toward Nyquist shows partials fading
  out instead of folding back down (spectrogram + listen).
- Per-op phase offset is automatable, resets at note-on, and visibly changes
  the summed waveform on a scope; supersaw width still responds to the stack
  phase knob independently.
- Factory square and saw presets load, stack, and decorrelate; documented as
  the analytic-shape demonstration.
