//! Mod-matrix data model + seeded default patch (ticket 0201).
//!
//! The routing model for VXN1b, adapted from VXN2's matrix
//! ([`vxn-2/crates/vxn2-engine/src/matrix.rs`]) with VXN1's source/destination
//! sets (ADR 0001 §2–§3). This ticket is **types + defaults only** — evaluation
//! (source fan-out, curve/scale application, dest smoothing) is 0202.
//!
//! VXN1b's matrix is **flat**, so — unlike VXN2 — there are no granularity
//! *tiers* and no coherence rules: every source and destination is a per-lane
//! (or patch-global) scalar the evaluator will read once per lane per control
//! block. [`StackWidth`](crate::params::StackWidth) (0266) spends several lanes
//! on one note, but that adds no tier — a stacked note's lanes are ordinary
//! lanes, each evaluating the matrix for itself.
//!
//! A slot is `(source, dest, depth, curve, scale_src)`. `depth` mirrors the
//! CLAP param (0200); the other four fields are **patch topology** (state + TOML,
//! 0203). Slots to the same dest sum (additive) — the evaluator's job.

use vxn_core_matrix::matrix_enum;

use crate::params::MATRIX_SLOTS;

/// The curve-shaping vocabulary, re-exported from
/// [`vxn_core_matrix::curve`] so that `crate::matrix::Polarity` keeps meaning
/// what it always did.
///
/// Both axes, their tables, the flat preset codec and the scale VCA live in the
/// shared crate as of 0330 — VXN1b's copy was a hand-port of VXN2's, added 96
/// minutes after it, and the two had already started drifting. What stays here
/// is the roster: which sources and destinations *this* synth can route.
pub use vxn_core_matrix::curve::{
    CURVE_NAMES, N_CURVES, N_POLARITIES, N_SHAPES, POLARITY_LABELS, POLARITY_NAMES, Polarity,
    SHAPE_LABELS, SHAPE_NAMES, Shape, curve_code, curve_split,
};

/// Slot count — the single source of truth is the param table's slot-depth count
/// ([`crate::params::MATRIX_SLOTS`]), so the topology and the automatable depths
/// can never disagree on how many slots exist.
pub const N_SLOTS: usize = MATRIX_SLOTS;

// ── SourceId ────────────────────────────────────────────────────────────────

matrix_enum! {
    /// Modulation source. `None` is the empty-slot sentinel (index 0); a slot whose
    /// source is `None` is inert and skipped by the evaluator.
    ///
    /// The real sources are VXN1's fixed-route inputs (Env/LFO/Velocity/Key/
    /// wheels), the two VXN1 lacks — `Aftertouch` (MPE per-voice pressure from
    /// 0198) and `NoteRandom` (per-voice latch from 0199) — and the two stack
    /// positions, `Spread` (knob-scaled, 0260) and `StackPos` (raw, 0308).
    SourceId, fallback = None, names = SOURCE_NAMES,
    labels = SOURCE_LABELS, roster_names = ROSTER_SOURCE_NAMES,
    roster_labels = ROSTER_SOURCE_LABELS, polarity;
    sentinel None = 0, "none", "—";
    Env1 = 1, "env1", "Env 1", uni;
    Env2 = 2, "env2", "Env 2", uni;
    Lfo1 = 3, "lfo1", "LFO 1", bi;
    Lfo2 = 4, "lfo2", "LFO 2", bi;
    Velocity = 5, "velocity", "Velocity", uni;
    Key = 6, "key", "Key", uni;
    ModWheel = 7, "mod-wheel", "Mod Wheel", uni;
    PitchWheel = 8, "pitch-wheel", "Pitch Wheel", bi;
    Aftertouch = 9, "aftertouch", "Aftertouch", uni;
    NoteRandom = 10, "note-random", "Note Rnd", uni;
    /// The voice's own place in the stereo image: the lane's fixed
    /// position scaled by the `Spread` param, so a route into [`DestId::Pan`]
    /// at depth 1 reproduces VXN1's hard-wired unison spread exactly. Keeping
    /// the param's scaling *inside* the source is what lets Spread stay a
    /// front-panel knob instead of becoming "slot 3's depth". For the position
    /// *without* that scaling, use [`SourceId::StackPos`].
    Spread = 11, "spread", "Spread", bi;
    /// The voice's raw place in its stack: `stack_spread(i, width)` in
    /// `[-1, 1]`, `0.0` for a width-1 stack — the same allocator position
    /// [`SourceId::Spread`] carries, but **without** the `Spread` param folded
    /// in.
    ///
    /// Why both exist: `Spread`'s in-source scaling is what keeps the pan knob
    /// a knob, but it makes every *other* use of lane position hostage to a pan
    /// control — fanning envelope times across a unison stack shouldn't require
    /// widening the stereo image, and reads as dead at the knob's `0.0` default.
    /// This source is the position on its own, for routes that want the stack's
    /// shape rather than its picture.
    StackPos = 12, "stack-pos", "Stack Pos", bi;
}

/// Count of non-sentinel sources (`None` excluded). Derived from the generated
/// table, so adding a row cannot leave it stale.
pub const N_SOURCES: usize = SOURCE_NAMES.len() - 1;

