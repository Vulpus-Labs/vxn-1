//! Mod-matrix evaluator (ticket 0202) — the generic source→dest accumulate that
//! replaces VXN1's fixed per-channel routing (ADR 0001 §2, §4).
//!
//! Two stages, both per **control block** (sr/32), matching VXN1's modulation
//! granularity:
//!
//! 1. [`eval_sources`] normalises one voice's ten raw modulation inputs into a
//!    `[f32; N_SOURCES]` lookup indexed by [`SourceId::idx`]. Every source emits
//!    a documented shape — bipolar `[-1, 1]` (LFOs, pitch wheel) or unipolar
//!    `[0, 1]` (envelopes, velocity, wheels, aftertouch, note-random) — except
//!    **Key**, which carries signed *octaves relative to C4* so the seeded
//!    Key→Cutoff route reproduces VXN1's key-track (see [`DEST_GAIN`]).
//! 2. [`eval_dests`] walks the 16 slots and accumulates
//!    `curve(source) · depth · DEST_GAIN[dest] · scale_norm(scale_src)` into a
//!    `[f32; N_DESTS]` per-dest total. Slots to the same dest **sum**
//!    (additive). `curve` shapes the *source* value (per VXN2's model); `depth`
//!    is the slot's bipolar `[-1, 1]` param (0200); `DEST_GAIN` converts the
//!    normalised product to the dest's native unit; `scale_src` is the per-route
//!    VCA (ADR 0009).
//!
//! The dest totals are consumed by the render loop with VXN1's
//! consumption-matched smoothing (per-sample cutoff/pitch, block-rate gains) —
//! that wiring is the render-path fork; this module is the pure maths, so the
//! routing table is unit-testable in isolation like VXN1's `resolve_mod`.
//!
//! Everything here is fixed-size and branch-light (curve dispatch is per slot,
//! not per source) — allocation-free and NEON-friendly.

use crate::matrix::{Curve, DestId, MatrixTable, N_DESTS, N_SOURCES, SourceId};

/// One voice's normalised source lookup, indexed by [`SourceId::idx`].
pub type SourceVals = [f32; N_SOURCES];

/// One voice's per-dest modulation accumulator, indexed by [`DestId::idx`].
pub type DestVals = [f32; N_DESTS];

/// Raw per-voice modulation inputs at block start, in their natural units. The
/// engine fills this from the voice bank + patch each control block;
/// [`eval_sources`] normalises it into a [`SourceVals`] table.
#[derive(Clone, Copy, Debug, Default)]
pub struct SourceInputs {
    /// Env 1 level `[0, 1]`.
    pub env1: f32,
    /// Env 2 level `[0, 1]`.
    pub env2: f32,
    /// Per-voice LFO 1 `[-1, 1]`, already onset-scaled by the caller.
    pub lfo1: f32,
    /// Global LFO 2 `[-1, 1]`.
    pub lfo2: f32,
    /// Note velocity `[0, 1]`.
    pub velocity: f32,
    /// MIDI note number (0..127); normalised to octaves relative to C4.
    pub note: u8,
    /// Mod wheel `[0, 1]`.
    pub mod_wheel: f32,
    /// Pitch wheel `[-1, 1]`.
    pub pitch_wheel: f32,
    /// Per-voice aftertouch pressure `[0, 1]` (0198).
    pub aftertouch: f32,
    /// Per-voice note-on random `[0, 1)` (0199).
    pub note_random: f32,
}

/// MIDI note of C4 — the Key source's reference. `note − 60` is semitones
/// relative to C4; dividing by 12 gives octaves (the Key source unit).
const C4_NOTE: f32 = 60.0;

/// Normalise raw inputs into the per-source lookup. Index expressions
/// (`SourceId::X.idx()`) are compile-time constants, so this is straight stores.
#[inline]
pub fn eval_sources(inp: &SourceInputs) -> SourceVals {
    let mut v = [0.0_f32; N_SOURCES];
    v[idx(SourceId::Env1)] = inp.env1;
    v[idx(SourceId::Env2)] = inp.env2;
    v[idx(SourceId::Lfo1)] = inp.lfo1;
    v[idx(SourceId::Lfo2)] = inp.lfo2;
    v[idx(SourceId::Velocity)] = inp.velocity;
    // Key: signed octaves relative to C4 (see DEST_GAIN / KEY_CUTOFF_UNITY_DEPTH).
    v[idx(SourceId::Key)] = (inp.note as f32 - C4_NOTE) / 12.0;
    v[idx(SourceId::ModWheel)] = inp.mod_wheel;
    v[idx(SourceId::PitchWheel)] = inp.pitch_wheel;
    v[idx(SourceId::Aftertouch)] = inp.aftertouch;
    v[idx(SourceId::NoteRandom)] = inp.note_random;
    v
}

