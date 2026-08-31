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

use vxn_core_matrix::curve::{
    bend_exp, bend_lin, bend_log, clamp_unit, fold_bipolar, fold_unipolar, pol_abs, pol_bipolar,
    pol_direct, shape_exp, shape_lin, shape_log,
};

use crate::matrix::{
    DestId, MatrixSlot, MatrixTable, N_DESTS, N_SLOTS, N_SOURCES, Polarity, Shape, SourceId,
};

/// The shaping arithmetic itself, re-exported from
/// [`vxn_core_matrix::curve`] so that `crate::eval::shape` keeps meaning what it
/// always did (0330).
///
/// [`shape`] stays `pub` here for the same reason it was `pub(crate)` before:
/// the bank's Amp factoring ([`crate::bank`]) has to fold non-linear Amp routes
/// at their block-start value and must shape them exactly as this evaluator
/// does, so it composes the same two functions rather than spelling the
/// arithmetic a second time.
pub use vxn_core_matrix::curve::{bend, bend_unit, map_polarity, scale_norm, shape};

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
/// Generated from the destination row list (0332) and re-exported here under the
/// name it has always had: each value is the `gain =` column of its row, and the
/// row is also where that gain's rationale lives. It was a hand-written table in
/// this module until three other structures keyed on the same destination —
/// the taper, the tier and the smoothing class — grew into three more places to
/// remember.
///
/// **Provisional** — matched to VXN1's fixed-route full-scale ranges (ADR 0004);
/// the render-parity work (0202 render fork) may refine individual gains. The
/// evaluator's *mechanics* (accumulation, curve, scale) are independent of these
/// constants; only the felt depth-to-effect mapping is.
pub use crate::matrix::ROSTER_DEST_GAIN as DEST_GAIN;

/// The alias holds only while the evaluator's index and the roster's are the
/// same number: the generated table drops the sentinel, so its row 0 is the
/// first real destination and [`DestId::index`] must agree, row for row.
const _: () = {
    assert!(DEST_GAIN.len() == N_DESTS);
    let mut i = 0;
    while i < N_DESTS {
        // Ask `index()` for the row rather than assuming it is `i`: that is the
        // agreement being pinned, and indexing by `i` would hold by
        // construction of the macro whatever `index()` did.
        assert!(DEST_GAIN[DestId::ALL[i + 1].index()] == DestId::ALL[i + 1].gain());
        i += 1;
    }
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
        Some(sc) => scale_norm(slot.scale_src.is_bipolar(), sources[sc], slot.scale_shape),
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
        // `is_active` is the switch *and* both endpoints — the same predicate
        // `RouteList::compile` drops on, which is what keeps the two evaluators
        // bit-exact against each other.
        if !slot.is_active() || slot.depth == 0.0 {
            continue;
        }
        let (Some(si), Some(di)) = (slot.source.idx(), slot.dest.idx()) else {
            continue;
        };
        out[di] += shape(slot.polarity, slot.shape, sources[si]) * slot_gain(slot, sources);
    }
}

// ── bank-wide evaluation ────────────────────────────────────────────────────

/// One active slot with its **lane-invariant half already resolved**, and the
/// block's list of them — both shared with VXN2 as of 0333.
///
/// [`eval_dests`] recomputes all of it per voice: the two sentinel checks, the
/// on/off switch, the zero-depth skip, the `cook_depth` taper and the dest-gain
/// lookup are pure functions of the patch, yet a whole bank of lanes ran them
/// once each per block. Compiling them out once is half of what makes
/// [`eval_dests_bank`] cheaper; the other half is that `curve` and `scale_src`
/// become **outer**-loop dispatch, so the lane loop underneath is branch-free
/// and vectorises.
///
/// [`RouteList::compile`] takes the **raw** depth and cooks it itself, which is
/// why this synth's slots must never store a cooked one. Slot order survives
/// compilation, which is what keeps [`eval_dests_bank`] bit-exact against
/// [`eval_dests`]: dests accumulate additively, float addition is not
/// associative, so "same routes in the same order" is the whole contract.
pub use vxn_core_matrix::slot::Route;

/// The block's active routes, compiled once from the patch — see [`Route`].
pub type RouteList = vxn_core_matrix::slot::RouteList<N_SLOTS>;

/// Lane-major source table: `[source][lane]`.
pub type SourceLanesSoa<const L: usize> = [[f32; L]; N_SOURCES];
/// Lane-major dest accumulator: `[dest][lane]`.
pub type DestLanesSoa<const L: usize> = [[f32; L]; N_DESTS];