impl SourceId {
    /// Index into a per-voice source lookup, or `None` for the sentinel.
    #[inline]
    pub const fn idx(self) -> Option<usize> {
        match self {
            SourceId::None => None,
            _ => Some(self as usize - 1),
        }
    }

    /// Index into a per-voice source table, with the sentinel folded to 0.
    ///
    /// The companion to [`SourceId::idx`]: reach for `idx` where the sentinel
    /// is a real case to branch on (an empty slot), and for this where the
    /// caller already knows it holds a real source and only wants to index.
    /// Folding rather than panicking keeps the method `const`, so index
    /// expressions stay compile-time constants.
    #[inline]
    pub const fn index(self) -> usize {
        match self.idx() {
            Some(i) => i,
            None => 0,
        }
    }


}

// ── DestId ──────────────────────────────────────────────────────────────────

matrix_enum! {
    /// Modulation destination. `None` is the empty-slot sentinel. The core v1 dest
    /// set (ADR 0001 §2): the continuous synthesis targets the fixed VXN1 routes
    /// used to reach. `XModSweep` is the wide, mode-aware osc-sweep target (inherits
    /// VXN1 ADR 0004's mode-gated behaviour); `CrossModAmount` modulates the FM/sync
    /// index.
    /// ## Reading a row
    ///
    /// `gain` converts the normalised `[-1, 1]` route product into the dest's
    /// own unit (semitones of pitch or cutoff, a pulse-width fraction, octaves
    /// of envelope time), so a fixed depth is musically comparable across dest
    /// kinds. `taper` is `cubic` only on `Pitch`; `tier` is `per_lane` for every
    /// row, because VXN1b's matrix is flat — the tier column is the degenerate
    /// case of VXN2's model rather than a rival one, and it costs nothing until
    /// this synth grows a patch-global destination.
    ///
    /// `smooth` is the class the smoother bank applies to the dest's summed
    /// total ([`crate::mod_smoothing`]), not an inventory of every motion the
    /// render applies — see the comment on `Amp`.
    ///
    /// The taper is applied at *consumption* ([`crate::eval::eval_dests`]),
    /// never to the stored slot depth: the CLAP param, the preset file and the
    /// state blob all stay linear, so automation and round-trips are unaffected.
    DestId, fallback = None, names = DEST_NAMES,
    labels = DEST_LABELS, roster_names = ROSTER_DEST_NAMES,
    roster_labels = ROSTER_DEST_LABELS, roster_gains = ROSTER_DEST_GAIN;
    sentinel None = 0, "none", "—";
    /// ±12 st (±1 oct) of vibrato, and the only `cubic` row. With a linear
    /// depth the whole vibrato range lives in the bottom sliver of fader travel
    /// — VXN1's default 0.05 st is 0.4% of the ±12 st span, so a single pixel of
    /// movement is a semitone-scale jump and precise vibrato is undialable. `d³`
    /// keeps the sign and the full ±12 st reach while widening the musical low
    /// end: 25% travel ≈ ±0.19 st, 50% ≈ ±1.5 st, 100% ≈ ±12 st.
    Pitch = 1, "pitch", "Pitch", gain = 12.0, taper = cubic,
        tier = per_lane, smooth = quantum_cascade;
    // The wide osc sweep rides the same cascade as `Pitch` (a stepped sweep
    // clicks the same way) but stays **linear**: it is a sweep amount, not a
    // tuning offset, so a depth taper would fight the range it exists to reach.
    XModSweep = 2, "xmod-sweep", "Cross Mod Sweep", gain = 48.0, taper = linear,
        tier = per_lane, smooth = quantum_cascade;
    Pwm = 3, "pwm", "PWM (Both)", gain = 0.5, taper = linear,
        tier = per_lane, smooth = quantum;
    // `Cutoff` / `HpfCutoff` are deliberately **not** tapered: their gain is
    // already log/semitone-shaped, so a depth taper would double-bend the
    // response (same rule as VXN2). Nor are they smoothed here — the OTA ladder
    // ramps its own coefficients per frame (`bank::prepare_ramp` /
    // `tick_coeffs`), which already absorbs their block-edge steps.
    Cutoff = 4, "cutoff", "Cutoff", gain = 48.0, taper = linear,
        tier = per_lane, smooth = block;
    Resonance = 5, "resonance", "Resonance", gain = 1.0, taper = linear,
        tier = per_lane, smooth = block;
    HpfCutoff = 6, "hpf-cutoff", "HPF Cutoff", gain = 48.0, taper = linear,
        tier = per_lane, smooth = block;
    // **`block` is deliberate here, and is the one documented exception in
    // ADR 0003 §3 — do not "fix" it to `per_sample`.** VXN1b's VCA smooths only
    // the *non-envelope* part of its Amp coefficient, per frame; the envelope
    // part has to stay per-frame exact or the attack smears. That factoring is a
    // property of this synth's VCA, not of routing, so the engine is not told
    // about it: the bank splits the coefficient itself
    // (`bank`'s `amp_stat` one-pole) and the roster declares the class the
    // *shared* bank would apply to the whole total, which is none.
    Amp = 7, "amp", "Amp", gain = 1.0, taper = linear,
        tier = per_lane, smooth = block;
    CrossModAmount = 8, "cross-mod-amount", "Cross Mod Amt", gain = 4.0,
        taper = linear, tier = per_lane, smooth = quantum;
    /// Voice position in the stereo image, `[-1, 1]`. Replaces VXN1's
    /// hard-wired `pan_position(lane) × spread`: the default patch routes
    /// [`SourceId::Spread`] here at depth 1, and anything else routed on top
    /// (LFO auto-pan, an envelope throwing a transient left) sums with it.
    Pan = 9, "pan", "Pan", gain = 1.0, taper = linear,
        tier = per_lane, smooth = quantum;
    /// Osc 1's pulse width alone (0261). Sums with [`DestId::Pwm`], which stays
    /// as the both-oscillators route: osc 1's offset is `Pwm + Osc1Pwm`.
    /// Two detuned pulse oscs get their thickness from the widths sweeping
    /// *independently*, which a single shared dest cannot express.
    ///
    /// Same gain as the combined [`DestId::Pwm`] because the two sum per
    /// oscillator: a route moved from one to the other keeps its felt depth.
    Osc1Pwm = 10, "osc1-pwm", "Osc 1 PWM", gain = 0.5, taper = linear,
        tier = per_lane, smooth = quantum;
    /// Osc 2's pulse width alone. Mirror of [`DestId::Osc1Pwm`].
    Osc2Pwm = 11, "osc2-pwm", "Osc 2 PWM", gain = 0.5, taper = linear,
        tier = per_lane, smooth = quantum;
    /// Envelope 1's **A/D/R times**, as a multiplier cooked at note-on (0268):
    /// `2^x` over the dest total clamped to `[-1, 1]`, so the reachable range
    /// is 0.5× (half as long) .. 2.0× (twice as long) with 0 exactly unity.
    /// Sustain is a *level*, not a time, and is deliberately untouched.
    ///
    /// Unlike every other dest this one is **not continuous**: the multiplier
    /// is latched when the voice triggers and held for the life of the note
    /// (see [`crate::eval::env_time_scale`]). Latched, so there is nothing for a
    /// smoother to glide: `smooth = block`.
    ///
    /// The gain is 1.0 because the native unit is *octaves of time* — depth 1
    /// reaches the 2× rail and −1 the 0.5× one, the same musical distance.
    Env1Scale = 12, "env1-scale", "Env 1 Scale", gain = 1.0, taper = linear,
        tier = per_lane, smooth = block;
    /// Envelope 2's A/D/R times. Mirror of [`DestId::Env1Scale`].
    Env2Scale = 13, "env2-scale", "Env 2 Scale", gain = 1.0, taper = linear,
        tier = per_lane, smooth = block;
    /// Per-voice LFO 1's **rate**, as a multiplier on the resolved Hz (0269):
    /// `2^x` over the dest total clamped to `[-2, 2]`, so a full-depth route
    /// spans 0.25× .. 4× the panel rate with 0 exactly unity.
    ///
    /// Multiplicative on the *resolved* rate, so it composes with tempo sync
    /// (0267): a synced LFO stays on the grid under any power-of-two amount,
    /// and lands between subdivisions otherwise — the same freedom the Rate
    /// fader has when sync is off, now per voice.
    ///
    /// Unlike every other dest this one reads the **previous** control block's
    /// total: LFO 1 is itself a source, and the lanes tick before the matrix is
    /// evaluated, so a same-block read would be circular. The lag is one
    /// control block (32 samples, ~0.7 ms at 48 kHz).
    ///
    /// Exponential like the envelope time scales but wanting a wider reach: two
    /// octaves either way turns a 5 Hz wobble into a 1.25 Hz sway or a 20 Hz
    /// buzz, which is the range the wheel/velocity routes are for.
    Lfo1Rate = 14, "lfo1-rate", "LFO 1 Rate", gain = 2.0, taper = linear,
        tier = per_lane, smooth = block;
    /// Envelope 1's **sustain level**, as an offset cooked at note-on (0270):
    /// the dest total is *added* to the patch sustain and clamped to `[0, 1]`,
    /// so depth 1 spans the whole range.
    ///
    /// Additive where the time dests ([`DestId::Env1Scale`]) are multiplicative,
    /// because sustain is an absolute level rather than a duration: a
    /// multiplier can never lift a sustain of 0, and never reach the ceiling
    /// from a low one — which is exactly what a velocity or wheel route wants
    /// to do.
    ///
    /// Latched at note-on like the time scales: the sustain level is the
    /// envelope's *held* value, so tracking it continuously would step a
    /// ringing note (and, through the decay rate it also sets, bend a decay
    /// already in flight). Latched, so `smooth = block`; unity gain, because an
    /// additive `[0, 1]` level wants depth 1 to span the whole range.
    Env1Sustain = 15, "env1-sustain", "Env 1 Sustain", gain = 1.0, taper = linear,
        tier = per_lane, smooth = block;
    /// Envelope 2's sustain level. Mirror of [`DestId::Env1Sustain`].
    Env2Sustain = 16, "env2-sustain", "Env 2 Sustain", gain = 1.0, taper = linear,
        tier = per_lane, smooth = block;
}