/// Compile-time source index (the sentinel `None` never reaches here).
#[inline]
const fn idx(s: SourceId) -> usize {
    match s.idx() {
        Some(i) => i,
        None => 0, // unreachable for real sources; keeps this const-callable
    }
}

/// Per-destination gain: converts the normalised `curve(source)·depth` product
/// (both roughly `[-1, 1]`) into the dest's native unit, so a fixed depth is
/// musically comparable across dest kinds (VXN2's `DEST_GAIN` idiom). Indexed by
/// [`DestId::idx`].
///
/// **Provisional** — matched to VXN1's fixed-route full-scale ranges (ADR 0004);
/// the render-parity work (0202 render fork) may refine individual gains. The
/// evaluator's *mechanics* (accumulation, curve, scale) are independent of these
/// constants; only the felt depth-to-effect mapping is.
///
/// | Dest | Gain | Native unit @ depth 1 |
/// |---|---|---|
/// | `Pitch` | 12.0 | ±12 st (±1 oct vibrato) |
/// | `XModSweep` | 48.0 | ±48 st (VXN1 wide sweep) |
/// | `Pwm` | 0.5 | ±0.5 pulse-width fraction |
/// | `Cutoff` | 48.0 | ±48 st of cutoff |
/// | `Resonance` | 1.0 | additive `[0, 1]` |
/// | `HpfCutoff` | 48.0 | ±48 st of HPF cutoff |
/// | `Amp` | 1.0 | full VCA gain (Env2→Amp @1 = VXN1 VCA) |
/// | `CrossModAmount` | 4.0 | the 0..4 cross-mod range |
pub const DEST_GAIN: [f32; N_DESTS] = {
    let mut g = [1.0_f32; N_DESTS];
    g[di(DestId::Pitch)] = 12.0;
    g[di(DestId::XModSweep)] = 48.0;
    g[di(DestId::Pwm)] = 0.5;
    g[di(DestId::Cutoff)] = 48.0;
    g[di(DestId::Resonance)] = 1.0;
    g[di(DestId::HpfCutoff)] = 48.0;
    g[di(DestId::Amp)] = 1.0;
    g[di(DestId::CrossModAmount)] = 4.0;
    g
};

/// Compile-time dest index (the sentinel `None` never reaches here).
#[inline]
const fn di(d: DestId) -> usize {
    match d.idx() {
        Some(i) => i,
        None => 0,
    }
}

/// Shape a source value through a curve (applied to the *source*, per VXN2).
/// `Bipolar` AC-couples a unipolar `[0, 1]` source to `[-1, 1]`.
#[inline]
fn shape(curve: Curve, v: f32) -> f32 {
    match curve {
        Curve::Lin => v,
        Curve::Exp => v.abs() * v,       // signed square
        Curve::Log => {
            let m = v.abs().sqrt();
            if v < 0.0 { -m } else { m }
        }
        Curve::Bipolar => 2.0 * v - 1.0,
    }
}

/// Normalise a scale source's value to the `[0, 1]` VCA range (ADR 0009):
/// unipolar sources pass through; bipolar map `(x + 1)·0.5`. Always clamped, so
/// an out-of-range source can't push the factor negative or past full.
/// `0 → route contributes nothing`, `1 → route at full configured depth`.
#[inline]
pub fn scale_norm(src: SourceId, v: f32) -> f32 {
    let n = if src.is_bipolar() { (v + 1.0) * 0.5 } else { v };
    n.clamp(0.0, 1.0)
}

