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
//!    `curve(source) · cook_depth(depth) · DEST_GAIN[dest] · scale_norm(scale_src)`
//!    into a `[f32; N_DESTS]` per-dest total. Slots to the same dest **sum**
//!    (additive). `curve` shapes the *source* value (per VXN2's model); `depth`
//!    is the slot's bipolar `[-1, 1]` param (0200), passed through the dest's
//!    [`DestId::cook_depth`] taper (cubic on `Pitch`, identity elsewhere);
//!    `DEST_GAIN` converts the normalised product to the dest's native unit;
//!    `scale_src` is the per-route VCA (ADR 0009).
//!
//! The dest totals are consumed by the render loop with VXN1's
//! consumption-matched smoothing (per-sample cutoff/pitch, block-rate gains) —
//! that wiring is the render-path fork; this module is the pure maths, so the
//! routing table is unit-testable in isolation like VXN1's `resolve_mod`.
//!
//! Everything here is fixed-size and branch-light (curve dispatch is per slot,
//! not per source) — allocation-free and NEON-friendly.

use crate::matrix::{Curve, DestId, MatrixSlot, MatrixTable, N_DESTS, N_SOURCES, SourceId};

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
    /// This lane's stereo position `[-1, 1]` (0260): the allocator's fixed lane
    /// offset already scaled by the `Spread` param. Routed to [`DestId::Pan`]
    /// at depth 1 by the default patch, which is what makes unison spread work
    /// the way it always has — now as topology rather than hard wiring.
    pub spread_pos: f32,
    /// This lane's raw place in its stack `[-1, 1]` (0308): the same allocator
    /// position as `spread_pos`, but **not** scaled by the `Spread` param — for
    /// routes that want the stack's shape (fan the envelope times, the LFO
    /// rates, the cutoffs) without the pan knob in the loop.
    pub stack_pos: f32,
}

/// MIDI note of C4 — the Key source's reference. `note − 60` is semitones
/// relative to C4; dividing by 12 gives octaves (the Key source unit).
const C4_NOTE: f32 = 60.0;

