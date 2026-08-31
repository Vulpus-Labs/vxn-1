//! The **roster** half of the seam: what a synth can route.
//!
//! A roster is a zero-sized type carrying, as a compile-time table, everything
//! the mechanism needs to know about a synth's sources and destinations —
//! their counts, each dest's native gain and depth taper, each endpoint's
//! granularity tier, each dest's smoothing class, and the name/label tables.
//! Nothing here knows what a destination *means*; applying a dest total to a
//! filter coefficient, a phase increment or a VCA stays in the synth
//! ([ADR 0003](../../../../adrs/0003-vxn-core-matrix.md) §"Consequences").
//!
//! Ticket [0332](../../../../tickets/open/0332-roster-row-declares-everything.md)
//! generates the implementations from one row list per enum, so that a
//! destination cannot be added without every column being filled. Until then
//! an implementation is written by hand; the trait shape is the same either
//! way.

/// Granularity tier of a source or destination — how many independent values
/// it carries per patch. Coarse → fine, and **the discriminant order is the
/// coarseness order**: [`Tier::covers`] and every ordering comparison depend
/// on it, so a new variant must be inserted at its place in the order, not
/// appended.
///
/// - `PatchGlobal` — one value per patch. vxn-2's `lfo1`, `delay-mix`,
///   `reverb-mix`.
/// - `PerStack` — one value per played voice, broadcast across that voice's
///   unison lanes. vxn-2's `velocity`, `cutoff`, `resonance`.
/// - `PerLane` — one value per unison lane. vxn-2's `lfo2`, `op1-pitch`; and
///   **every** vxn-1b endpoint, whose matrix is flat.
///
/// vxn-1b is the degenerate case of this model rather than a rival one: all
/// endpoints `PerLane` makes every coherence verdict `Ok`, and the machinery
/// costs it nothing until it grows a global destination. That is why the tier
/// vocabulary is shared even though only one synth uses more than one variant
/// today.
///
/// The routing rule this exists for: a routing is **coherent** iff the source
/// tier is coarser-or-equal to the dest tier. A coarser source broadcasts
/// unambiguously to a finer dest; a finer source into a coarser dest is a lossy
/// collapse to lane 0 — which lane wins? [`Tier::covers`] is that rule;
/// [0336](../../../../tickets/open/0336-coherence-in-the-shared-engine.md)
/// adds the two special cases (an LFO into its own rate, and a lane-0-collapsed
/// `voice-idx` route) and the verdict enum the UI reads.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(u8)]
pub enum Tier {
    /// One value per patch.
    PatchGlobal = 0,
    /// One value per played voice, broadcast across its unison lanes.
    PerStack = 1,
    /// One value per unison lane.
    PerLane = 2,
}

impl Tier {
    /// Whether a source at `self` reaches a destination at `dest` without a
    /// lossy collapse — i.e. whether `self` is coarser-or-equal.
    ///
    /// This is the tier half of the coherence rule and nothing more: the
    /// special cases live with the verdict enum in 0336.
    #[inline]
    pub const fn covers(self, dest: Tier) -> bool {
        (self as u8) <= (dest as u8)
    }
}