/// Accumulate every active slot's contribution into a per-dest total for one
/// voice. Zeroes `out` first. Empty slots (`None` source/dest) and zero-depth
/// slots are skipped. Curve dispatch is per slot (out of any inner loop);
/// `scale_src` is resolved from the same [`SourceVals`] table, so it can never
/// form a cycle.
#[inline]
pub fn eval_dests(table: &MatrixTable, sources: &SourceVals, out: &mut DestVals) {
    out.fill(0.0);
    for slot in &table.slots {
        let (Some(si), Some(di)) = (slot.source.idx(), slot.dest.idx()) else {
            continue;
        };
        if slot.depth == 0.0 {
            continue;
        }
        let scale = match slot.scale_src.idx() {
            Some(sc) => scale_norm(slot.scale_src, sources[sc]),
            None => 1.0,
        };
        let gain = slot.depth * DEST_GAIN[di] * scale;
        out[di] += shape(slot.curve, sources[si]) * gain;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matrix::{MatrixSlot, default_patch};

    fn slot(source: SourceId, dest: DestId, depth: f32, curve: Curve) -> MatrixSlot {
        MatrixSlot { source, dest, depth, curve, scale_src: SourceId::None }
    }

    fn scaled(source: SourceId, dest: DestId, depth: f32, scale_src: SourceId) -> MatrixSlot {
        MatrixSlot { source, dest, depth, curve: Curve::Lin, scale_src }
    }

    fn table(slots: &[MatrixSlot]) -> MatrixTable {
        let mut t = MatrixTable::default();
        for (i, s) in slots.iter().enumerate() {
            t.slots[i] = *s;
        }
        t
    }

    #[test]
    fn key_source_is_octaves_relative_to_c4() {
        let at_c4 = eval_sources(&SourceInputs { note: 60, ..Default::default() });
        assert_eq!(at_c4[SourceId::Key.idx().unwrap()], 0.0);
        let one_oct_up = eval_sources(&SourceInputs { note: 72, ..Default::default() });
        assert_eq!(one_oct_up[SourceId::Key.idx().unwrap()], 1.0);
        let one_oct_down = eval_sources(&SourceInputs { note: 48, ..Default::default() });
        assert_eq!(one_oct_down[SourceId::Key.idx().unwrap()], -1.0);
    }

    #[test]
    fn single_route_scales_by_depth_and_gain() {
        // LFO1 (=1.0) → Pitch, depth 0.5 → 0.5 × 12 st = 6 st.
        let s = eval_sources(&SourceInputs { lfo1: 1.0, ..Default::default() });
        let t = table(&[slot(SourceId::Lfo1, DestId::Pitch, 0.5, Curve::Lin)]);
        let mut out = [0.0; N_DESTS];
        eval_dests(&t, &s, &mut out);
        assert!((out[DestId::Pitch.idx().unwrap()] - 6.0).abs() < 1e-6);
    }

    #[test]
    fn slots_to_one_dest_sum_additively() {
        let s = eval_sources(&SourceInputs { lfo1: 1.0, env1: 0.5, ..Default::default() });
        let t = table(&[
            slot(SourceId::Lfo1, DestId::Cutoff, 0.5, Curve::Lin), // 0.5·48 = 24
            slot(SourceId::Env1, DestId::Cutoff, 1.0, Curve::Lin), // 0.5·1·48 = 24
        ]);
        let mut out = [0.0; N_DESTS];
        eval_dests(&t, &s, &mut out);
        assert!((out[DestId::Cutoff.idx().unwrap()] - 48.0).abs() < 1e-5);
    }

    #[test]
    fn none_and_zero_depth_slots_are_inert() {
        let s = eval_sources(&SourceInputs { lfo1: 1.0, ..Default::default() });
        let t = table(&[
            slot(SourceId::None, DestId::Pitch, 1.0, Curve::Lin),
            slot(SourceId::Lfo1, DestId::None, 1.0, Curve::Lin),
            slot(SourceId::Lfo1, DestId::Pitch, 0.0, Curve::Lin),
        ]);
        let mut out = [0.0; N_DESTS];
        eval_dests(&t, &s, &mut out);
        assert!(out.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn scale_norm_unipolar_passthrough_bipolar_folds() {
        // Unipolar (mod wheel): 0 → 0, 1 → 1.
        assert_eq!(scale_norm(SourceId::ModWheel, 0.0), 0.0);
        assert_eq!(scale_norm(SourceId::ModWheel, 1.0), 1.0);
        // Bipolar (LFO): (x+1)/2, clamped.
        assert_eq!(scale_norm(SourceId::Lfo1, 0.0), 0.5);
        assert_eq!(scale_norm(SourceId::Lfo1, 1.0), 1.0);
        assert_eq!(scale_norm(SourceId::Lfo1, -1.0), 0.0);
        assert_eq!(scale_norm(SourceId::ModWheel, 2.0), 1.0); // clamp
    }

    #[test]
    fn scale_src_modwheel_gates_route_zero_to_full() {
        let t = table(&[scaled(SourceId::Lfo1, DestId::Pitch, 1.0, SourceId::ModWheel)]);
        // Wheel at 0 → route contributes nothing.
        let s0 = eval_sources(&SourceInputs { lfo1: 1.0, mod_wheel: 0.0, ..Default::default() });
        let mut out = [0.0; N_DESTS];
        eval_dests(&t, &s0, &mut out);
        assert_eq!(out[DestId::Pitch.idx().unwrap()], 0.0);
        // Wheel at 1 → full configured depth (1.0·12 st).
        let s1 = eval_sources(&SourceInputs { lfo1: 1.0, mod_wheel: 1.0, ..Default::default() });
        eval_dests(&t, &s1, &mut out);
        assert!((out[DestId::Pitch.idx().unwrap()] - 12.0).abs() < 1e-6);
        // Wheel at 0.5 → half.
        let sh = eval_sources(&SourceInputs { lfo1: 1.0, mod_wheel: 0.5, ..Default::default() });
        eval_dests(&t, &sh, &mut out);
        assert!((out[DestId::Pitch.idx().unwrap()] - 6.0).abs() < 1e-6);
    }

    #[test]
    fn bipolar_scale_src_follows_half_shift() {
        // Scale by a bipolar LFO2 at 0 → (0+1)/2 = 0.5 gate.
        let t = table(&[scaled(SourceId::Env1, DestId::Cutoff, 1.0, SourceId::Lfo2)]);
        let s = eval_sources(&SourceInputs { env1: 1.0, lfo2: 0.0, ..Default::default() });
        let mut out = [0.0; N_DESTS];
        eval_dests(&t, &s, &mut out);
        // 1.0(env) · 1.0(depth) · 48(gain) · 0.5(scale) = 24.
        assert!((out[DestId::Cutoff.idx().unwrap()] - 24.0).abs() < 1e-5);
    }

    #[test]
    fn curves_shape_the_source() {
        let s = eval_sources(&SourceInputs { lfo1: 0.5, ..Default::default() });
        // Exp: sign(v)·v² = 0.25; ·depth1·gain12 = 3.0.
        let mut out = [0.0; N_DESTS];
        eval_dests(&table(&[slot(SourceId::Lfo1, DestId::Pitch, 1.0, Curve::Exp)]), &s, &mut out);
        assert!((out[DestId::Pitch.idx().unwrap()] - 3.0).abs() < 1e-6);
        // Log: √0.5 ≈ 0.7071; ·12 ≈ 8.485.
        eval_dests(&table(&[slot(SourceId::Lfo1, DestId::Pitch, 1.0, Curve::Log)]), &s, &mut out);
        assert!((out[DestId::Pitch.idx().unwrap()] - 0.5_f32.sqrt() * 12.0).abs() < 1e-5);
        // Bipolar on a unipolar mod-wheel 0.5 → 2·0.5−1 = 0 → nothing.
        let sw = eval_sources(&SourceInputs { mod_wheel: 0.5, ..Default::default() });
        eval_dests(&table(&[slot(SourceId::ModWheel, DestId::Pitch, 1.0, Curve::Bipolar)]), &sw, &mut out);
        assert_eq!(out[DestId::Pitch.idx().unwrap()], 0.0);
    }

    #[test]
    fn default_patch_amp_follows_env2_only() {
        // The seeded default: Env2→Amp @1, Key→Cutoff @0. Amp total = Env2 level;
        // every other dest (incl. Cutoff, since key-track depth is 0) is zero.
        let t = default_patch();
        let s = eval_sources(&SourceInputs { env2: 0.73, note: 84, ..Default::default() });
        let mut out = [0.0; N_DESTS];
        eval_dests(&t, &s, &mut out);
        assert!((out[DestId::Amp.idx().unwrap()] - 0.73).abs() < 1e-6);
        assert_eq!(out[DestId::Cutoff.idx().unwrap()], 0.0, "key-track off by default");
        for (i, &x) in out.iter().enumerate() {
            if i != DestId::Amp.idx().unwrap() {
                assert_eq!(x, 0.0);
            }
        }
    }

    #[test]
    fn default_vibrato_is_the_vxn1_005_st() {
        // The seeded LFO1→Pitch depth × the Pitch gain must reproduce VXN1's
        // 0.05 st default vibrato at full LFO swing. Cross-checks the coupling
        // between DEFAULT_VIBRATO_DEPTH (matrix.rs) and DEST_GAIN[Pitch] here.
        let t = default_patch();
        let s = eval_sources(&SourceInputs { lfo1: 1.0, ..Default::default() });
        let mut out = [0.0; N_DESTS];
        eval_dests(&t, &s, &mut out);
        assert!((out[DestId::Pitch.idx().unwrap()] - 0.05).abs() < 1e-6,
            "default vibrato should be 0.05 st, got {}", out[DestId::Pitch.idx().unwrap()]);
    }

    #[test]
    fn key_cutoff_at_unity_depth_is_one_oct_per_oct() {
        use crate::matrix::KEY_CUTOFF_UNITY_DEPTH;
        // With Key in octaves and Cutoff gain 48, depth 0.25 gives 12 st of
        // cutoff per octave of key = 1 oct/oct.
        let t = table(&[slot(SourceId::Key, DestId::Cutoff, KEY_CUTOFF_UNITY_DEPTH, Curve::Lin)]);
        let one_up = eval_sources(&SourceInputs { note: 72, ..Default::default() }); // +1 oct
        let mut out = [0.0; N_DESTS];
        eval_dests(&t, &one_up, &mut out);
        assert!((out[DestId::Cutoff.idx().unwrap()] - 12.0).abs() < 1e-5,
            "1 octave of key should shift cutoff 12 st");
    }
}