/// Count of non-sentinel destinations. Derived, like [`N_SOURCES`].
pub const N_DESTS: usize = DEST_NAMES.len() - 1;

impl DestId {
    /// Index into a per-voice dest accumulator, or `None` for the sentinel.
    #[inline]
    pub const fn idx(self) -> Option<usize> {
        match self {
            DestId::None => None,
            _ => Some(self as usize - 1),
        }
    }

    /// Index into a per-voice dest accumulator, with the sentinel folded to 0.
    /// The companion to [`DestId::idx`] — see [`SourceId::index`] for when to
    /// reach for which.
    #[inline]
    pub const fn index(self) -> usize {
        match self.idx() {
            Some(i) => i,
            None => 0,
        }
    }
}

// ── MatrixSlot / MatrixTable ────────────────────────────────────────────────

/// The endpoint seam: what the shared routing mechanism needs to know about a
/// [`SourceId`] — which row it names, and which way it swings.
///
/// Both methods forward to the generated inherent ones, which keep name
/// resolution: `source.idx()` at a VXN1b call site still reaches the inherent
/// `idx`, trait in scope or not.
impl vxn_core_matrix::slot::SourceEndpoint for SourceId {
    #[inline]
    fn idx(self) -> Option<usize> {
        SourceId::idx(self)
    }

