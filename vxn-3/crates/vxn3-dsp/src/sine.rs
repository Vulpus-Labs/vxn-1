//! Q32 phase oscillator primitives.
//!
//! Phase is a `u32` fixed-point fraction of a cycle (`2^32` == one turn), so it
//! wraps for free on overflow. The sine is the branch-free Bhaskara+Moser
//! polynomial ported from `vxn2-dsp` — pure ALU, vectorises across lanes, ~-59
//! dB THD which is inaudible under a drum transient.

/// Q32 scale: one full cycle.
pub const PHASE_SCALE: f32 = 4_294_967_296.0; // 2^32

/// Branch-free polynomial sine of a phase fraction `p ∈ [0, 1)`. Output in
/// roughly `[-1, 1]`. (Bhaskara I sine + Moser correction.)
#[inline(always)]
pub fn fast_sine_01(p: f32) -> f32 {
    let x1 = p - 0.5;
    let x2 = x1 * 16.0 * (x1.abs() - 0.5);
    x2 + 0.225 * x2 * (x2.abs() - 1.0)
}

/// Polynomial sine of a Q32 phase.
#[inline(always)]
pub fn fast_sine_q32(phase: u32) -> f32 {
    fast_sine_01(phase as f32 * (1.0 / PHASE_SCALE))
}

/// Phase increment per sample (Q32, as `f32` so it can be scaled by a pitch
/// multiplier before truncation) for `freq_hz` at `sample_rate`. Float→int
/// casts saturate in Rust, so an out-of-range frequency clamps harmlessly.
#[inline(always)]
pub fn phase_inc_hz(freq_hz: f32, sample_rate: f32) -> f32 {
    (freq_hz / sample_rate) * PHASE_SCALE
}

/// Equal-tempered MIDI note (fractional allowed) → frequency in Hz.
#[inline(always)]
pub fn note_to_freq(note: f32) -> f32 {
    440.0 * ((note - 69.0) / 12.0).exp2()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_hits_cardinal_points() {
        assert!((fast_sine_01(0.0)).abs() < 0.02, "sin(0)");
        assert!((fast_sine_01(0.25) - 1.0).abs() < 0.02, "sin(pi/2)");
        assert!((fast_sine_01(0.5)).abs() < 0.02, "sin(pi)");
        assert!((fast_sine_01(0.75) + 1.0).abs() < 0.02, "sin(3pi/2)");
    }

    #[test]
    fn a4_is_440() {
        assert!((note_to_freq(69.0) - 440.0).abs() < 1e-3);
        assert!((note_to_freq(57.0) - 220.0).abs() < 1e-3); // A3
    }

    #[test]
    fn phase_inc_wraps_one_cycle_per_period() {
        // 1 Hz at 4 samples/sec → quarter turn per sample.
        let inc = phase_inc_hz(1.0, 4.0);
        assert!((inc - PHASE_SCALE / 4.0).abs() < 1.0);
    }
}
