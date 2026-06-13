//! Shared xorshift64* step. The stack's per-voice randomisation
//! ([`crate::stack`]) and the LFO sample-and-hold / phase-scatter
//! ([`crate::lfo`]) both need a cheap, deterministic, audio-thread-safe PRNG
//! seeded from a `u64`. They formerly carried byte-identical copies of the
//! step (ticket 0071 dedup); this is the single definition. Each caller keeps
//! its own `[0,1)` / `[-1,1)` wrapper since the output mapping differs.

/// xorshift64* — advances `state` in place and returns the scrambled word.
/// Constants are the canonical Vigna xorshift64* triple (13, 7, 17) and
/// multiplier; the top bits are the strong ones (callers take `>> 40`).
#[inline]
pub(crate) fn xorshift_step(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x.wrapping_mul(0x2545_F491_4F6C_DD1D)
}
