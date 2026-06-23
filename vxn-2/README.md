# VXN2

6-operator FM synth, CLAP plugin. DSP kernel, CLAP shell, and HTML faceplate
all shipped; in mod-matrix / review-remediation hardening (epic E006). Seven
epics closed (E001–E005, E007, E008).

## Premises

- **Operator-based oscillator architecture** — DX-style ops, stack-detuned to
  enable hypersaw-type voicings without table-wavetable cost.
- **Fixed-point phase accumulation** — Q32 phase, free wraparound, zero drift.
  Float past the phase boundary (filters, envelopes, mixing stay f32).
- **Approximated sines, not table lookup** — Bhaskara+Moser polynomial: 5 mul
  + 2 abs + 2 add, branch-free, vectorises terrifically (NEON/AVX gather
  avoided). THD ~ -59 dB; masked under hypersaw detune. Two-tier option:
  `lookup_sine_q32` reserved for solo carriers.

## Design docs

- [ADR 0001 — overall design](adrs/0001-vxn2-overall-design.md)
- [ADR 0002 — drop dual layer](adrs/0002-drop-dual-layer.md)
- [ADR 0003 — dirty-bitset diff pump](adrs/0003-dirty-bitset-diff-pump.md)
- [ADR 0004 — optional per-voice oversampled filter](adrs/0004-optional-per-voice-oversampled-filter.md)
- [ADR 0005 — stack pitch modulation](adrs/0005-stack-pitch-mod.md)
- [ADR 0006 — voice lifecycle & click-free voice reuse](adrs/0006-voice-lifecycle-click-free-reuse.md)
- [PARAMETERS.md](PARAMETERS.md) — full param table + mod-matrix source/dest sets.

Tickets use a per-project counter (vxn-1 and vxn-2 each start at 0001 and
historically overlap — both have 0055–0060); a vxn-2 ticket number always
refers to `vxn-2/tickets/`.
