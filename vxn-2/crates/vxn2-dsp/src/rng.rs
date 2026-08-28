//! Re-export of `vxn_core_utils::math::xorshift64_star`.
//!
//! The stack's per-voice randomisation ([`crate::stack`]), the LFO
//! sample-and-hold / phase-scatter ([`crate::lfo`]) and the phaser's per-stage
//! break-frequency scatter all need a cheap, deterministic, audio-thread-safe
//! PRNG seeded from a `u64`. Each caller keeps its own `[0,1)` / `[-1,1)`
//! wrapper since the output mapping differs.
//!
//! Moved to `vxn-core-utils` by ticket 0228: the phaser moved to
//! `vxn-core-dsp`, and it takes this generator's exact stream with it.
pub(crate) use vxn_core_utils::math::xorshift64_star as xorshift_step;
