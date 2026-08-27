//! Pure dest→parameter consumers for the mod matrix (ticket 0202, trimmed 0273).
//!
//! VXN1 resolves its fixed routes into a `ModOut { pitch_mod, pitch_mod_only,
//! sweep_mod, pwm_mod, cutoff_mod }` per voice at block start, then consumes it
//! in `voice_cutoff_hz` / the VCA. VXN1b instead evaluates the matrix
//! ([`crate::eval`]) into a per-voice [`DestVals`]; this module holds the
//! consumers that are **pure functions of that table** — no smoothing state, no
//! sample rate, no `self` — so the routing→apply maths stays unit-testable in
//! isolation like VXN1's `resolve_mod`.
//!
//! | Dest | Consumer |
//! |---|---|
//! | `Cutoff` | [`voice_cutoff_hz`] — semitones of cutoff shift, plus the param's key-track |
//! | `Resonance` | [`voice_resonance`] — additive `[0,1]` offset |
//! | `HpfCutoff` | [`voice_hpf_hz`] — semitones of HPF-cutoff shift (0272) |
//! | `Pwm` + `Osc1Pwm`/`Osc2Pwm` | [`pwm_offset`] — the per-oscillator sum, before the clamp |
//!
//! **The rest of the dests are consumed in [`crate::bank`], not here** — and
//! deliberately so. `Pitch`, `XModSweep`, `Pan`, `Amp` and `CrossModAmount` all
//! pass through a [`crate::mod_smoothing::MotionSmoother`] between the dest
//! total and the DSP, so their apply step is inseparable from the render loop's
//! per-quantum tick.
//!
//! **The rule when adding a dest:** if the bank has to smooth it, its statement
//! lives in the bank (`sweep_gates`, `cooked_pw`, `cooked_pm_index`, `vca`) and
//! is tested there. A pure duplicate here would only go unreachable — that is
//! what happened to five of them before 0273 deleted the lot.

use vxn_dsp::fast_exp2;

use crate::eval::DestVals;
use crate::matrix::DestId;

/// Filter cutoff in Hz for one voice: `cutoff_base` shifted by key-track, the
/// matrix `Cutoff` total (semitones), the per-voice drift key-track and the
/// cutoff trim, all via `fast_exp2`. Mirrors VXN1's `voice_cutoff_hz` +
/// `resolve_mod`'s key-track term, with `cutoff_mod = DestVals[Cutoff]`.
///
/// `key_track` is the [`FilterKeyTrack`](crate::params::ParamId) amount and, as
/// in VXN1, it does two jobs at the same depth (0245):
///
/// - the **static note term**, `(note − 12) · key_track` semitones — pivoting at
///   C0 (MIDI 12), so `1.0` is 1 oct of cutoff per oct of key with the cutoff
///   *equal to* the played note when `cutoff_base` sits at its C0 minimum;
/// - the **drift coupling**, the same amount applied to the mean osc drift —
///   the keyboard CV a real VCF tracks carries the VCO's drift, so a tracked
///   cutoff wanders with it.
///
/// `note` is the raw note-on MIDI note (VXN1 tracks the played note, not the
/// glided pitch). A Key→Cutoff matrix route is *additional* free-form tracking
/// and arrives inside `dests`.
#[inline]
#[allow(clippy::too_many_arguments)]
pub fn voice_cutoff_hz(
    dests: &DestVals,
    cutoff_base: f32,
    note: f32,
    key_track: f32,
    drift1: f32,
    drift2: f32,
    trim_cutoff: f32,
    trim_cutoff_cents: f32,
    drift_amount: f32,
) -> f32 {
    let cutoff_mod = dests[DestId::Cutoff.index()];
    let kt = (note - C0_NOTE) * key_track;
    let dk = 0.5 * (drift1 + drift2) * key_track;
    let trim_semi = trim_cutoff * (trim_cutoff_cents / 100.0) * drift_amount;
    cutoff_base * fast_exp2((cutoff_mod + kt + dk + trim_semi) / 12.0)
}

/// MIDI note of C0 — the key-track pivot, matching VXN1's `resolve_mod`
/// (`(note − 12) · amt`). The cutoff param's minimum is C0's frequency
/// (16.3516 Hz), which is what makes "cutoff at minimum + key-track at 1.0 ⇒
/// cutoff is the played note" hold; the two calibrations are a pair.
const C0_NOTE: f32 = 12.0;

