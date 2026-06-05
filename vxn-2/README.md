# VXN2

Next-gen synth — design phase.

## Premises

- **Operator-based oscillator architecture** — DX-style ops, stack-detuned to
  enable hypersaw-type voicings without table-wavetable cost.
- **Fixed-point phase accumulation** — Q32 phase, free wraparound, zero drift.
  Float past the phase boundary (filters, envelopes, mixing stay f32).
- **Approximated sines, not table lookup** — Bhaskara+Moser polynomial: 5 mul
  + 2 abs + 2 add, branch-free, vectorises terrifically (NEON/AVX gather
  avoided). THD ~ -59 dB; masked under hypersaw detune. Two-tier option:
  `lookup_sine_q32` reserved for solo carriers.

See ADRs (forthcoming) for full design.
