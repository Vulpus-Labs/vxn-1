//! MIDI note utilities.

/// Reference frequency for V/oct: MIDI note 0 (C-1) ≈ 8.1758 Hz.
pub const MIDI_0_HZ: f32 = 8.175_799;

/// Convert a MIDI note number (with fractional cents/bend) to frequency in Hz.
///
/// `f32` rather than `u8` so callers can pass `note + bend_semitones` directly
/// (pitch-bend, glide, mod-wheel pitch depth) without rounding.
///
/// Anchored on A4 = 440 Hz (`440 · 2^((note−69)/12)`) rather than the
/// `MIDI_0_HZ · 2^(note/12)` form: the A4 anchor is exact and this is the
/// formula vxn-2's operator core was already shipping, so consuming this fn
/// leaves vxn-2's render hash bit-for-bit unchanged (E027/0117). Note that
/// vxn-1's audio path uses its own `fast_exp2`-based `note_to_hz`
/// (`vxn-dsp`), a deliberately separate fast variant — this one is the
/// `std`-accurate shared definition.
#[inline]
pub fn note_to_hz(note: f32) -> f32 {
    // `powf` (not `exp2`) deliberately: it's the exact op vxn-2 shipped, so
    // integer-note results stay bit-identical (`exp2` can differ by an ULP).
    440.0 * 2_f32.powf((note - 69.0) / 12.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a4_is_440() {
        // MIDI 69 = A4 = 440 Hz exactly (modulo float).
        assert!((note_to_hz(69.0) - 440.0).abs() < 1e-3);
    }

    #[test]
    fn octave_doubles() {
        let a4 = note_to_hz(69.0);
        let a5 = note_to_hz(81.0);
        assert!((a5 / a4 - 2.0).abs() < 1e-5);
    }

    #[test]
    fn midi_0_matches_constant() {
        assert!((note_to_hz(0.0) - MIDI_0_HZ).abs() < 1e-4);
    }

    #[test]
    fn fractional_note_for_bend() {
        // 1 semitone up = 2^(1/12) ≈ 1.0594631.
        let a4 = note_to_hz(69.0);
        let bend = note_to_hz(70.0);
        assert!((bend / a4 - 2.0_f32.powf(1.0 / 12.0)).abs() < 1e-5);
    }
}
