//! Matrix→render apply layer (ticket 0202).
//!
//! VXN1 resolves its fixed routes into a `ModOut { pitch_mod, pitch_mod_only,
//! sweep_mod, pwm_mod, cutoff_mod }` per voice at block start, then consumes it
//! in `voice_pitches` / `voice_cutoff_hz` and the VCA. VXN1b instead evaluates
//! the matrix ([`crate::eval`]) into a per-voice [`DestVals`]; this module maps
//! those totals onto **the same DSP consumption points**, so the forked render
//! loop is otherwise VXN1's, byte-for-byte.
//!
//! The mapping (VXN1b dest → VXN1 consumption):
//!
//! | Dest | Applied as |
//! |---|---|
//! | `Pitch` | semitones added to **both** oscillators (VXN1 `pitch_mod`) |
//! | `XModSweep` | semitones to the mode-gated osc (VXN1 `sweep_mod`; also subsumes VXN1's `pitch_mod_only`) |
//! | `Pwm` | pulse-width fraction added to both PWs (VXN1 `pwm_mod`) |
//! | `Cutoff` | semitones of cutoff shift (VXN1 `cutoff_mod`) |
//! | `Resonance` | additive `[0,1]` resonance offset |
//! | `HpfCutoff` | semitones of HPF-cutoff shift |
//! | `CrossModAmount` | additive cross-mod index offset |
//! | `Amp` | VCA gain (per-frame for env sources — [`crate::engine`]) |
//!
//! `XModSweep` is mode-gated exactly as VXN1 gates its sweep: Off/Ring → both
//! oscs, Sync → osc1 (the slave whose pitch creates the sweep), PM → osc2 (the
//! modulator whose pitch sets the FM index). VXN1's separate `pitch_mod_only`
//! toggle is gone — routing to `XModSweep` *is* "modulate the modulator".
//!
//! All functions here are pure (no `self`, no DSP state, no sample-rate side
//! effects), so the routing→apply maths is unit-testable in isolation like
//! VXN1's `resolve_mod` — and cross-checked against VXN1's formulas below.

use vxn_dsp::fast_exp2;

use crate::eval::DestVals;
use crate::matrix::DestId;
use crate::params::CrossModType;

/// The cross-mod mode for `XModSweep` gating (Off/Sync/PM/Ring) — the render-side
/// view of VXN1b's [`crate::params::CrossModType`].
pub use crate::params::CrossModType as Mode;

#[inline]
fn dest(d: &DestVals, id: DestId) -> f32 {
    // Sentinel `None` never indexes a real dest; every real dest has an idx.
    match id.idx() {
        Some(i) => d[i],
        None => 0.0,
    }
}

/// Per-voice pitch of osc1 / osc2 in semitones, from the matrix dest totals —
/// the VXN1b analogue of VXN1's `voice_pitches`. `base_semis` is master tune,
/// `nf` the (glided) note, `osc1_semi`/`osc2_semi` the per-osc tuning, `detune`
/// the unison cents→semis, `drift1`/`drift2` the per-osc drift.
///
/// `Pitch` moves both oscs (vibrato/bend). `XModSweep` moves the mode-gated osc
/// only. There is no `pitch_mod_only` term — see the module docs.
#[inline]
#[allow(clippy::too_many_arguments)]
pub fn voice_pitches(
    dests: &DestVals,
    mode: Mode,
    base_semis: f32,
    nf: f32,
    osc1_semi: f32,
    osc2_semi: f32,
    detune: f32,
    drift1: f32,
    drift2: f32,
) -> (f32, f32) {
    let pitch_mod = dest(dests, DestId::Pitch);
    let sweep = dest(dests, DestId::XModSweep);
    let (sweep_to_osc1, sweep_to_osc2) = match mode {
        CrossModType::Off | CrossModType::Ring => (sweep, sweep),
        CrossModType::Sync => (sweep, 0.0),
        CrossModType::Pm => (0.0, sweep),
    };
    let s1 = base_semis + nf + osc1_semi + pitch_mod + sweep_to_osc1 + detune + drift1;
    let s2 = base_semis + nf + osc2_semi + pitch_mod + sweep_to_osc2 + detune + drift2;
    (s1, s2)
}

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
    let cutoff_mod = dest(dests, DestId::Cutoff);
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

/// Pulse width for an oscillator: base PW plus the matrix `Pwm` total, clamped
/// to the VXN1 range. Both oscs take the same offset (VXN1 `pwm_mod`).
#[inline]
pub fn voice_pw(dests: &DestVals, base_pw: f32) -> f32 {
    (base_pw + dest(dests, DestId::Pwm)).clamp(0.05, 0.95)
}

/// Resonance for the block: base plus the matrix `Resonance` total, clamped
/// `[0, 1]`. (The OTA coeff builder also clamps internally.)
#[inline]
pub fn voice_resonance(dests: &DestVals, base_reso: f32) -> f32 {
    (base_reso + dest(dests, DestId::Resonance)).clamp(0.0, 1.0)
}

/// HPF cutoff in Hz: `base_hz` shifted by the matrix `HpfCutoff` total
/// (semitones via `fast_exp2`). A zero total leaves the base untouched
/// (bit-exact — `fast_exp2(0) == 1`).
#[inline]
pub fn voice_hpf_hz(dests: &DestVals, base_hz: f32) -> f32 {
    let m = dest(dests, DestId::HpfCutoff);
    if m == 0.0 {
        base_hz
    } else {
        base_hz * fast_exp2(m / 12.0)
    }
}