/// The matrix pulse-width offset for one oscillator (0261): the combined `Pwm`
/// total plus that oscillator's own dest. Summing the two dests *before* the
/// clamp (and before the block-rate one-pole in [`crate::bank`]) is what makes a
/// patch using only `Pwm` behave exactly as it did before the split.
#[inline]
pub fn pwm_offset(dests: &DestVals, per_osc: DestId) -> f32 {
    dests[DestId::Pwm.index()] + dests[per_osc.index()]
}

/// Resonance for the block: base plus the matrix `Resonance` total, clamped
/// `[0, 1]`. (The OTA coeff builder also clamps internally.)
#[inline]
pub fn voice_resonance(dests: &DestVals, base_reso: f32) -> f32 {
    (base_reso + dests[DestId::Resonance.index()]).clamp(0.0, 1.0)
}

/// HPF cutoff in Hz: `base_hz` shifted by the matrix `HpfCutoff` total
/// (semitones via `fast_exp2`). A zero total leaves the base untouched
/// (bit-exact — `fast_exp2(0) == 1`).
#[inline]
pub fn voice_hpf_hz(dests: &DestVals, base_hz: f32) -> f32 {
    let m = dests[DestId::HpfCutoff.index()];
    if m == 0.0 {
        base_hz
    } else {
        base_hz * fast_exp2(m / 12.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::{DestVals, SourceInputs, eval_dests, eval_sources};
    use crate::matrix::{Curve, DestId, MatrixSlot, MatrixTable, N_DESTS, SourceId};

    fn zeros() -> DestVals {
        [0.0; N_DESTS]
    }

    fn with(id: DestId, v: f32) -> DestVals {
        let mut d = zeros();
        d[id.index()] = v;
        d
    }

    #[test]
    fn cutoff_zero_mod_is_identity() {
        // fast_exp2(0) == 1, so a zero Cutoff total leaves cutoff_base exact.
        let hz = voice_cutoff_hz(&zeros(), 1000.0, 60.0, 0.0, 0.0, 0.0, 0.0, 3.0, 0.0);
        assert_eq!(hz, 1000.0);
    }

    #[test]
    fn cutoff_one_octave_mod_doubles() {
        let hz = voice_cutoff_hz(&with(DestId::Cutoff, 12.0), 1000.0, 60.0, 0.0, 0.0, 0.0, 0.0, 3.0, 0.0);
        assert!((hz - 2000.0).abs() < 1.0, "12 st should ~double, got {hz}");
    }

    #[test]
    fn key_track_matches_vxn1s_pivot_and_slope() {
        // VXN1's `resolve_mod`: `(note − 12) · amt` semitones of cutoff. Check
        // the *absolute* shift, not just the slope — the C0 pivot is the point.
        for note in [12.0, 36.0, 60.0, 69.0, 96.0] {
            let hz = voice_cutoff_hz(&zeros(), 1000.0, note, 1.0, 0.0, 0.0, 0.0, 3.0, 0.0);
            let want = 1000.0 * (2.0f32).powf((note - 12.0) / 12.0);
            assert!((hz / want - 1.0).abs() < 1e-3, "note {note}: {hz} Hz, want {want}");
        }
        // Amount scales it linearly, and zero is exactly inert.
        let half = voice_cutoff_hz(&zeros(), 1000.0, 24.0, 0.5, 0.0, 0.0, 0.0, 3.0, 0.0);
        assert!((half / (1000.0 * (2.0f32).powf(0.5)) - 1.0).abs() < 1e-3);
        assert_eq!(voice_cutoff_hz(&zeros(), 1000.0, 96.0, 0.0, 0.0, 0.0, 0.0, 3.0, 0.0), 1000.0);
    }

    #[test]
    fn full_key_track_at_min_cutoff_is_the_played_note() {
        // The pair of calibrations (0245): the cutoff param's minimum is C0's
        // frequency, so key-track at 1.0 puts the cutoff *on* the played pitch.
        for (note, hz_want) in [(69.0, 440.0), (60.0, 261.626), (12.0, 16.3516)] {
            let hz = voice_cutoff_hz(&zeros(), 16.3516, note, 1.0, 0.0, 0.0, 0.0, 3.0, 0.0);
            assert!((hz / hz_want - 1.0).abs() < 1e-3, "note {note}: {hz} Hz, want {hz_want}");
        }
    }

    #[test]
    fn key_track_amount_also_drives_the_drift_coupling() {
        // VXN1 tracks the VCF to the *drifted* pitch at the same amount (0218),
        // and 0245 keeps that tied to the param rather than to matrix topology:
        // no Key→Cutoff route exists here at all.
        let base = voice_cutoff_hz(&zeros(), 1000.0, 12.0, 1.0, 0.0, 0.0, 0.0, 3.0, 0.0);
        let drifted = voice_cutoff_hz(&zeros(), 1000.0, 12.0, 1.0, 0.4, 0.2, 0.0, 3.0, 0.0);
        // Mean drift 0.3 st at amount 1.0.
        assert!((drifted / (base * (2.0f32).powf(0.3 / 12.0)) - 1.0).abs() < 1e-3);
        // At amount 0 the drift coupling vanishes with the tracking.
        let off = voice_cutoff_hz(&zeros(), 1000.0, 12.0, 0.0, 0.4, 0.2, 0.0, 3.0, 0.0);
        assert_eq!(off, 1000.0);
    }

    #[test]
    fn matrix_key_route_stacks_on_top_of_the_param() {
        // A Key→Cutoff route is *extra* tracking: C4-pivoted like every other
        // Key route, summed with the param's C0-pivoted term.
        use crate::matrix::KEY_CUTOFF_UNITY_DEPTH;
        let mut t = MatrixTable::default();
        t.slots[0] = MatrixSlot {
            source: SourceId::Key,
            dest: DestId::Cutoff,
            depth: KEY_CUTOFF_UNITY_DEPTH,
            curve: Curve::Lin,
            scale_src: SourceId::None,
        };
        let s = eval_sources(&SourceInputs { note: 72, ..Default::default() });
        let mut out = zeros();
        eval_dests(&t, &s, &mut out);
        // Param: 72 − 12 = 60 st. Route: (72 − 60)/12 · 0.25 · 48 = 12 st. Sum 72.
        let hz = voice_cutoff_hz(&out, 100.0, 72.0, 1.0, 0.0, 0.0, 0.0, 3.0, 0.0);
        let want = 100.0 * (2.0f32).powf(72.0 / 12.0);
        assert!((hz / want - 1.0).abs() < 1e-3, "{hz} Hz, want {want}");
    }

    #[test]
    fn resonance_and_hpf_apply_and_clamp() {
        assert_eq!(voice_resonance(&with(DestId::Resonance, 0.3), 0.5), 0.8);
        assert_eq!(voice_resonance(&with(DestId::Resonance, 5.0), 0.5), 1.0); // clamp
        assert_eq!(voice_resonance(&with(DestId::Resonance, -5.0), 0.5), 0.0); // clamp
        assert_eq!(voice_hpf_hz(&zeros(), 200.0), 200.0); // identity — fast_exp2(0) == 1
        assert!((voice_hpf_hz(&with(DestId::HpfCutoff, 12.0), 200.0) - 400.0).abs() < 1.0);
        assert!((voice_hpf_hz(&with(DestId::HpfCutoff, -12.0), 200.0) - 100.0).abs() < 1.0);
    }

    /// 0261: the per-osc dests move one width each and sum with the combined
    /// `Pwm`. The clamp that used to be tested here is the bank's — see
    /// `bank::tests::cooked_pw_clamps_each_oscillator_independently`.
    #[test]
    fn per_osc_pwm_dests_split_and_sum() {
        // Osc 1 alone moves; osc 2 sees nothing.
        let d = with(DestId::Osc1Pwm, 0.2);
        assert_eq!(pwm_offset(&d, DestId::Osc1Pwm), 0.2);
        assert_eq!(pwm_offset(&d, DestId::Osc2Pwm), 0.0);

        // The combined dest reaches both, identically.
        let d = with(DestId::Pwm, 0.1);
        assert_eq!(pwm_offset(&d, DestId::Osc1Pwm), 0.1);
        assert_eq!(pwm_offset(&d, DestId::Osc2Pwm), 0.1);

        // Combined + per-osc sum on osc 1; osc 2 sees the combined alone.
        let mut d = with(DestId::Pwm, 0.1);
        d[DestId::Osc1Pwm.index()] = 0.2;
        assert!((pwm_offset(&d, DestId::Osc1Pwm) - 0.3).abs() < 1e-6);
        assert!((pwm_offset(&d, DestId::Osc2Pwm) - 0.1).abs() < 1e-6);
    }
}