/// How a destination's summed total is moved from one control block's value to
/// the next.
///
/// Smoothing is **post-sum, per-destination, and declared here** rather than
/// per-route or uniform ([ADR 0003](../../../../adrs/0003-vxn-core-matrix.md)
/// §3). Post-sum because the smoothers are linear, so filtering each route and
/// then summing is arithmetically identical to summing and then filtering, at
/// N× the cost and N× the state for N slots sharing a dest. Per-destination
/// because the right time constant is a property of how click-prone the target
/// is, not of the source driving it: `delay-mix` never clicks, while pitch
/// stairsteps audibly at every control-block edge (~1.5 kHz at 48 kHz).
///
/// The four classes cover every smoother both synths run today:
///
/// | Class | Filter | Ticked | Used today for |
/// |---|---|---|---|
/// | `Block` | none — held for the control block | — | the default; most dests |
/// | `Quantum` | one-pole | per render quantum | vxn-1b PWM, cross-mod, Pan |
/// | `QuantumCascade` | two cascaded one-poles | per render quantum | pitch (both synths), vxn-1b `XModSweep` |
/// | `PerSample` | one-pole | per frame | **nothing** — see the Amp exception below |
///
/// They do **not** cover every *motion*. vxn-2 applies several kinds of
/// engine-side movement after the matrix that are not smoothers in this sense —
/// the op level/pan/phase per-sample linear ramps, the block-rate one-pole on
/// `StackDetune`/`StackSpread`, and the nine EG-rate dests that are consumed
/// once at note-on. All of those declare `Block` here and keep their motion in
/// the synth's target application; migrating them into the shared bank would be
/// a behaviour change and is out of E049's scope. vxn-1b's Amp is the same
/// deliberate escape hatch: it declares `Block` and smooths only the
/// non-envelope part of its VCA coefficient itself, because smoothing the
/// envelope part would smear the attack — which is why, as of 0332, no row in
/// either roster declares `PerSample`. The class stays in the vocabulary
/// because the *filter* it names is one vxn-1b runs; what no roster asks for is
/// that filter over a destination's whole total.
///
/// Consumed by
/// [0335](../../../../tickets/open/0335-declared-target-smoothing.md), which
/// builds the bank. Declared here from the start so that 0332's roster row has
/// a column to fill.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[repr(u8)]
pub enum Smoothing {
    /// No filter — the block accumulator's value is held for the whole control
    /// block. The default, and correct for any destination that cannot click.
    #[default]
    Block = 0,
    /// One-pole, ticked once per render quantum (a sub-block subdivision of the
    /// control block).
    Quantum = 1,
    /// Two cascaded one-poles, ticked once per render quantum.
    ///
    /// The cascade is load-bearing and must not be "simplified" to one pole: a
    /// single pole is C0 but C1-broken — at a saw or pulse LFO step the output
    /// *value* is continuous while its *velocity* jumps 0→max, and that
    /// velocity step is the click. Both synths independently arrived at two
    /// poles for pitch.
    QuantumCascade = 2,
    /// One-pole, ticked every frame. The most expensive class; reserved for
    /// destinations where even a sub-block stairstep is audible.
    PerSample = 3,
}

/// What a synth can route: the compile-time table the shared mechanism reads.
///
/// # Indices are opaque `u8` storage indices
///
/// Every method here takes a bare `u8` rather than an associated enum type.
/// This is deliberate. The engine never needs to know what `dest 7` *is*, and
/// associated types would force every shared function to carry two more generic
/// parameters for no gain. Each synth's `SourceId` / `DestId` stay its own and
/// convert at the boundary, exactly as they already do at the wire boundary.
///
/// The index space is the **storage index**, `0..N_SOURCES` / `0..N_DESTS` —
/// the row a value occupies in the accumulator, which is what a compiled route
/// carries. Both synths spell the "empty slot" sentinel as discriminant 0 of
/// their own enum and already have an `idx()` that drops it; the sentinel is a
/// wire-format concern and does not cross this seam. So a roster's name and
/// label tables are `N_SOURCES` / `N_DESTS` long here, one shorter than the
/// `[&str; N + 1]` tables the synths keep for decoding.
///
/// Out-of-range indices are a caller bug: an implementation may panic, and a
/// generated one will.
///
/// # Contract
///
/// - `source_names().len() == N_SOURCES`, and likewise for labels and dests.
///   Index `i` in every table describes the same endpoint as storage index `i`.
/// - `dest_gain` and `cook_depth` are pure and patch-independent — the
///   evaluator hoists both out of its lane loops and may call them once per
///   block, or once at table-rebuild time.
/// - `cook_depth` is applied to the raw depth **before** `dest_gain`, and must
///   be sign-preserving and monotone (both synths' only non-identity taper is
///   the cubic on semitone dests).
///
/// # The one hazard worth naming here
///
/// The mechanism cooks, so a synth must hand it **raw** depth. vxn-1b already
/// does; vxn-2 does not — it cooks at table-rebuild time and stores the cooked
/// value in the slot, so a shared route compiler fed vxn-2's slots would cook
/// twice and quietly cube an already-cubed depth (0.5 → 0.125 → ~0.00195, a
/// ~64× loss of pitch modulation on a `GlobalPitch` route). That is silent,
/// plausible-looking, and well past E049's −100 dBFS bar. Untangling it is
/// [0333](../../../../tickets/open/0333-share-slot-and-route-compilation.md)'s
/// job; the contract is stated here because this is what an implementor reads.
///
/// # Storage sizing
///
/// `N_SOURCES` / `N_DESTS` describe the roster; they do **not** size any buffer
/// declared in this crate. Callers pass their own exactly-sized arrays and the
/// widths ride as separate const generic parameters — see
/// [`crate::storage`] for why, and for the guard that keeps the two from
/// silently disagreeing.
pub trait MatrixRoster: Copy {
    /// Number of routable sources, sentinel excluded.
    const N_SOURCES: usize;
    /// Number of routable destinations, sentinel excluded.
    const N_DESTS: usize;
    /// Slots in one matrix table. 16 in both synths today.
    const N_SLOTS: usize;

