//! Branchless drum-envelope coefficients.
//!
//! Drum voices use a one-pole attack rise × exponential decay, both updated
//! per-sample with a single multiply/MAC and **no stage branch** — so the
//! 4-lane voice loop stays vectorisable (the vxn-1/vxn-2 "no match in the lane
//! loop" lesson, applied to envelopes since a poly engine's lanes carry
//! independent per-voice envelope state). The engine stores per-lane state
//! (`atk`, `dec`); these helpers cook the shared per-sample coefficients.

const LN_0_001: f32 = -6.907_755_4; // ln(10^-3) — -60 dB

/// Per-sample multiplier for an exponential decay reaching -60 dB over
/// `time_s`. `time_s <= 0` collapses to an instant decay (returns 0).
#[inline]
pub fn decay_coef(time_s: f32, sample_rate: f32) -> f32 {
    if time_s <= 0.0 {
        return 0.0;
    }
    (LN_0_001 / (time_s * sample_rate)).exp()
}

/// One-pole rise coefficient `a` for `state += (1 - state) * a`, reaching ~63%
/// of target in `time_s`. `time_s <= 0` snaps instantly (returns 1).
#[inline]
pub fn attack_coef(time_s: f32, sample_rate: f32) -> f32 {
    if time_s <= 0.0 {
        return 1.0;
    }
    1.0 - (-1.0 / (time_s * sample_rate)).exp()
}

/// Below this combined envelope value a voice is treated as finished and its
/// lane is free for reuse.
pub const SILENCE_EPS: f32 = 1.0e-4;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decay_reaches_minus_60db_in_time() {
        let sr = 48_000.0;
        let coef = decay_coef(0.5, sr);
        let mut v = 1.0_f32;
        for _ in 0..(0.5 * sr) as usize {
            v *= coef;
        }
        // ~ -60 dB == 0.001.
        assert!((v - 0.001).abs() < 1e-4, "got {v}");
    }

    #[test]
    fn attack_rises_toward_one() {
        let sr = 48_000.0;
        let coef = attack_coef(0.002, sr);
        let mut a = 0.0_f32;
        for _ in 0..(0.002 * sr) as usize {
            a += (1.0 - a) * coef;
        }
        assert!((a - 0.632).abs() < 0.02, "one tau ≈ 63%, got {a}");
    }

    #[test]
    fn nonpositive_times_are_instant() {
        assert_eq!(decay_coef(0.0, 48_000.0), 0.0);
        assert_eq!(attack_coef(0.0, 48_000.0), 1.0);
    }
}
