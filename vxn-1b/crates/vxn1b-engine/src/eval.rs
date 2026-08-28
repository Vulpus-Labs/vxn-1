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

use crate::matrix::{
    Curve, DestId, MatrixSlot, MatrixTable, N_DESTS, N_SLOTS, N_SOURCES, SourceId,
};

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

/// Widest envelope-time excursion, in octaves of time: ±1 octave → the
/// 0.5× .. 2.0× range of [`DestId::Env1Scale`].
const ENV_SCALE_OCTAVES: f32 = 1.0;

/// Convert an `Env1Scale` / `Env2Scale` dest total into the A/D/R **multiplier**
/// the bank applies: `2^x` over the total clamped to ±[`ENV_SCALE_OCTAVES`].
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
/// range of [`DestId::Lfo1Rate`].
const LFO_RATE_OCTAVES: f32 = 2.0;

/// Convert a `Lfo1Rate` dest total into the **multiplier** on the lane's
/// resolved rate: `2^x` over the total clamped to ±[`LFO_RATE_OCTAVES`].
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

// ── bank-wide evaluation ────────────────────────────────────────────────────

/// One active slot with its **lane-invariant half already resolved**.
///
/// [`eval_dests`] recomputes all of this per voice: the two sentinel checks,
/// the zero-depth skip, the `cook_depth` taper and the `DEST_GAIN` lookup are
/// pure functions of the patch, yet a 32-lane synth ran them 32 times a block.
/// Compiling them out once is half of what makes [`eval_dests_bank`] cheaper;
/// the other half is that `curve` and `scale_src` become **outer**-loop
/// dispatch, so the lane loop underneath is branch-free and vectorises.
#[derive(Clone, Copy, Debug)]
pub struct Route {
    /// Index into [`SourceVals`].
    pub src: u8,
    /// Index into [`DestVals`].
    pub dest: u8,
    /// Shape applied to the source value.
    pub curve: Curve,
    /// [`slot_topology_gain`] — `cook_depth(depth) · DEST_GAIN[dest]`.
    pub gain: f32,
    /// The per-route VCA's source index, or `None` for an unscaled route.
    pub scale: Option<u8>,
    /// Whether that VCA source is bipolar (so [`scale_norm`]'s two arms also
    /// hoist out of the lane loop).
    pub scale_bipolar: bool,
}

/// The block's active routes, compiled once from the patch.
///
/// Slot order is preserved, which is what keeps [`eval_dests_bank`] bit-exact
/// against [`eval_dests`]: dests accumulate additively, and float addition is
/// not associative, so "same routes in the same order" is the whole contract.
#[derive(Clone, Copy, Debug)]
pub struct RouteList {
    routes: [Route; N_SLOTS],
    n: usize,
}

impl RouteList {
    /// Resolve a patch's slots into active routes. Empty (`None` endpoint) and
    /// zero-depth slots are dropped here rather than branched over per lane.
    pub fn compile(table: &MatrixTable) -> Self {
        let mut routes = [Route {
            src: 0,
            dest: 0,
            curve: Curve::Lin,
            gain: 0.0,
            scale: None,
            scale_bipolar: false,
        }; N_SLOTS];
        let mut n = 0;
        for slot in &table.slots {
            let (Some(si), Some(di)) = (slot.source.idx(), slot.dest.idx()) else {
                continue;
            };
            if slot.depth == 0.0 {
                continue;
            }
            routes[n] = Route {
                src: si as u8,
                dest: di as u8,
                curve: slot.curve,
                gain: slot_topology_gain(slot),
                scale: slot.scale_src.idx().map(|sc| sc as u8),
                scale_bipolar: slot.scale_src.is_bipolar(),
            };
            n += 1;
        }
        Self { routes, n }
    }

    /// The active routes, in slot order.
    #[inline]
    pub fn active(&self) -> &[Route] {
        &self.routes[..self.n]
    }

    /// Whether any slot is live. A patch with an empty matrix skips the pass.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
}

/// Lane-major source table: `[source][lane]`.
pub type SourceLanesSoa<const L: usize> = [[f32; L]; N_SOURCES];
/// Lane-major dest accumulator: `[dest][lane]`.
pub type DestLanesSoa<const L: usize> = [[f32; L]; N_DESTS];