/// [`eval_dests`] for a whole lane bank at once.
///
/// Identical arithmetic in an identical order — see [`RouteList`] — but
/// transposed: the outer loop is routes, the inner loop is lanes, and every
/// branch the scalar form ran per lane (sentinel, off switch, zero depth,
/// polarity match, shape match, scale-source match, bipolar test, scale bend)
/// has been hoisted above the inner loop or compiled away. What is left is a
/// contiguous multiply-accumulate over `L` contiguous floats, which LLVM
/// contracts to NEON.
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
                // Fold and bend are both per-route constants, so both are
                // dispatched here — six straight-line arms rather than a
                // `scale_norm` call carrying two branches into the lane loop.
                // The arms are the shared crate's free functions, which is what
                // keeps this loop's arithmetic and `scale_norm`'s the same
                // arithmetic rather than two spellings that agree today.
                macro_rules! vca {
                    ($fold:path, $bend:path) => {
                        for l in 0..L {
                            scale[l] = $bend(clamp_unit($fold(s[l])));
                        }
                    };
                }
                match (r.scale_bipolar, r.scale_shape) {
                    (false, Shape::Lin) => vca!(fold_unipolar, bend_lin),
                    (false, Shape::Exp) => vca!(fold_unipolar, bend_exp),
                    (false, Shape::Log) => vca!(fold_unipolar, bend_log),
                    (true, Shape::Lin) => vca!(fold_bipolar, bend_lin),
                    (true, Shape::Exp) => vca!(fold_bipolar, bend_exp),
                    (true, Shape::Log) => vca!(fold_bipolar, bend_log),
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
        // Polarity x shape, dispatched once per route. Nine arms, each a
        // straight-line multiply-accumulate over L contiguous floats, built from
        // the same shared maps and bends [`shape`] dispatches on.
        macro_rules! arm {
            ($pol:path, $bend:path) => {
                accumulate!(|v: f32| $bend($pol(v)))
            };
        }
        match (r.polarity, r.shape) {
            (Polarity::Direct, Shape::Lin) => arm!(pol_direct, shape_lin),
            (Polarity::Direct, Shape::Exp) => arm!(pol_direct, shape_exp),
            (Polarity::Direct, Shape::Log) => arm!(pol_direct, shape_log),
            (Polarity::Bipolar, Shape::Lin) => arm!(pol_bipolar, shape_lin),
            (Polarity::Bipolar, Shape::Exp) => arm!(pol_bipolar, shape_exp),
            (Polarity::Bipolar, Shape::Log) => arm!(pol_bipolar, shape_log),
            (Polarity::Abs, Shape::Lin) => arm!(pol_abs, shape_lin),
            (Polarity::Abs, Shape::Exp) => arm!(pol_abs, shape_exp),
            (Polarity::Abs, Shape::Log) => arm!(pol_abs, shape_log),
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

/// # What is tested here, and what is not
///
/// The **mechanism** — that a route multiplies, sums, shapes, gates and
/// short-circuits correctly — is tested once for both synths in
/// `vxn_core_matrix::golden`, against a synthetic roster whose gains are all
/// 1.0 and whose taper is the identity
/// ([ADR 0003](../../../../adrs/0003-vxn-core-matrix.md) §5, ticket 0331).
/// Asserting it here meant baking roster constants into an expectation:
/// `out[Cutoff] == 24.0` claimed three things at once — the evaluator
/// multiplies, `DEST_GAIN[Cutoff]` is 48, and `Cutoff` takes no taper — so
/// changing a gain failed a test of the evaluator.
///
/// What stays below is **roster tests** plus the randomised scalar-vs-bank
/// parity sweep. A roster test asserts a fact about *this synth's* table — this
/// gain is 48, these dests take the cubic taper, the default patch drives the
/// amp — and reads the evaluator only as the most direct way to observe it.
/// The sweep stays because the golden table covers the cases someone thought
/// of and the sweep covers the ones they didn't.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::matrix::{MatrixSlot, default_patch};

    /// Slot with the default `Direct` polarity — the common case in tests.
    fn slot(source: SourceId, dest: DestId, depth: f32, shape: Shape) -> MatrixSlot {
        slot_pol(source, dest, depth, Polarity::Direct, shape)
    }

    fn slot_pol(
        source: SourceId,
        dest: DestId,
        depth: f32,
        polarity: Polarity,
        shape: Shape,
    ) -> MatrixSlot {
        MatrixSlot {
            source,
            dest,
            depth,
            polarity,
            shape,
            enabled: true,
            scale_src: SourceId::None,
            scale_shape: Shape::Lin,
        }
    }

    /// The bank evaluator must be the scalar one, lane for lane, **bit-exact**
    /// — not close. It is a transposition, not a reformulation: any drift means
    /// an operation was reassociated, and the two paths would then voice the
    /// same patch differently depending on how many lanes were live.
    #[test]
    fn eval_dests_bank_is_bit_exact_against_the_scalar_form() {
        const L: usize = 8;
        // A deterministic spread of patches: every (polarity, shape) pair, every
        // scale bend, scaled and unscaled routes, several slots sharing a dest
        // (the accumulate order case), and inert slots interleaved so
        // compaction is exercised.
        let mut rng = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        // Every (polarity, shape) pair, so the parity test walks all nine
        // dispatch arms rather than the four the flat curve enum had.
        let curves = [
            (Polarity::Direct, Shape::Lin),
            (Polarity::Direct, Shape::Exp),
            (Polarity::Direct, Shape::Log),
            (Polarity::Bipolar, Shape::Lin),
            (Polarity::Bipolar, Shape::Exp),
            (Polarity::Bipolar, Shape::Log),
            (Polarity::Abs, Shape::Lin),
            (Polarity::Abs, Shape::Exp),
            (Polarity::Abs, Shape::Log),
        ];
        for _ in 0..200 {
            let mut t = MatrixTable::default();
            for slot in t.slots.iter_mut() {
                let pick = next();
                *slot = MatrixSlot {
                    source: SourceId::from_u8((pick % (N_SOURCES as u64 + 1)) as u8),
                    dest: DestId::from_u8(((pick >> 8) % (N_DESTS as u64 + 1)) as u8),
                    depth: ((pick >> 16) % 2001) as f32 / 1000.0 - 1.0,
                    polarity: curves[((pick >> 32) % 9) as usize].0,
                    shape: curves[((pick >> 32) % 9) as usize].1,
                    // Two slots in three are switched on, so the parity test
                    // covers the off case as well — a disabled slot must be
                    // dropped identically by both evaluators.
                    enabled: ((pick >> 48) % 3) != 0,
                    scale_src: SourceId::from_u8(((pick >> 40) % (N_SOURCES as u64 + 1)) as u8),
                    scale_shape: [Shape::Lin, Shape::Exp, Shape::Log]
                        [((pick >> 52) % 3) as usize],
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

    // ── the shared golden-vector table, run through VXN1b's own evaluators ──

    /// VXN1b endpoints standing in for the synthetic roster's four sources,
    /// in its storage-index order: two bipolar, then two unipolar.
    const GOLDEN_SOURCES: [SourceId; 4] = [
        SourceId::Lfo1,
        SourceId::Lfo2,
        SourceId::ModWheel,
        SourceId::Velocity,
    ];

    /// …and for its four destinations. Every one of these has `DEST_GAIN` 1.0
    /// and the identity taper, which is what lets a case's expectation — written
    /// against a roster with no gain and no taper — carry over unchanged. The
    /// assertion below holds them to it, so swapping in a scaled dest fails
    /// here rather than producing a plausible-looking wrong number.
    const GOLDEN_DESTS: [DestId; 4] = [
        DestId::Resonance,
        DestId::Amp,
        DestId::Pan,
        DestId::Env1Sustain,
    ];

    /// The mechanism table from `vxn_core_matrix::golden`, evaluated by
    /// **VXN1b's** evaluators rather than by the harness's reference pair.
    ///
    /// This is what makes the deleted mechanism tests a move rather than a
    /// loss. The shared table's own paths prove the shared arithmetic
    /// self-consistent; nothing there touches `eval_dests`, so without this
    /// bridge a transposed arm in this file's nine-way dispatch — `Abs` wired
    /// to `pol_direct`, say — would be invisible. Both of this synth's
    /// evaluators run every case, so the compaction path is covered too.
    #[test]
    fn the_shared_golden_vectors_hold_for_vxn1b() {
        use vxn_core_matrix::golden::{CASES, NONE, expected_totals};
        use vxn_core_matrix::roster::MatrixRoster;
        use vxn_core_matrix::test_roster::TestRoster;

        for (i, d) in GOLDEN_DESTS.iter().enumerate() {
            assert_eq!(DEST_GAIN[d.index()], 1.0, "{d:?} is not a unit-gain dest");
            assert_eq!(d.cook_depth(0.5), 0.5, "{d:?} does not take the identity taper");
            assert_eq!(
                GOLDEN_SOURCES[i].is_bipolar(),
                TestRoster::source_is_bipolar(i as u8),
                "source {i} stands in for the wrong polarity"
            );
        }

        let endpoint = |i: u8, table: &[SourceId; 4]| {
            if i == NONE { SourceId::None } else { table[i as usize] }
        };
        for case in CASES {
            let mut t = MatrixTable::default();
            for (i, r) in case.routes.iter().enumerate() {
                let (polarity, shape) = vxn_core_matrix::curve::curve_split(r.curve);
                t.slots[i] = MatrixSlot {
                    source: endpoint(r.source, &GOLDEN_SOURCES),
                    dest: if r.dest == NONE {
                        DestId::None
                    } else {
                        GOLDEN_DESTS[r.dest as usize]
                    },
                    depth: r.depth,
                    polarity,
                    shape,
                    enabled: r.enabled,
                    scale_src: endpoint(r.scale_src, &GOLDEN_SOURCES),
                    scale_shape: Shape::from_u8(r.scale_bend),
                };
            }
            let mut sources = [0.0f32; N_SOURCES];
            for &(si, v) in case.sources {
                sources[GOLDEN_SOURCES[si as usize].index()] = v;
            }

            let want: [f32; 4] = expected_totals::<TestRoster, 4>(case);
            let mut got = [0.0; N_DESTS];
            eval_dests(&t, &sources, &mut got);

            const L: usize = 8;
            let mut soa = [[0.0f32; L]; N_DESTS];
            eval_dests_bank(&RouteList::compile(&t), &sources_to_soa(&[sources; L]), &mut soa);
            let banked = dests_for_lane(&soa, L - 1);

            for (d, x) in got.iter().enumerate() {
                // A dest the case does not name must come out exactly zero, and
                // so must every VXN1b dest the mapping never touches.
                let expect = GOLDEN_DESTS
                    .iter()
                    .position(|g| g.index() == d)
                    .map_or(0.0, |g| want[g]);
                assert_eq!(x.to_bits(), expect.to_bits(), "'{}': scalar dest {d}", case.name);
                assert_eq!(
                    banked[d].to_bits(),
                    expect.to_bits(),
                    "'{}': banked dest {d}",
                    case.name
                );
            }
        }
    }

    // ── roster tests: facts about VXN1b's own source and destination tables ──

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
    fn pitch_depth_takes_the_cubic_taper_others_stay_linear() {
        let s = eval_sources(&SourceInputs { lfo1: 1.0, ..Default::default() });
        let mut out = [0.0; N_DESTS];
        // Pitch: 0.5³ · 12 st = 1.5 st — half travel is a musical vibrato/
        // detune range, not 6 st.
        eval_dests(&table(&[slot(SourceId::Lfo1, DestId::Pitch, 0.5, Shape::Lin)]), &s, &mut out);
        assert!((out[DestId::Pitch.index()] - 1.5).abs() < 1e-6);
        // Endpoints and sign survive the taper.
        eval_dests(&table(&[slot(SourceId::Lfo1, DestId::Pitch, 1.0, Shape::Lin)]), &s, &mut out);
        assert!((out[DestId::Pitch.index()] - 12.0).abs() < 1e-6);
        eval_dests(&table(&[slot(SourceId::Lfo1, DestId::Pitch, -1.0, Shape::Lin)]), &s, &mut out);
        assert!((out[DestId::Pitch.index()] + 12.0).abs() < 1e-6);
        eval_dests(&table(&[slot(SourceId::Lfo1, DestId::Pitch, -0.5, Shape::Lin)]), &s, &mut out);
        assert!((out[DestId::Pitch.index()] + 1.5).abs() < 1e-6);
        // Every other dest is untouched: 0.5 × 48 st stays 24 st.
        for d in [DestId::XModSweep, DestId::Cutoff, DestId::HpfCutoff] {
            eval_dests(&table(&[slot(SourceId::Lfo1, d, 0.5, Shape::Lin)]), &s, &mut out);
            let want = 0.5 * DEST_GAIN[d.index()];
            assert!((out[d.index()] - want).abs() < 1e-5, "{d:?} should stay linear");
        }
    }

    /// A switched-off slot keeps its wiring: persistence writes it, and
    /// re-enabling is lossless. **Roster/slot-semantics test** — the
    /// evaluator half of the old test (a disabled slot contributes nothing,
    /// re-enabling restores exactly what it contributed) is a mechanism claim
    /// and lives in `vxn_core_matrix::golden`, which asserts it against every
    /// evaluator path rather than against this one.
    #[test]
    fn disabled_slot_keeps_its_wiring() {
        let mut sl = slot(SourceId::Lfo1, DestId::Cutoff, 0.5, Shape::Lin);
        assert!(sl.is_wired() && sl.is_active());
        sl.enabled = false;
        assert!(sl.is_wired() && !sl.is_active());
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
            eval_dests(&table(&[slot(SourceId::ModWheel, d, 1.0, Shape::Lin)]), &s, &mut out);
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
            &table(&[slot(SourceId::ModWheel, DestId::Lfo1Rate, 1.0, Shape::Lin)]),
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
        let t = table(&[slot(SourceId::Key, DestId::Cutoff, KEY_CUTOFF_UNITY_DEPTH, Shape::Lin)]);
        let one_up = eval_sources(&SourceInputs { note: 72, ..Default::default() }); // +1 oct
        let mut out = [0.0; N_DESTS];
        eval_dests(&t, &one_up, &mut out);
        assert!((out[DestId::Cutoff.index()] - 12.0).abs() < 1e-5,
            "1 octave of key should shift cutoff 12 st");
    }
}
