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

/// Filter cutoff in Hz for one voice: `cutoff_base` shifted by the matrix
/// `Cutoff` total (semitones) plus the per-voice drift key-track and cutoff
/// trim, all via `fast_exp2`. Mirrors VXN1's `voice_cutoff_hz` with
/// `cutoff_mod = DestVals[Cutoff]`.
///
/// `drift_keytrack` here is the *engine* key-track amount applied to the mean
/// drift (VXN1 couples the tracked cutoff to VCO drift). In VXN1b, keyboard
/// key-track is a matrix route (Key→Cutoff), already folded into the `Cutoff`
/// dest; this term is only the drift coupling, so it takes the same amount the
/// Key→Cutoff *route* uses — passed in by the caller.
#[inline]
#[allow(clippy::too_many_arguments)]
pub fn voice_cutoff_hz(
    dests: &DestVals,
    cutoff_base: f32,
    drift1: f32,
    drift2: f32,
    drift_key_track: f32,
    trim_cutoff: f32,
    trim_cutoff_cents: f32,
    drift_amount: f32,
) -> f32 {
    let cutoff_mod = dest(dests, DestId::Cutoff);
    let dk = 0.5 * (drift1 + drift2) * drift_key_track;
    let trim_semi = trim_cutoff * (trim_cutoff_cents / 100.0) * drift_amount;
    cutoff_base * fast_exp2((cutoff_mod + dk + trim_semi) / 12.0)
}

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
        let hz = voice_cutoff_hz(&zeros(), 1000.0, 0.0, 0.0, 0.0, 0.0, 3.0, 0.0);
        assert_eq!(hz, 1000.0);
    }

    #[test]
    fn cutoff_one_octave_mod_doubles() {
        let hz = voice_cutoff_hz(&with(DestId::Cutoff, 12.0), 1000.0, 0.0, 0.0, 0.0, 0.0, 3.0, 0.0);
        assert!((hz - 2000.0).abs() < 1.0, "12 st should ~double, got {hz}");
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
        assert_eq!(voice_cutoff_hz(&d, 1000.0, 0.0, 0.0, 0.0, 0.0, 3.0, 0.0), 1000.0);
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
        assert!((voice_cutoff_hz(&out, 1000.0, 0.0, 0.0, 0.0, 0.0, 3.0, 0.0) - 2000.0).abs() < 1.0);
    }
}