/// [`eval_dests`] for a whole lane bank at once.
///
/// Identical arithmetic in an identical order — see [`RouteList`] — but
/// transposed: the outer loop is routes, the inner loop is lanes, and every
/// branch the scalar form ran per lane (sentinel, zero depth, curve match,
/// scale-source match, bipolar test) has been hoisted above the inner loop or
/// compiled away. What is left is a contiguous multiply-accumulate over `L`
/// contiguous floats, which LLVM contracts to NEON.
///
/// The scatter goes too. `out[di] += …` with a runtime `di` serialised on a
/// store-to-load chain whenever two slots shared a dest; here a route owns its
/// dest row for the whole inner loop.
#[inline]
pub fn eval_dests_bank<const L: usize>(
    routes: &RouteList,
    src: &SourceLanesSoa<L>,
    out: &mut DestLanesSoa<L>,
) {
    for row in out.iter_mut() {
        row.fill(0.0);
    }
    // The per-route VCA, resolved for every lane before the accumulate. Kept
    // outside the route loop so it is written, not allocated, per route.
    let mut scale = [1.0f32; L];
    for r in routes.active() {
        match r.scale {
            None => scale = [1.0; L],
            Some(sc) => {
                let s = &src[sc as usize];
                if r.scale_bipolar {
                    for l in 0..L {
                        scale[l] = ((s[l] + 1.0) * 0.5).clamp(0.0, 1.0);
                    }
                } else {
                    for l in 0..L {
                        scale[l] = s[l].clamp(0.0, 1.0);
                    }
                }
            }
        }
        let s = &src[r.src as usize];
        let row = &mut out[r.dest as usize];
        let g = r.gain;
        // `shape(v) * (gain * scale)` — the association matters. The scalar
        // form multiplies by `slot_gain`, which is `topology * scale` already
        // folded, so grouping them the other way would round differently and
        // cost the bit-exactness the parity test asserts.
        macro_rules! accumulate {
            ($shape:expr) => {
                for l in 0..L {
                    row[l] += $shape(s[l]) * (g * scale[l]);
                }
            };
        }
        match r.curve {
            Curve::Lin => accumulate!(|v: f32| v),
            Curve::Exp => accumulate!(|v: f32| v.abs() * v),
            Curve::Log => accumulate!(|v: f32| {
                let m = v.abs().sqrt();
                if v < 0.0 { -m } else { m }
            }),
            Curve::Bipolar => accumulate!(|v: f32| 2.0 * v - 1.0),
        }
    }
}

/// Transpose `L` voices' source tables into the lane-major layout
/// [`eval_dests_bank`] reads.
#[inline]
pub fn sources_to_soa<const L: usize>(per_lane: &[SourceVals; L]) -> SourceLanesSoa<L> {
    let mut soa = [[0.0f32; L]; N_SOURCES];
    for (l, vals) in per_lane.iter().enumerate() {
        for (s, &v) in vals.iter().enumerate() {
            soa[s][l] = v;
        }
    }
    soa
}

/// Read one lane's dest totals back out of the lane-major accumulator.
///
/// Per lane, not one block transpose: the whole-bank version writes with a
/// `N_DESTS`-float stride and measured 95 ns/quantum *slower* — strided stores
/// cost more than the strided loads they replace.
#[inline]
pub fn dests_for_lane<const L: usize>(soa: &DestLanesSoa<L>, lane: usize) -> DestVals {
    let mut d = [0.0f32; N_DESTS];
    for (i, row) in soa.iter().enumerate() {
        d[i] = row[lane];
    }
    d
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

    /// The bank evaluator must be the scalar one, lane for lane, **bit-exact**
    /// — not close. It is a transposition, not a reformulation: any drift means
    /// an operation was reassociated, and the two paths would then voice the
    /// same patch differently depending on how many lanes were live.
    #[test]
    fn eval_dests_bank_is_bit_exact_against_the_scalar_form() {
        const L: usize = 8;
        // A deterministic spread of patches: every curve, scaled and unscaled
        // routes, several slots sharing a dest (the accumulate order case), and
        // inert slots interleaved so compaction is exercised.
        let mut rng = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        let curves = [Curve::Lin, Curve::Exp, Curve::Log, Curve::Bipolar];
        for _ in 0..200 {
            let mut t = MatrixTable::default();
            for slot in t.slots.iter_mut() {
                let pick = next();
                *slot = MatrixSlot {
                    source: SourceId::from_u8((pick % (N_SOURCES as u64 + 1)) as u8),
                    dest: DestId::from_u8(((pick >> 8) % (N_DESTS as u64 + 1)) as u8),
                    depth: ((pick >> 16) % 2001) as f32 / 1000.0 - 1.0,
                    curve: curves[((pick >> 32) % 4) as usize],
                    scale_src: SourceId::from_u8(((pick >> 40) % (N_SOURCES as u64 + 1)) as u8),
                };
            }
            let mut per_lane = [[0.0f32; N_SOURCES]; L];
            for lane in per_lane.iter_mut() {
                for v in lane.iter_mut() {
                    *v = ((next() % 4001) as f32 / 2000.0) - 1.0;
                }
            }

            let routes = RouteList::compile(&t);
            let mut soa = [[0.0f32; L]; N_DESTS];
            eval_dests_bank(&routes, &sources_to_soa(&per_lane), &mut soa);

            for (l, sources) in per_lane.iter().enumerate() {
                let mut want = [0.0f32; N_DESTS];
                eval_dests(&t, sources, &mut want);
                assert_eq!(
                    dests_for_lane(&soa, l).to_bits_array(),
                    want.to_bits_array(),
                    "lane {l}"
                );
            }
        }
    }

    /// Compare by bit pattern, so a `-0.0` / `0.0` swap or a NaN cannot pass.
    trait BitsArray {
        fn to_bits_array(&self) -> [u32; N_DESTS];
    }
    impl BitsArray for DestVals {
        fn to_bits_array(&self) -> [u32; N_DESTS] {
            std::array::from_fn(|i| self[i].to_bits())
        }
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