    /// Whether source `src` emits a bipolar `[-1, 1]` shape rather than a
    /// unipolar `[0, 1]` one.
    ///
    /// Consumed by the scale VCA, which folds a bipolar scale source into
    /// `[0, 1]` before bending it. A unipolar source passes through: folding a
    /// `[0, 1)` random into `[0.5, 1)` would mean it could never gate to zero.
    fn source_is_bipolar(src: u8) -> bool;

    /// Native-unit gain for destination `dest` — the factor that turns a
    /// normalised `[-1, 1]` route product into the destination's own unit
    /// (semitones, octaves, a fraction), so that a depth of 1.0 means something
    /// musically comparable across dest kinds.
    fn dest_gain(dest: u8) -> f32;

    /// Taper applied to a slot's raw `depth` for destination `dest`, before
    /// [`dest_gain`](MatrixRoster::dest_gain).
    ///
    /// Identity for most destinations. Semitone destinations take a cubic, so
    /// that the low end of the depth range has usable vibrato resolution.
    fn cook_depth(dest: u8, depth: f32) -> f32;

    /// Granularity tier of destination `dest`.
    fn dest_tier(dest: u8) -> Tier;

    /// Granularity tier of source `src`.
    fn source_tier(src: u8) -> Tier;

    /// Smoothing class for destination `dest`'s summed total.
    fn dest_smoothing(dest: u8) -> Smoothing;

    /// Machine ids (kebab-case wire names), indexed by source storage index.
    fn source_names() -> &'static [&'static str];

    /// Machine ids (kebab-case wire names), indexed by dest storage index.
    fn dest_names() -> &'static [&'static str];

    /// Display labels, indexed by source storage index.
    fn source_labels() -> &'static [&'static str];

    /// Display labels, indexed by dest storage index.
    fn dest_labels() -> &'static [&'static str];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_discriminant_order_is_the_coarseness_order() {
        assert!(Tier::PatchGlobal < Tier::PerStack);
        assert!(Tier::PerStack < Tier::PerLane);
    }

    #[test]
    fn covers_is_coarser_or_equal() {
        // Coarser or equal reaches finer.
        assert!(Tier::PatchGlobal.covers(Tier::PerLane));
        assert!(Tier::PatchGlobal.covers(Tier::PatchGlobal));
        assert!(Tier::PerStack.covers(Tier::PerLane));
        // Finer into coarser is the lossy collapse.
        assert!(!Tier::PerLane.covers(Tier::PerStack));
        assert!(!Tier::PerStack.covers(Tier::PatchGlobal));
    }

    /// vxn-1b's flat matrix is the degenerate case: everything `PerLane`, so
    /// every verdict is `Ok` without the synth doing anything.
    #[test]
    fn an_all_per_lane_roster_is_trivially_coherent() {
        assert!(Tier::PerLane.covers(Tier::PerLane));
    }

    #[test]
    fn smoothing_defaults_to_block() {
        assert_eq!(Smoothing::default(), Smoothing::Block);
    }
}