    #[inline]
    fn is_bipolar(self) -> bool {
        SourceId::is_bipolar(self)
    }
}

/// The endpoint seam for a [`DestId`]: its row, its native-unit gain and its
/// depth taper — the two numeric columns
/// [`RouteList::compile`](vxn_core_matrix::slot::RouteList::compile) folds into
/// a route's single gain factor.
impl vxn_core_matrix::slot::DestEndpoint for DestId {
    #[inline]
    fn idx(self) -> Option<usize> {
        DestId::idx(self)
    }

    #[inline]
    fn gain(self) -> f32 {
        DestId::gain(self)
    }

    #[inline]
    fn cook_depth(self, depth: f32) -> f32 {
        DestId::cook_depth(self, depth)
    }
}

/// One matrix route. `depth` mirrors the slot's CLAP param (0200, bipolar
/// `[-1, 1]`) and stays **raw** — the taper is applied at compile time, not
/// stored; `source`/`dest`/`curve`/`scale_src`/`enabled` are patch topology.
///
/// The type itself is [`vxn_core_matrix::slot::MatrixSlot`] as of 0333: VXN2's
/// slot was the same eight fields under a different `enabled` convention, and
/// the on/off vs wired distinction below is the part that kept being
/// re-derived. What stays here is the roster the two type parameters name.
pub type MatrixSlot = vxn_core_matrix::slot::MatrixSlot<SourceId, DestId>;

/// The 16-slot patch topology + depths, shared with VXN2 (0333). Slot order is
/// the load-bearing part: dests accumulate additively and float addition is not
/// associative, so "the same routes in the same order" is what keeps the scalar
/// and banked evaluators bit-exact against each other.
pub type MatrixTable = vxn_core_matrix::slot::MatrixTable<SourceId, DestId, N_SLOTS>;

/// VXN1b-only behaviour on the shared [`MatrixTable`].
///
/// An extension trait rather than an inherent `impl` because the type is now
/// defined in `vxn-core-matrix`, and an inherent method may only be written in
/// the crate that owns the type. Nothing about pan seeding belongs in the shared
/// mechanism: it exists because *this* synth used to hard-wire unison spread.
pub trait MatrixTableExt {
    /// Install the default `Spread → Pan` route if this table has **no** route
    /// into [`DestId::Pan`] at all, using the first free slot. Returns whether
    /// one was installed.
    fn ensure_pan_route(&mut self) -> bool;
}

impl MatrixTableExt for MatrixTable {
    /// Why loading needs this: before pan was a destination, spread was
    /// hard-wired DSP, so every patch written until now carries no pan route
    /// and would load dead-centre — a silent regression on every existing
    /// preset. Seeding on load fixes that without a format change, since the
    /// preset text is name-keyed and sparse rather than positional.
    ///
    /// A patch that *does* route `Pan` — even from some other source, even at
    /// depth 0, even switched off — is left alone: it has an opinion about pan,
    /// and overriding it would be worse than the problem being solved. A table
    /// with all 16 slots **wired** is likewise left alone rather than evicting
    /// the player's work.
    fn ensure_pan_route(&mut self) -> bool {
        if self.slots.iter().any(|s| s.dest == DestId::Pan) {
            return false;
        }
        // A *free* slot means an unwired one — `is_active` would also match a
        // route the player set up and switched off, and seeding over that would
        // destroy their work to solve a problem they don't have.
        match self.slots.iter_mut().find(|s| !s.is_wired()) {
            Some(slot) => {
                *slot = SPREAD_TO_PAN;
                true
            }
            None => false,
        }
    }
}

/// Both layers' topology as a **view payload** (0247): the engine→page echo that
/// keeps the mod-matrix combos honest when the patch changes underneath an open
/// editor (preset load, host state load, undo).
///
/// Topology is not a CLAP param, so the host replays nothing for it; the page is
/// seeded once at editor-open ([`crate::PluginState`] → the faceplate splice,
/// 0246) and thereafter learns of changes only through this. Depths are
/// deliberately absent — they *are* params and ride `ParamChanged`, and echoing
/// them here would give the page two sources of truth for one value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MatrixSnapshot {
    pub layers: [MatrixTable; 2],
}