/// Normalise raw inputs into the per-source lookup. Index expressions
/// (`SourceId::X.idx()`) are compile-time constants, so this is straight stores.
#[inline]
pub fn eval_sources(inp: &SourceInputs) -> SourceVals {
    let mut v = [0.0_f32; N_SOURCES];
    v[SourceId::Env1.index()] = inp.env1;
    v[SourceId::Env2.index()] = inp.env2;
    v[SourceId::Lfo1.index()] = inp.lfo1;
    v[SourceId::Lfo2.index()] = inp.lfo2;
    v[SourceId::Velocity.index()] = inp.velocity;
    // Key: signed octaves relative to C4 (see DEST_GAIN / KEY_CUTOFF_UNITY_DEPTH).
    v[SourceId::Key.index()] = (inp.note as f32 - C4_NOTE) / 12.0;
    v[SourceId::ModWheel.index()] = inp.mod_wheel;
    v[SourceId::PitchWheel.index()] = inp.pitch_wheel;
    v[SourceId::Aftertouch.index()] = inp.aftertouch;
    v[SourceId::NoteRandom.index()] = inp.note_random;
    v[SourceId::Spread.index()] = inp.spread_pos;
    v[SourceId::StackPos.index()] = inp.stack_pos;
    v
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
/// | `Pwm` | 0.5 | ±0.5 pulse-width fraction (both oscs) |
/// | `Osc1Pwm` / `Osc2Pwm` | 0.5 | ±0.5 pulse-width fraction (one osc) |
/// | `Cutoff` | 48.0 | ±48 st of cutoff |
/// | `Resonance` | 1.0 | additive `[0, 1]` |
/// | `HpfCutoff` | 48.0 | ±48 st of HPF cutoff |
/// | `Amp` | 1.0 | full VCA gain (Env2→Amp @1 = VXN1 VCA) |
/// | `CrossModAmount` | 4.0 | the 0..4 cross-mod range |
/// | `Pan` | 1.0 | ±1 pan position (hard left .. hard right) |
/// | `Env1Scale` / `Env2Scale` | 1.0 | ±1 octave of envelope time (0.5× .. 2×) |
/// | `Lfo1Rate` | 2.0 | ±2 octaves of LFO rate (0.25× .. 4×) |
/// | `Env1Sustain` / `Env2Sustain` | 1.0 | ±1 of sustain level (additive, clamped) |
///
/// **Cubic taper:** `Pitch` additionally takes a `d³` taper on the stored depth
/// before this gain ([`DestId::cook_depth`]) so vibrato-scale amounts are
/// dialable; every other dest stays linear.
pub const DEST_GAIN: [f32; N_DESTS] = {
    let mut g = [1.0_f32; N_DESTS];
    g[DestId::Pitch.index()] = 12.0;
    g[DestId::XModSweep.index()] = 48.0;
    g[DestId::Pwm.index()] = 0.5;
    g[DestId::Cutoff.index()] = 48.0;
    g[DestId::Resonance.index()] = 1.0;
    g[DestId::HpfCutoff.index()] = 48.0;
    g[DestId::Amp.index()] = 1.0;
    g[DestId::CrossModAmount.index()] = 4.0;
    // Pan's native unit *is* the normalised depth: ±1 spans the image, so a
    // route at full depth reaches hard left/right and nothing needs scaling.
    g[DestId::Pan.index()] = 1.0;
    // Same unit and gain as the combined `Pwm` (0261) — the three sum per osc,
    // so a route moved from `Pwm` to `Osc1Pwm` keeps its felt depth.
    g[DestId::Osc1Pwm.index()] = 0.5;
    g[DestId::Osc2Pwm.index()] = 0.5;
    // The envelope time scales are exponential (0268): their native unit is
    // *octaves of time*, so gain 1.0 means depth 1 reaches the 2× rail and the
    // range stays symmetric about unity (−1 → 0.5×, the same musical distance).
    g[DestId::Env1Scale.index()] = 1.0;
    g[DestId::Env2Scale.index()] = 1.0;
    // LFO rate is exponential too (0269), but wants a wider reach than the
    // envelopes: two octaves either way turns a 5 Hz wobble into a 1.25 Hz sway
    // or a 20 Hz buzz, which is the range the wheel/velocity routes are for.
    g[DestId::Lfo1Rate.index()] = 2.0;
    // Sustain is an absolute `[0, 1]` level and the dest is *additive* (0270),
    // so unity gain means depth 1 spans the full range in either direction.
    g[DestId::Env1Sustain.index()] = 1.0;
    g[DestId::Env2Sustain.index()] = 1.0;
    g
};

/// Widest envelope-time excursion, in octaves of time: ±1 octave → the 0.5×
/// .. 2.0× range of [`DestId::Env1Scale`] (0268).
const ENV_SCALE_OCTAVES: f32 = 1.0;

/// Convert an `Env1Scale` / `Env2Scale` dest total into the A/D/R **multiplier**
/// the bank applies (0268): `2^x` over the total clamped to ±[`ENV_SCALE_OCTAVES`].
///
/// Exponential rather than linear so the two directions are musically
/// symmetric — a route at `+d` lengthens by exactly as much as `−d` shortens —
/// and so summed routes *compose* (two half-depth routes at full swing land on
/// the same 2× a single full-depth one does). Clamping the exponent rather than
/// the result keeps the rails hard: no depth or stack of routes can push an
/// attack past 2× or below 0.5×.
///
/// Unity at 0 is what makes the dest free: a patch with no route (or every
/// route at depth 0) gets exactly `1.0` and the render stays bit-identical.
#[inline]
pub fn env_time_scale(total: f32) -> f32 {
    total.clamp(-ENV_SCALE_OCTAVES, ENV_SCALE_OCTAVES).exp2()
}

/// Widest LFO-rate excursion, in octaves of rate: ±2 octaves → the 0.25× .. 4×
/// range of [`DestId::Lfo1Rate`] (0269).
const LFO_RATE_OCTAVES: f32 = 2.0;

/// Convert a `Lfo1Rate` dest total into the **multiplier** on the lane's
/// resolved rate (0269): `2^x` over the total clamped to ±[`LFO_RATE_OCTAVES`].
///
/// Exponential for the same reasons as [`env_time_scale`], plus one specific to
/// rate: powers of two are the musical intervals of a tempo-synced LFO, so a
/// route at ±1 or ±2 octaves moves a synced LFO between subdivisions rather
/// than off the grid.
#[inline]
pub fn lfo_rate_scale(total: f32) -> f32 {
    total.clamp(-LFO_RATE_OCTAVES, LFO_RATE_OCTAVES).exp2()
}

/// Shape a source value through a curve (applied to the *source*, per VXN2).
/// `Bipolar` AC-couples a unipolar `[0, 1]` source to `[-1, 1]`.
///
/// `pub(crate)` because the bank's Amp factoring ([`crate::bank`]) has to fold
/// non-linear Amp routes at their block-start value and must shape them exactly
/// as the evaluator does — it used to carry its own copy of this match.
#[inline]
pub(crate) fn shape(curve: Curve, v: f32) -> f32 {
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

/// The **topology half** of a slot's gain: `cook_depth(depth) · DEST_GAIN[dest]`.
/// Depends only on the patch, so a consumer that resolves routes once per block
/// can hoist it out of its per-voice loop ([`crate::bank`]'s Amp factoring does).
#[inline]
pub(crate) fn slot_topology_gain(slot: &MatrixSlot) -> f32 {
    slot.dest.cook_depth(slot.depth) * DEST_GAIN[slot.dest.index()]
}

/// The **per-voice half** of a slot's gain: its `scale_src` VCA resolved against
/// this voice's sources, or `1.0` for an unscaled slot (ADR 0009).
#[inline]
pub(crate) fn slot_scale(slot: &MatrixSlot, sources: &SourceVals) -> f32 {
    match slot.scale_src.idx() {
        Some(sc) => scale_norm(slot.scale_src, sources[sc]),
        None => 1.0,
    }
}

/// One slot's full gain — `cook_depth(depth) · DEST_GAIN[dest] · scale_norm`.
/// The single statement of that product: [`eval_dests`] applies it to every
/// dest, and [`crate::bank`]'s Amp factoring applies its two halves separately,
/// so a new taper or scale rule lands in both without being written twice.
#[inline]
pub(crate) fn slot_gain(slot: &MatrixSlot, sources: &SourceVals) -> f32 {
    slot_topology_gain(slot) * slot_scale(slot, sources)
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
        out[di] += shape(slot.curve, sources[si]) * slot_gain(slot, sources);
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
        assert_eq!(at_c4[SourceId::Key.index()], 0.0);
        let one_oct_up = eval_sources(&SourceInputs { note: 72, ..Default::default() });
        assert_eq!(one_oct_up[SourceId::Key.index()], 1.0);
        let one_oct_down = eval_sources(&SourceInputs { note: 48, ..Default::default() });
        assert_eq!(one_oct_down[SourceId::Key.index()], -1.0);
    }

    #[test]
    fn single_route_scales_by_depth_and_gain() {
        // LFO1 (=1.0) → Cutoff (linear depth), 0.5 × 48 st = 24 st.
        let s = eval_sources(&SourceInputs { lfo1: 1.0, ..Default::default() });
        let t = table(&[slot(SourceId::Lfo1, DestId::Cutoff, 0.5, Curve::Lin)]);
        let mut out = [0.0; N_DESTS];
        eval_dests(&t, &s, &mut out);
        assert!((out[DestId::Cutoff.index()] - 24.0).abs() < 1e-5);
    }

    #[test]
    fn pitch_depth_takes_the_cubic_taper_others_stay_linear() {
        let s = eval_sources(&SourceInputs { lfo1: 1.0, ..Default::default() });
        let mut out = [0.0; N_DESTS];
        // Pitch: 0.5³ · 12 st = 1.5 st — half travel is a musical vibrato/
        // detune range, not 6 st.
        eval_dests(&table(&[slot(SourceId::Lfo1, DestId::Pitch, 0.5, Curve::Lin)]), &s, &mut out);
        assert!((out[DestId::Pitch.index()] - 1.5).abs() < 1e-6);
        // Endpoints and sign survive the taper.
        eval_dests(&table(&[slot(SourceId::Lfo1, DestId::Pitch, 1.0, Curve::Lin)]), &s, &mut out);
        assert!((out[DestId::Pitch.index()] - 12.0).abs() < 1e-6);
        eval_dests(&table(&[slot(SourceId::Lfo1, DestId::Pitch, -1.0, Curve::Lin)]), &s, &mut out);
        assert!((out[DestId::Pitch.index()] + 12.0).abs() < 1e-6);
        eval_dests(&table(&[slot(SourceId::Lfo1, DestId::Pitch, -0.5, Curve::Lin)]), &s, &mut out);
        assert!((out[DestId::Pitch.index()] + 1.5).abs() < 1e-6);
        // Every other dest is untouched: 0.5 × 48 st stays 24 st.
        for d in [DestId::XModSweep, DestId::Cutoff, DestId::HpfCutoff] {
            eval_dests(&table(&[slot(SourceId::Lfo1, d, 0.5, Curve::Lin)]), &s, &mut out);
            let want = 0.5 * DEST_GAIN[d.index()];
            assert!((out[d.index()] - want).abs() < 1e-5, "{d:?} should stay linear");
        }
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
        assert!((out[DestId::Cutoff.index()] - 48.0).abs() < 1e-5);
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
        assert_eq!(out[DestId::Pitch.index()], 0.0);
        // Wheel at 1 → full configured depth (1.0·12 st).
        let s1 = eval_sources(&SourceInputs { lfo1: 1.0, mod_wheel: 1.0, ..Default::default() });
        eval_dests(&t, &s1, &mut out);
        assert!((out[DestId::Pitch.index()] - 12.0).abs() < 1e-6);
        // Wheel at 0.5 → half.
        let sh = eval_sources(&SourceInputs { lfo1: 1.0, mod_wheel: 0.5, ..Default::default() });
        eval_dests(&t, &sh, &mut out);
        assert!((out[DestId::Pitch.index()] - 6.0).abs() < 1e-6);
    }

    #[test]
    fn bipolar_scale_src_follows_half_shift() {
        // Scale by a bipolar LFO2 at 0 → (0+1)/2 = 0.5 gate.
        let t = table(&[scaled(SourceId::Env1, DestId::Cutoff, 1.0, SourceId::Lfo2)]);
        let s = eval_sources(&SourceInputs { env1: 1.0, lfo2: 0.0, ..Default::default() });
        let mut out = [0.0; N_DESTS];
        eval_dests(&t, &s, &mut out);
        // 1.0(env) · 1.0(depth) · 48(gain) · 0.5(scale) = 24.
        assert!((out[DestId::Cutoff.index()] - 24.0).abs() < 1e-5);
    }

    #[test]
    fn curves_shape_the_source() {
        let s = eval_sources(&SourceInputs { lfo1: 0.5, ..Default::default() });
        // Exp: sign(v)·v² = 0.25; ·depth1·gain12 = 3.0.
        let mut out = [0.0; N_DESTS];
        eval_dests(&table(&[slot(SourceId::Lfo1, DestId::Pitch, 1.0, Curve::Exp)]), &s, &mut out);
        assert!((out[DestId::Pitch.index()] - 3.0).abs() < 1e-6);
        // Log: √0.5 ≈ 0.7071; ·12 ≈ 8.485.
        eval_dests(&table(&[slot(SourceId::Lfo1, DestId::Pitch, 1.0, Curve::Log)]), &s, &mut out);
        assert!((out[DestId::Pitch.index()] - 0.5_f32.sqrt() * 12.0).abs() < 1e-5);
        // Bipolar on a unipolar mod-wheel 0.5 → 2·0.5−1 = 0 → nothing.
        let sw = eval_sources(&SourceInputs { mod_wheel: 0.5, ..Default::default() });
        eval_dests(&table(&[slot(SourceId::ModWheel, DestId::Pitch, 1.0, Curve::Bipolar)]), &sw, &mut out);
        assert_eq!(out[DestId::Pitch.index()], 0.0);
    }

    #[test]
    fn env_time_scale_is_symmetric_and_railed() {
        // Unity at nothing routed — the property that keeps an unrouted patch
        // bit-identical.
        assert_eq!(env_time_scale(0.0), 1.0);
        assert!((env_time_scale(1.0) - 2.0).abs() < 1e-6);
        assert!((env_time_scale(-1.0) - 0.5).abs() < 1e-6);
        // Symmetric: +d lengthens by the factor −d shortens by.
        for d in [0.25, 0.5, 0.75] {
            assert!((env_time_scale(d) * env_time_scale(-d) - 1.0).abs() < 1e-6);
        }
        // The exponent clamps, so no stack of routes escapes [0.5, 2.0].
        assert!((env_time_scale(9.0) - 2.0).abs() < 1e-6);
        assert!((env_time_scale(-9.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn env_scale_dests_are_linear_unity_gain() {
        // Depth 1 from a full unipolar source = 1 octave of time = the 2× rail;
        // no cubic taper (the exponential mapping is the taper).
        let s = eval_sources(&SourceInputs { mod_wheel: 1.0, ..Default::default() });
        let mut out = [0.0; N_DESTS];
        for d in [DestId::Env1Scale, DestId::Env2Scale] {
            eval_dests(&table(&[slot(SourceId::ModWheel, d, 1.0, Curve::Lin)]), &s, &mut out);
            assert_eq!(out[d.index()], 1.0);
            assert!((env_time_scale(out[d.index()]) - 2.0).abs() < 1e-6);
        }
    }

    #[test]
    fn lfo_rate_scale_spans_two_octaves_either_way() {
        assert_eq!(lfo_rate_scale(0.0), 1.0);
        assert!((lfo_rate_scale(2.0) - 4.0).abs() < 1e-6);
        assert!((lfo_rate_scale(-2.0) - 0.25).abs() < 1e-6);
        assert!((lfo_rate_scale(1.0) - 2.0).abs() < 1e-6);
        // Rails hold whatever the depth stack sums to.
        assert!((lfo_rate_scale(50.0) - 4.0).abs() < 1e-6);
        assert!((lfo_rate_scale(-50.0) - 0.25).abs() < 1e-6);
        // A full-depth route from a full unipolar source reaches the rail.
        let s = eval_sources(&SourceInputs { mod_wheel: 1.0, ..Default::default() });
        let mut out = [0.0; N_DESTS];
        eval_dests(
            &table(&[slot(SourceId::ModWheel, DestId::Lfo1Rate, 1.0, Curve::Lin)]),
            &s,
            &mut out,
        );
        assert!((lfo_rate_scale(out[DestId::Lfo1Rate.index()]) - 4.0).abs() < 1e-6);
    }

    #[test]
    fn default_patch_amp_follows_env2_only() {
        // The seeded default: Env2→Amp @1, Key→Cutoff @0. Amp total = Env2 level;
        // every other dest (incl. Cutoff, since key-track depth is 0) is zero.
        let t = default_patch();
        let s = eval_sources(&SourceInputs { env2: 0.73, note: 84, ..Default::default() });
        let mut out = [0.0; N_DESTS];
        eval_dests(&t, &s, &mut out);
        assert!((out[DestId::Amp.index()] - 0.73).abs() < 1e-6);
        assert_eq!(out[DestId::Cutoff.index()], 0.0, "key-track off by default");
        for (i, &x) in out.iter().enumerate() {
            if i != DestId::Amp.index() {
                assert_eq!(x, 0.0);
            }
        }
    }

    #[test]
    fn default_vibrato_is_the_vxn1_005_st() {
        // The seeded LFO1→Pitch depth, cubically tapered × the Pitch gain, must
        // reproduce VXN1's 0.05 st default vibrato at full LFO swing.
        // Cross-checks the coupling between DEFAULT_VIBRATO_DEPTH (matrix.rs,
        // a cube-root literal), DestId::cook_depth and DEST_GAIN[Pitch] here.
        let t = default_patch();
        let s = eval_sources(&SourceInputs { lfo1: 1.0, ..Default::default() });
        let mut out = [0.0; N_DESTS];
        eval_dests(&t, &s, &mut out);
        assert!((out[DestId::Pitch.index()] - 0.05).abs() < 1e-4,
            "default vibrato should be 0.05 st, got {}", out[DestId::Pitch.index()]);
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
        assert!((out[DestId::Cutoff.index()] - 12.0).abs() < 1e-5,
            "1 octave of key should shift cutoff 12 st");
    }
}