/// Cross-mod index (PM depth / sync sweep amount): base plus the matrix
/// `CrossModAmount` total, clamped non-negative (VXN1's `cross_mod_amount` is
/// `0..4`).
#[inline]
pub fn voice_cross_mod_amount(dests: &DestVals, base_amount: f32) -> f32 {
    (base_amount + dest(dests, DestId::CrossModAmount)).max(0.0)
}

/// VCA gain for one voice before tremolo. 0 when inactive; the bare note gate at
/// full level when `bypass` (organ mode); else the matrix `Amp` total clamped
/// non-negative. For the seeded default (Env2→Amp @1) the `Amp` total is the
/// voice's Env2 level, so this reproduces VXN1's `amp_base(env2_level)` exactly.
#[inline]
pub fn voice_amp(dests: &DestVals, active: bool, gate: bool, bypass: bool) -> f32 {
    if !active {
        0.0
    } else if bypass {
        if gate { 1.0 } else { 0.0 }
    } else {
        dest(dests, DestId::Amp).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::{DestVals, SourceInputs, eval_dests, eval_sources};
    use crate::matrix::{Curve, DestId, MatrixSlot, MatrixTable, N_DESTS, SourceId, default_patch};

    fn zeros() -> DestVals {
        [0.0; N_DESTS]
    }

    fn with(id: DestId, v: f32) -> DestVals {
        let mut d = zeros();
        d[id.idx().unwrap()] = v;
        d
    }

    #[test]
    fn pitch_moves_both_oscs_sweep_is_mode_gated() {
        // Pitch only → both oscs get it; no sweep.
        let d = with(DestId::Pitch, 3.0);
        let (s1, s2) = voice_pitches(&d, Mode::Off, 0.0, 60.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        assert_eq!((s1, s2), (63.0, 63.0));
        // Sweep under Sync → osc1 only; under PM → osc2 only.
        let d = with(DestId::XModSweep, 5.0);
        let (s1, s2) = voice_pitches(&d, Mode::Sync, 0.0, 60.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        assert_eq!((s1, s2), (65.0, 60.0));
        let (s1, s2) = voice_pitches(&d, Mode::Pm, 0.0, 60.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        assert_eq!((s1, s2), (60.0, 65.0));
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
    fn pw_and_reso_and_hpf_and_xmod_clamp_and_apply() {
        assert_eq!(voice_pw(&with(DestId::Pwm, 0.1), 0.5), 0.6);
        assert_eq!(voice_pw(&with(DestId::Pwm, 10.0), 0.5), 0.95); // clamp hi
        assert_eq!(voice_resonance(&with(DestId::Resonance, 0.3), 0.5), 0.8);
        assert_eq!(voice_resonance(&with(DestId::Resonance, 5.0), 0.5), 1.0); // clamp
        assert_eq!(voice_hpf_hz(&zeros(), 200.0), 200.0); // identity
        assert!((voice_hpf_hz(&with(DestId::HpfCutoff, 12.0), 200.0) - 400.0).abs() < 1.0);
        assert_eq!(voice_cross_mod_amount(&with(DestId::CrossModAmount, 1.0), 2.0), 3.0);
        assert_eq!(voice_cross_mod_amount(&with(DestId::CrossModAmount, -5.0), 2.0), 0.0);
    }

    #[test]
    fn amp_reproduces_vxn1_amp_base() {
        let d = with(DestId::Amp, 0.6);
        // active, not bypassed → the Amp total (= env2 in the default patch).
        assert_eq!(voice_amp(&d, true, true, false), 0.6);
        // inactive → 0.
        assert_eq!(voice_amp(&d, false, true, false), 0.0);
        // bypass (organ) → gate only, ignoring the matrix Amp.
        assert_eq!(voice_amp(&d, true, true, true), 1.0);
        assert_eq!(voice_amp(&d, true, false, true), 0.0);
    }

    #[test]
    fn default_patch_pitch_is_vxn1_vibrato_only() {
        // End-to-end through the evaluator: default patch, LFO1 at +1 → both
        // oscs get exactly VXN1's 0.05 st vibrato; nothing else moves.
        let d = {
            let t = default_patch();
            let s = eval_sources(&SourceInputs { lfo1: 1.0, env2: 0.5, note: 60, ..Default::default() });
            let mut out = zeros();
            eval_dests(&t, &s, &mut out);
            out
        };
        let (s1, s2) = voice_pitches(&d, Mode::Off, 0.0, 60.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        assert!((s1 - 60.05).abs() < 1e-5 && (s2 - 60.05).abs() < 1e-5);
        // Cutoff unchanged (key-track off), amp follows env2.
        assert_eq!(voice_cutoff_hz(&d, 1000.0, 60.0, 0.0, 0.0, 0.0, 0.0, 3.0, 0.0), 1000.0);
        assert_eq!(voice_amp(&d, true, true, false), 0.5);
    }

    #[test]
    fn scale_src_free_default_slots() {
        // A hand-built table exercising a summed dest through the apply layer.
        let mut t = MatrixTable::default();
        t.slots[0] = MatrixSlot { source: SourceId::Env1, dest: DestId::Cutoff, depth: 1.0, curve: Curve::Lin, scale_src: SourceId::None };
        let s = eval_sources(&SourceInputs { env1: 0.25, ..Default::default() });
        let mut out = zeros();
        eval_dests(&t, &s, &mut out);
        // 0.25 · 1 · 48 = 12 st → ~double.
        assert!((voice_cutoff_hz(&out, 1000.0, 60.0, 0.0, 0.0, 0.0, 0.0, 3.0, 0.0) - 2000.0).abs() < 1.0);
    }
}