/// Key→Cutoff depth that reproduces **exactly 1 octave of cutoff per octave of
/// key** (ADR 0001 §3), under the convention **locked by the 0202 evaluator**
/// ([`crate::eval`]):
///
/// - the **Key** source emits signed **octaves relative to C4** (`(note−60)/12`);
/// - the **Cutoff** destination has native-unit gain **48 semitones**
///   ([`crate::eval::DEST_GAIN`]).
///
/// Key→Cutoff at depth `d` shifts cutoff by `octaves_from_c4 · d · 48`
/// semitones; over one octave of key (`octaves_from_c4` changes by 1) that is
/// `d · 48` semitones. For 12 st (1 oct of cutoff) → `d = 12/48 = 0.25`.
///
/// **This is the *extra*, freely-routed tracking, not the VXN1 control** (0245).
/// VXN1's key-track is [`ParamId::FilterKeyTrack`](crate::params::ParamId), a
/// dedicated `0..1` param pivoting at **C0** — matching pivot as well as slope,
/// which is what makes "cutoff at minimum (16.3516 Hz = C0) + track at 1.0 ⇒
/// cutoff *is* the played note" hold. A matrix route pivots at C4 like every
/// other Key route, and stacks additively on top of the param: use it for the
/// things the param cannot do (envelope-scaled, negative, or curved tracking,
/// or more than 1 oct/oct).
pub const KEY_CUTOFF_UNITY_DEPTH: f32 = 0.25;

/// Matrix depth reproducing VXN1's default LFO1→pitch **vibrato** of 0.05 st
/// (`pitch_lfo_depth` default). Pitch takes the cubic depth taper
/// ([`DestId::cook_depth`]) before the dest's native gain
/// ([`crate::eval::DEST_GAIN`]`[Pitch]` = 12 st), so the depth is the *cube
/// root* of the semitone value over that gain: `∛(0.05/12) ≈ 0.1609`, giving
/// `source·depth³·gain` = 0.05 st at full LFO swing. Cross-checked against both
/// in `eval::tests::default_vibrato_is_the_vxn1_005_st`.
pub const DEFAULT_VIBRATO_DEPTH: f32 = 0.160_918;

/// The factory default patch: seeds the routes that reproduce **VXN1's default
/// sound**, leaving every other slot inert. The rest of the modulation surface
/// is the player's to fill.
///
/// - **Slot 0 — Env2 → Amp @ 1.0.** Reproduces VXN1's hardwired VCA = Env2: the
///   amp envelope drives the VCA at full depth (VXN1 ADR 0004's Amp column).
///   Essential — without it the VCA never opens.
/// - **Slot 1 — LFO1 → Pitch @ [`DEFAULT_VIBRATO_DEPTH`].** Reproduces VXN1's
///   gentle default vibrato (0.05 st), so VXN1b's factory patch matches VXN1's
///   real default — the render-parity target.
///
/// - **Slot 2 — Spread → Pan @ 1.0.** Reproduces VXN1's hard-wired unison
///   spread as an ordinary route. Depth 1.0 is the identity, so the
///   `Spread` knob keeps its full range and meaning; delete the route and the
///   knob goes inert, which is the honest consequence of routing being visible.
///
/// Filter key-track is **not** here — it is its own param
/// ([`ParamId::FilterKeyTrack`](crate::params::ParamId), default `0.0` like
/// VXN1's), so the slot stays the player's.
pub fn default_patch() -> MatrixTable {
    let mut table = MatrixTable::default();
    table.slots[0] = MatrixSlot {
        source: SourceId::Env2,
        dest: DestId::Amp,
        depth: 1.0,
        polarity: Polarity::Direct,
        shape: Shape::Lin,
        enabled: true,
        scale_shape: Shape::Lin,
        scale_src: SourceId::None,
    };
    table.slots[1] = MatrixSlot {
        source: SourceId::Lfo1,
        dest: DestId::Pitch,
        depth: DEFAULT_VIBRATO_DEPTH,
        polarity: Polarity::Direct,
        shape: Shape::Lin,
        enabled: true,
        scale_shape: Shape::Lin,
        scale_src: SourceId::None,
    };
    table.slots[2] = SPREAD_TO_PAN;
    table
}

/// The `Spread → Pan` route the default patch seeds and
/// [`MatrixTable::ensure_pan_route`] restores. Depth 1.0 is the
/// identity: the `Spread` param does the scaling inside the source.
pub const SPREAD_TO_PAN: MatrixSlot = MatrixSlot {
    source: SourceId::Spread,
    dest: DestId::Pan,
    depth: 1.0,
    polarity: Polarity::Direct,
    shape: Shape::Lin,
    enabled: true,
    scale_shape: Shape::Lin,
    scale_src: SourceId::None,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_count_tracks_param_table() {
        assert_eq!(N_SLOTS, MATRIX_SLOTS);
    }

    #[test]
    fn source_u8_roundtrips_and_degrades() {
        for v in 0..=(N_SOURCES as u8) {
            let s = SourceId::from_u8(v);
            assert_eq!(s as u8, v);
        }
        // Out-of-range decodes to the inert sentinel.
        assert_eq!(SourceId::from_u8(200), SourceId::None);
    }

    #[test]
    fn dest_u8_roundtrips_and_degrades() {
        for v in 0..=(N_DESTS as u8) {
            assert_eq!(DestId::from_u8(v) as u8, v);
        }
        assert_eq!(DestId::from_u8(200), DestId::None);
    }

    #[test]
    fn cook_depth_tapers_pitch_only() {
        // Cubic on Pitch: sign and endpoints kept, low end widened.
        assert_eq!(DestId::Pitch.cook_depth(0.0), 0.0);
        assert_eq!(DestId::Pitch.cook_depth(1.0), 1.0);
        assert_eq!(DestId::Pitch.cook_depth(-1.0), -1.0);
        assert!((DestId::Pitch.cook_depth(0.5) - 0.125).abs() < 1e-6);
        assert!((DestId::Pitch.cook_depth(-0.25) + 0.015_625).abs() < 1e-6);
        // Every other dest passes through untouched.
        for d in [
            DestId::None,
            DestId::XModSweep,
            DestId::Pwm,
            DestId::Cutoff,
            DestId::Resonance,
            DestId::HpfCutoff,
            DestId::Amp,
            DestId::CrossModAmount,
            DestId::Pan,
            DestId::Osc1Pwm,
            DestId::Osc2Pwm,
        ] {
            assert_eq!(d.cook_depth(0.5), 0.5, "{d:?} should stay linear");
            assert_eq!(d.cook_depth(-0.3), -0.3, "{d:?} should stay linear");
        }
    }

    #[test]
    fn polarity_and_shape_u8_roundtrip_and_degrade() {
        for v in 0..(N_POLARITIES as u8) {
            assert_eq!(Polarity::from_u8(v) as u8, v);
        }
        for v in 0..(N_SHAPES as u8) {
            assert_eq!(Shape::from_u8(v) as u8, v);
        }
        assert_eq!(Polarity::from_u8(200), Polarity::Direct);
        assert_eq!(Shape::from_u8(200), Shape::Lin);
    }

    /// The flat code is what **preset files** carry, so the four spellings that
    /// predate the axis split must still land on their original meanings —
    /// codes 0..=3 are load-bearing.
    #[test]
    fn curve_code_preserves_pre_split_preset_encoding() {
        let legacy = [
            (0u8, Polarity::Direct, Shape::Lin, "lin"),
            (1, Polarity::Direct, Shape::Exp, "exp"),
            (2, Polarity::Direct, Shape::Log, "log"),
            (3, Polarity::Bipolar, Shape::Lin, "bipolar"),
        ];
        for (code, pol, shape, name) in legacy {
            assert_eq!(curve_code(pol, shape), code, "{name} code moved");
            assert_eq!(curve_split(code), (pol, shape), "{name} decode moved");
            assert_eq!(CURVE_NAMES[code as usize], name);
        }
    }

    /// Every pair round-trips through the flat code with no collisions, and
    /// anything past the roster degrades rather than aliasing onto a real curve.
    #[test]
    fn curve_code_round_trips_every_pair() {
        let mut seen = std::collections::HashSet::new();
        for p in Polarity::ALL {
            for sh in Shape::ALL {
                let code = curve_code(p, sh);
                assert!((code as usize) < N_CURVES, "{p:?}/{sh:?} out of range");
                assert!(seen.insert(code), "{p:?}/{sh:?} collided on {code}");
                assert_eq!(curve_split(code), (p, sh));
            }
        }
        assert_eq!(seen.len(), N_CURVES);
        assert_eq!(curve_split(N_CURVES as u8), (Polarity::Direct, Shape::Lin));
        assert_eq!(curve_split(255), (Polarity::Direct, Shape::Lin));
    }

    #[test]
    fn idx_maps_reals_and_skips_sentinel() {
        assert_eq!(SourceId::None.idx(), None);
        assert_eq!(SourceId::Env1.idx(), Some(0));
        assert_eq!(SourceId::StackPos.idx(), Some(N_SOURCES - 1));
        assert_eq!(DestId::None.idx(), None);
        assert_eq!(DestId::Pitch.idx(), Some(0));
        assert_eq!(DestId::Pan.idx(), Some(8));
        assert_eq!(DestId::Osc2Pwm.idx(), Some(10));
        assert_eq!(DestId::Env2Scale.idx(), Some(12));
        assert_eq!(DestId::Lfo1Rate.idx(), Some(13));
        assert_eq!(DestId::Env2Sustain.idx(), Some(N_DESTS - 1));
    }

    #[test]
    fn name_and_label_tables_are_sized() {
        assert_eq!(SOURCE_NAMES.len(), N_SOURCES + 1);
        assert_eq!(SOURCE_LABELS.len(), N_SOURCES + 1);
        assert_eq!(DEST_NAMES.len(), N_DESTS + 1);
        assert_eq!(DEST_LABELS.len(), N_DESTS + 1);
        assert_eq!(POLARITY_NAMES.len(), N_POLARITIES);
        assert_eq!(POLARITY_LABELS.len(), N_POLARITIES);
        assert_eq!(SHAPE_NAMES.len(), N_SHAPES);
        assert_eq!(SHAPE_LABELS.len(), N_SHAPES);
        // The flat preset table spans the whole product of the two axes.
        assert_eq!(CURVE_NAMES.len(), N_CURVES);
        assert_eq!(N_CURVES, N_POLARITIES * N_SHAPES);
    }

    /// `ALL` is the bridge between a variant and its row in the two string
    /// tables, so it has to be dense and in discriminant order: `ALL[i] as u8`
    /// must be `i`, or every name and label after a gap is off by one.
    #[test]
    fn variant_order_matches_the_tables() {
        macro_rules! check {
            ($ty:ident, $names:ident, $labels:ident) => {
                for (i, v) in $ty::ALL.iter().enumerate() {
                    assert_eq!(*v as usize, i, "{} is not at index {i}", stringify!($ty));
                    assert_eq!($ty::from_u8(i as u8), *v, "{}::from_u8({i})", stringify!($ty));
                }
                assert_eq!($ty::ALL.len(), $names.len());
                assert_eq!($ty::ALL.len(), $labels.len());
            };
        }
        check!(SourceId, SOURCE_NAMES, SOURCE_LABELS);
        check!(DestId, DEST_NAMES, DEST_LABELS);
        check!(Polarity, POLARITY_NAMES, POLARITY_LABELS);
        check!(Shape, SHAPE_NAMES, SHAPE_LABELS);
    }

    /// Name N must *describe* variant N — the property the old length-only
    /// check could not see, and the one a user notices first because the wrong
    /// word is sitting in the mod-matrix combo.
    ///
    /// Generation makes a transposition unrepresentable, so this is here to
    /// catch the other half: a reordered or mis-transcribed row. Deliberately
    /// spot-checks the pairs most likely to be swapped — adjacent rows, the
    /// mirrored Env1/Env2 pairs, and the two spread sources that differ by one
    /// word — rather than restating the whole table, which would just be the
    /// parallel list again.
    #[test]
    fn names_and_labels_describe_their_own_variant() {
        let src = |s: SourceId| (SOURCE_NAMES[s as usize], SOURCE_LABELS[s as usize]);
        assert_eq!(src(SourceId::None), ("none", "—"));
        assert_eq!(src(SourceId::Lfo1), ("lfo1", "LFO 1"));
        assert_eq!(src(SourceId::Lfo2), ("lfo2", "LFO 2"));
        assert_eq!(src(SourceId::ModWheel), ("mod-wheel", "Mod Wheel"));
        assert_eq!(src(SourceId::PitchWheel), ("pitch-wheel", "Pitch Wheel"));
        assert_eq!(src(SourceId::Spread), ("spread", "Spread"));
        assert_eq!(src(SourceId::StackPos), ("stack-pos", "Stack Pos"));

        let dst = |d: DestId| (DEST_NAMES[d as usize], DEST_LABELS[d as usize]);
        assert_eq!(dst(DestId::None), ("none", "—"));
        // The two renames whose wire name deliberately did NOT follow the label.
        assert_eq!(dst(DestId::XModSweep), ("xmod-sweep", "Cross Mod Sweep"));
        assert_eq!(dst(DestId::Pwm), ("pwm", "PWM (Both)"));
        assert_eq!(dst(DestId::Osc1Pwm), ("osc1-pwm", "Osc 1 PWM"));
        assert_eq!(dst(DestId::Osc2Pwm), ("osc2-pwm", "Osc 2 PWM"));
        assert_eq!(dst(DestId::Env1Scale), ("env1-scale", "Env 1 Scale"));
        assert_eq!(dst(DestId::Env2Scale), ("env2-scale", "Env 2 Scale"));
        assert_eq!(dst(DestId::Env1Sustain), ("env1-sustain", "Env 1 Sustain"));
        assert_eq!(dst(DestId::Env2Sustain), ("env2-sustain", "Env 2 Sustain"));
        assert_eq!(dst(DestId::Lfo1Rate), ("lfo1-rate", "LFO 1 Rate"));

        let pol = |p: Polarity| (POLARITY_NAMES[p as usize], POLARITY_LABELS[p as usize]);
        assert_eq!(pol(Polarity::Direct), ("direct", "Direct"));
        assert_eq!(pol(Polarity::Bipolar), ("bipolar", "Bipolar"));
        assert_eq!(pol(Polarity::Abs), ("abs", "Abs"));

        let shp = |s: Shape| (SHAPE_NAMES[s as usize], SHAPE_LABELS[s as usize]);
        assert_eq!(shp(Shape::Lin), ("lin", "Lin"));
        assert_eq!(shp(Shape::Exp), ("exp", "Exp"));
        assert_eq!(shp(Shape::Log), ("log", "Log"));
    }

    // ── ensure_pan_route ─────────────────────────────────────────────

    #[test]
    fn ensure_pan_route_seeds_a_patch_with_no_pan_opinion() {
        let mut t = MatrixTable::default();
        t.slots[0] = MatrixSlot {
            source: SourceId::Env2,
            dest: DestId::Amp,
            depth: 1.0,
            polarity: Polarity::Direct,
            shape: Shape::Lin,
            enabled: true,
            scale_shape: Shape::Lin,
            scale_src: SourceId::None,
        };
        assert!(t.ensure_pan_route(), "a pan-less patch must be seeded");
        assert_eq!(t.slots[1], SPREAD_TO_PAN, "seeded into the first free slot");
        // Idempotent: a second pass finds the route it just installed.
        assert!(!t.ensure_pan_route());
    }

    #[test]
    fn ensure_pan_route_leaves_an_existing_pan_opinion_alone() {
        // Any route into Pan counts — including one from another source, and
        // including one parked at depth 0. The patch has an opinion; honour it.
        for existing in [
            MatrixSlot {
                source: SourceId::Lfo1,
                dest: DestId::Pan,
                depth: 0.5,
                ..MatrixSlot::default()
            },
            MatrixSlot {
                source: SourceId::Spread,
                dest: DestId::Pan,
                depth: 0.0,
                ..MatrixSlot::default()
            },
        ] {
            let mut t = MatrixTable::default();
            t.slots[4] = existing;
            assert!(!t.ensure_pan_route(), "must not seed over {existing:?}");
            assert_eq!(t.slots[4], existing);
            assert!(
                t.slots.iter().filter(|s| s.dest == DestId::Pan).count() == 1,
                "exactly one pan route"
            );
        }
    }

    /// A switched-off route is still the player's work: the pan seed must skip
    /// it and take a genuinely blank slot instead. Using `is_active` to find a
    /// free slot would silently overwrite a route someone had parked.
    #[test]
    fn ensure_pan_route_does_not_evict_a_switched_off_route() {
        let mut t = MatrixTable::default();
        let parked = MatrixSlot {
            source: SourceId::Lfo2,
            dest: DestId::Cutoff,
            depth: 0.5,
            polarity: Polarity::Direct,
            shape: Shape::Lin,
            enabled: false,
            scale_src: SourceId::None,
            scale_shape: Shape::Lin,
        };
        t.slots[0] = parked;
        assert!(t.ensure_pan_route(), "there are blank slots left to seed into");
        assert_eq!(t.slots[0], parked, "the parked route must survive untouched");
        assert!(
            t.slots.iter().any(|s| s.dest == DestId::Pan),
            "and the seed still landed somewhere"
        );
    }

    #[test]
    fn ensure_pan_route_gives_up_rather_than_evicting() {
        // Sixteen live routes, none of them pan: the player's work wins over
        // the convenience seed.
        let mut t = MatrixTable::default();
        for slot in t.slots.iter_mut() {
            *slot = MatrixSlot {
                source: SourceId::Lfo1,
                dest: DestId::Cutoff,
                depth: 0.5,
                polarity: Polarity::Direct,
                shape: Shape::Lin,
                enabled: true,
                scale_shape: Shape::Lin,
                scale_src: SourceId::None,
            };
        }
        assert!(!t.ensure_pan_route(), "a full table must not be evicted");
        assert!(t.slots.iter().all(|s| s.dest == DestId::Cutoff));
    }

    #[test]
    fn default_slot_is_inert() {
        let s = MatrixSlot::default();
        assert!(!s.is_active());
        assert_eq!(s.source, SourceId::None);
        assert_eq!(s.dest, DestId::None);
        // A None-endpoint slot is inert even if a stale depth rides along.
        let with_depth = MatrixSlot { depth: 0.9, ..MatrixSlot::default() };
        assert!(!with_depth.is_active());
    }

    #[test]
    fn default_patch_seeds_amp_vibrato_and_spread_pan() {
        let t = default_patch();
        // Slot 0: Env2 → Amp @ 1.0 — essential, drives the VCA.
        assert_eq!(
            t.slots[0],
            MatrixSlot {
                source: SourceId::Env2,
                dest: DestId::Amp,
                depth: 1.0,
                polarity: Polarity::Direct,
                shape: Shape::Lin,
                enabled: true,
                scale_shape: Shape::Lin,
                scale_src: SourceId::None,
            }
        );
        // Slot 1: LFO1 → Pitch gentle vibrato (matches VXN1's real default).
        assert_eq!(
            t.slots[1],
            MatrixSlot {
                source: SourceId::Lfo1,
                dest: DestId::Pitch,
                depth: DEFAULT_VIBRATO_DEPTH,
                polarity: Polarity::Direct,
                shape: Shape::Lin,
                enabled: true,
                scale_shape: Shape::Lin,
                scale_src: SourceId::None,
            }
        );
        // Slot 2: Spread → Pan at unity — VXN1's unison spread, as a route
        // rather than hard-wired DSP.
        assert_eq!(t.slots[2], SPREAD_TO_PAN);
        assert_eq!(t.slots[2].depth, 1.0);
        // Key-track is a param, not a pre-wired slot: nothing in the
        // factory patch touches Cutoff, and every remaining slot is the
        // player's.
        for s in &t.slots[3..] {
            assert!(!s.is_active(), "slot past the seeds must be inert");
        }
        assert!(
            !t.slots.iter().any(|s| s.dest == DestId::Cutoff),
            "the factory patch must not pre-wire a Cutoff route"
        );
        // The calibration constant survives as the *extra* tracking's unity mark.
        assert_eq!(KEY_CUTOFF_UNITY_DEPTH, 0.25);
    }
}
