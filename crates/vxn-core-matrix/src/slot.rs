//! The **patch's routing table** and the block's compiled routes: what a player
//! wires up, and what the evaluator is handed.
//!
//! Both synths already had the same slot — source, dest, depth, polarity,
//! shape, scale source, scale shape — and the same table around it. What
//! differed was everything the shared form now settles:
//!
//! - vxn-1b carried an `enabled` switch; vxn-2 folded "switched off" into
//!   `source = None` at table-rebuild time, so its evaluator never had a flag to
//!   read and its preset format had no column to write.
//! - vxn-1b stored **raw** depth and cooked it once per block, into
//!   [`RouteList::compile`]; vxn-2 stored an **already-cooked** depth and looked
//!   the dest gain up again on every eval.
//! - vxn-1b compiled a route list once per block, hoisting the sentinel checks,
//!   the zero-depth skip, the taper and the gain out of its lane loops; vxn-2
//!   re-ran all of that per stack.
//!
//! [`RouteList::compile`] is now the single entry point for all of it, and it
//! takes **raw** depth. That is the one hazard worth naming twice: a synth that
//! kept cooking at rebuild time and handed the cooked value here would cook
//! twice, cubing an already-cubed depth (0.5 → 0.125 → ~0.00195 — a ~64× loss of
//! pitch modulation), silently and plausibly.
//!
//! ## Two questions a slot answers, and why they are different
//!
//! [`MatrixSlot::is_active`] — switched on *and* both endpoints real — is what
//! the evaluator drops on. [`MatrixSlot::is_wired`] — both endpoints real,
//! **regardless** of the switch — is what persistence asks, because a
//! switched-off route still has wiring worth saving; writing only `is_active`
//! slots would turn the toggle into a destructive delete across a save/load. It
//! is also what a "find me a free slot" search must use, or seeding into the
//! first free slot would evict a route the player parked.
//!
//! ## What does *not* live here
//!
//! Wire and state encodings. vxn-2 nibble-packs a slot into one `u32` per row
//! and vxn-1b spends a widened byte record; the two formats have different,
//! explicitly stated compatibility contracts
//! ([ADR 0003](../../../../adrs/0003-vxn-core-matrix.md) §4), and vxn-2's word is
//! *exactly* full at 32 bits. A shared slot type deliberately says nothing about
//! how a synth spells one on disk.

use crate::curve::{Polarity, Shape};

// ── the endpoint seam ───────────────────────────────────────────────────────

/// A routable **source**, as the routing mechanism needs to see one.
///
/// Deliberately two methods rather than a whole roster: a slot is edited,
/// persisted and displayed in terms of the synth's own `SourceId`, so the type
/// has to cross this boundary — but the mechanism only ever asks it which
/// accumulator row it names and, for a scale source, which way it swings.
/// Everything else keyed on a source stays in the roster
/// ([`crate::roster::MatrixRoster`]), which is indexed by the `usize` this
/// returns.
///
/// Both synths' generated enums already carry inherent `idx` and `is_bipolar`
/// with these exact signatures, so an implementation is a forward and nothing
/// else — and inherent methods win name resolution, so the synth's own call
/// sites are unaffected by the trait being in scope.
pub trait SourceEndpoint: Copy {
    /// Storage index into the source table, or `None` for the empty-slot
    /// sentinel. The sentinel is a wire-format concern and does not cross the
    /// seam as a number; it crosses as this `None`.
    fn idx(self) -> Option<usize>;

    /// Whether this source emits a bipolar `[-1, 1]` shape rather than a
    /// unipolar `[0, 1]` one — read only for a slot's **scale** source, which
    /// the VCA folds into `[0, 1]` accordingly.
    fn is_bipolar(self) -> bool;
}

/// A routable **destination**, as the routing mechanism needs to see one.
///
/// The two numeric columns are what [`RouteList::compile`] folds into a route's
/// single `gain` factor, in this order: the taper first, then the native-unit
/// gain. Both are pure functions of the patch, which is the whole reason the
/// fold can happen once per block instead of once per lane.
pub trait DestEndpoint: Copy {
    /// Storage index into the dest accumulator, or `None` for the sentinel.
    fn idx(self) -> Option<usize>;

    /// Native-unit gain — the factor that turns a normalised `[-1, 1]` route
    /// product into this destination's own unit (semitones, octaves, a
    /// fraction), so a depth of 1.0 means something musically comparable across
    /// dest kinds.
    fn gain(self) -> f32;

    /// Taper applied to a slot's **raw** depth, before [`Self::gain`].
    /// Identity for most destinations; semitone dests take a cubic so the low
    /// end of the fader has usable vibrato resolution.
    fn cook_depth(self, depth: f32) -> f32;
}

// ── the slot ────────────────────────────────────────────────────────────────

/// One matrix route as the player set it up: two endpoints, a depth, the two
/// shaping axes, an on/off switch, and an optional scale VCA.
///
/// `depth` is **raw** — the value the CLAP param, the preset file and the state
/// blob all carry, before any taper. Cooking happens in
/// [`RouteList::compile`] and nowhere else.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MatrixSlot<S, D> {
    pub source: S,
    pub dest: D,
    /// Bipolar `[-1, 1]`, untapered. See the type's note on raw depth.
    pub depth: f32,
    /// Range mapping, applied to the source value first.
    pub polarity: Polarity,
    /// Response bend, applied after [`Self::polarity`].
    pub shape: Shape,
    /// Whether the player has this route switched **on**. Independent of
    /// whether it is *wired*: a slot can have both endpoints set and still be
    /// off, which is what makes A/B-ing a route possible without losing its
    /// setup. Derive "active" from the endpoints alone and the only way to
    /// silence a route is to clear it.
    pub enabled: bool,
    /// Optional secondary "scale" source — a VCA on this route's depth. When
    /// wired, the slot's contribution is multiplied by that source's value
    /// normalised to `[0, 1]`, e.g. a mod wheel gating an LFO→pitch vibrato.
    /// The sentinel is identity. A *leaf* value, read from the same source
    /// table as the primary source, so it can never form a cycle.
    pub scale_src: S,
    /// Response bend on the normalised scale value, so the VCA need not be a
    /// straight line — `velocity` scaling an envelope route wants `Exp` so soft
    /// playing backs the route off faster than linear.
    ///
    /// No polarity twin: the VCA folds by the scale *source's* own polarity and
    /// has to land in `[0, 1]` regardless.
    pub scale_shape: Shape,
}

impl<S: Default, D: Default> Default for MatrixSlot<S, D> {
    /// A blank slot: unwired, at zero depth, and **off**. Picking a source is
    /// what switches it on (both editors do that on the `None`→real edge), so
    /// the default cannot be `enabled: true` without making every empty slot
    /// read as a route someone deliberately armed.
    fn default() -> Self {
        Self {
            source: S::default(),
            dest: D::default(),
            depth: 0.0,
            polarity: Polarity::Direct,
            shape: Shape::Lin,
            enabled: false,
            scale_src: S::default(),
            scale_shape: Shape::Lin,
        }
    }
}

impl<S: SourceEndpoint, D: DestEndpoint> MatrixSlot<S, D> {
    /// A slot **contributes to a dest** only when it is switched on *and* both
    /// endpoints are real. [`RouteList::compile`] additionally drops
    /// `depth == 0` slots; an inactive slot here is inert regardless of depth.
    #[inline]
    pub fn is_active(&self) -> bool {
        self.enabled && self.is_wired()
    }

    /// Whether both endpoints are real, **regardless of the on/off switch**.
    ///
    /// This is the "has the player set this slot up?" question. Persistence
    /// asks it, and so does anything hunting for a free slot — reaching for
    /// [`Self::is_active`] in either place quietly discards exactly what the
    /// toggle exists to preserve.
    #[inline]
    pub fn is_wired(&self) -> bool {
        self.source.idx().is_some() && self.dest.idx().is_some()
    }
}

/// A patch's whole routing topology: `N` slots, in the order the player sees
/// them.
///
/// That order is load-bearing all the way down. Destinations accumulate
/// additively and float addition is not associative, so "the same routes in the
/// same order" is what keeps any two evaluator paths bit-exact against each
/// other.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MatrixTable<S, D, const N: usize> {
    pub slots: [MatrixSlot<S, D>; N],
}

impl<S: Copy + Default, D: Copy + Default, const N: usize> Default for MatrixTable<S, D, N> {
    fn default() -> Self {
        Self {
            slots: [MatrixSlot::default(); N],
        }
    }
}

// ── compiled routes ─────────────────────────────────────────────────────────

/// One active slot with its **lane-invariant half already resolved**.
///
/// Everything a raw slot would make an evaluator re-derive — the two sentinel
/// unwraps, the on/off switch, the zero-depth skip, the depth taper and the
/// dest gain — is a pure function of the patch, yet an evaluator walking raw
/// slots re-runs the lot per lane, per voice, per block. Compiling them out once
/// is half of what makes a banked evaluator cheap; the other half is that the
/// curve and scale decisions become **outer**-loop dispatch, so the lane loop
/// underneath is branch-free and vectorises.
#[derive(Clone, Copy, Debug)]
pub struct Route {
    /// Storage index of the primary source.
    pub src: u8,
    /// Storage index of the destination.
    pub dest: u8,
    /// Range mapping applied to the source value.
    pub polarity: Polarity,
    /// Response bend applied after [`Self::polarity`].
    pub shape: Shape,
    /// `cook_depth(depth) · dest_gain`, folded into one factor.
    pub gain: f32,
    /// The VCA's source index, or `None` for an unscaled route.
    pub scale: Option<u8>,
    /// Whether that VCA source is bipolar, so the fold hoists out of the lane
    /// loop with everything else.
    pub scale_bipolar: bool,
    /// Bend on the VCA, hoisted for the same reason.
    pub scale_shape: Shape,
}

impl Route {
    /// The value a compiled list's unused tail holds. Never read — [`RouteList`]
    /// hands out only its live prefix — but an array needs filling, and a zero
    /// gain from the sentinel row is the initialiser that cannot mislead if one
    /// ever is.
    pub const INERT: Route = Route {
        src: 0,
        dest: 0,
        polarity: Polarity::Direct,
        shape: Shape::Lin,
        gain: 0.0,
        scale: None,
        scale_bipolar: false,
        scale_shape: Shape::Lin,
    };
}

/// The block's active routes, compiled once from the patch.
///
/// Fixed-size and allocation-free: the list is at most as long as the table, so
/// it is an array plus a count rather than anything that could reach an
/// allocator on the audio thread.
#[derive(Clone, Copy, Debug)]
pub struct RouteList<const N: usize> {
    routes: [Route; N],
    n: usize,
}

impl<const N: usize> Default for RouteList<N> {
    /// An empty list — no routes, so an evaluator handed one does nothing but
    /// zero its accumulator. What a synth's block context holds before the
    /// first [`RouteList::compile`].
    fn default() -> Self {
        Self {
            routes: [Route::INERT; N],
            n: 0,
        }
    }
}

impl<const N: usize> RouteList<N> {
    /// Resolve a patch's slots into active routes — **the** entry point, for
    /// every evaluator in either synth.
    ///
    /// Three things are settled here and nowhere else:
    ///
    /// - **The drop predicate.** Switched-off, unwired and zero-depth slots are
    ///   dropped here rather than branched over per lane. Two evaluators that
    ///   each spell this test for themselves will eventually disagree — vxn-1b's
    ///   two did, at `868faef`, where the banked path honoured `enabled` and the
    ///   scalar path did not, and only a parity test noticed. Sharing the
    ///   compile step makes that class of bug unrepresentable rather than merely
    ///   tested.
    /// - **The cook.** `cook_depth` runs on the **raw** depth, then the dest
    ///   gain, folded into one `gain` factor. A synth must not pre-cook.
    /// - **The order.** Slot order is preserved, and the compaction is stable,
    ///   so two paths walking this list accumulate in the same order and round
    ///   identically.
    pub fn compile<S: SourceEndpoint, D: DestEndpoint>(table: &MatrixTable<S, D, N>) -> Self {
        let mut routes = [Route::INERT; N];
        let mut n = 0;
        for slot in &table.slots {
            // `is_active` is the switch *and* both endpoints, so a switched-off
            // route never reaches a lane loop, exactly like an unwired one.
            if !slot.is_active() || slot.depth == 0.0 {
                continue;
            }
            let (Some(si), Some(di)) = (slot.source.idx(), slot.dest.idx()) else {
                continue;
            };
            routes[n] = Route {
                src: si as u8,
                dest: di as u8,
                polarity: slot.polarity,
                shape: slot.shape,
                gain: slot.dest.cook_depth(slot.depth) * slot.dest.gain(),
                scale: slot.scale_src.idx().map(|sc| sc as u8),
                scale_bipolar: slot.scale_src.is_bipolar(),
                scale_shape: slot.scale_shape,
            };
            n += 1;
        }
        Self { routes, n }
    }

    /// The active routes, in slot order.
    ///
    /// Bounding with `min` to spare the caller a slice-range panic path was
    /// tried and **measured slower** (vxn-2 `matrix_eval_full`, 95.5 → 98.2 ns):
    /// the `umin` lands in the loop preheader on every call, and the panic path
    /// it removes is cold and predicted-not-taken anyway. Left as the plain
    /// range.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Four sources, sentinel at 0. Bipolar iff the index is even, so a
    /// polarity mix-up in the compile step shows up rather than cancelling.
    #[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
    #[repr(u8)]
    enum Src {
        #[default]
        None = 0,
        Bi = 1,
        Uni = 2,
    }

    impl SourceEndpoint for Src {
        fn idx(self) -> Option<usize> {
            match self {
                Src::None => None,
                _ => Some(self as usize - 1),
            }
        }
        fn is_bipolar(self) -> bool {
            matches!(self, Src::Bi)
        }
    }

    /// Two dests: one plain, one carrying both a taper and a gain, so the fold
    /// order (`taper` then `gain`) is observable.
    #[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
    #[repr(u8)]
    enum Dst {
        #[default]
        None = 0,
        Plain = 1,
        Cooked = 2,
    }

    impl DestEndpoint for Dst {
        fn idx(self) -> Option<usize> {
            match self {
                Dst::None => None,
                _ => Some(self as usize - 1),
            }
        }
        fn gain(self) -> f32 {
            match self {
                Dst::Cooked => 12.0,
                _ => 1.0,
            }
        }
        fn cook_depth(self, depth: f32) -> f32 {
            match self {
                Dst::Cooked => depth * depth * depth,
                _ => depth,
            }
        }
    }

    type Slot = MatrixSlot<Src, Dst>;
    type Table = MatrixTable<Src, Dst, 4>;

    fn wired(source: Src, dest: Dst, depth: f32) -> Slot {
        Slot {
            source,
            dest,
            depth,
            enabled: true,
            ..Slot::default()
        }
    }

    #[test]
    fn a_blank_slot_is_unwired_inactive_and_off() {
        let s = Slot::default();
        assert!(!s.is_wired());
        assert!(!s.is_active());
        assert!(!s.enabled);
        assert_eq!(s.depth, 0.0);
    }

    /// The distinction the whole toggle rests on: wiring survives the switch.
    #[test]
    fn wired_and_active_are_different_questions() {
        let mut s = wired(Src::Bi, Dst::Plain, 0.5);
        assert!(s.is_wired() && s.is_active());
        s.enabled = false;
        assert!(s.is_wired(), "switching off must not unwire");
        assert!(!s.is_active());
        // Half-wired is neither, switch or no switch.
        s.enabled = true;
        s.dest = Dst::None;
        assert!(!s.is_wired() && !s.is_active());
    }

    #[test]
    fn compile_drops_off_unwired_and_zero_depth_slots() {
        let mut t = Table::default();
        t.slots[0] = wired(Src::Bi, Dst::Plain, 0.5);
        t.slots[1] = Slot { enabled: false, ..wired(Src::Uni, Dst::Plain, 0.5) };
        t.slots[2] = wired(Src::Bi, Dst::None, 0.5);
        t.slots[3] = wired(Src::Uni, Dst::Plain, 0.0);

        let list = RouteList::compile(&t);
        assert_eq!(list.active().len(), 1);
        assert_eq!(list.active()[0].src, Src::Bi.idx().unwrap() as u8);
        assert!(!list.is_empty());
        assert!(RouteList::compile(&Table::default()).is_empty());
    }

    /// Compaction must be **stable**: the surviving routes keep their relative
    /// slot order, because that order is what two evaluators round identically
    /// against.
    #[test]
    fn compaction_preserves_slot_order() {
        let mut t = Table::default();
        t.slots[0] = wired(Src::Uni, Dst::Plain, 0.0); // dropped
        t.slots[1] = wired(Src::Bi, Dst::Plain, 0.25);
        t.slots[2] = Slot { enabled: false, ..wired(Src::Uni, Dst::Plain, 0.5) }; // dropped
        t.slots[3] = wired(Src::Uni, Dst::Cooked, 0.5);

        let list = RouteList::compile(&t);
        let srcs: Vec<u8> = list.active().iter().map(|r| r.src).collect();
        assert_eq!(srcs, vec![Src::Bi.idx().unwrap() as u8, Src::Uni.idx().unwrap() as u8]);
    }

    /// The taper runs on the raw depth and the gain runs after it. Getting the
    /// order backwards would give `(0.5 · 12)³`, six thousand times the answer.
    #[test]
    fn gain_is_the_taper_then_the_native_unit() {
        let mut t = Table::default();
        t.slots[0] = wired(Src::Bi, Dst::Cooked, 0.5);
        t.slots[1] = wired(Src::Bi, Dst::Plain, 0.5);
        let list = RouteList::compile(&t);
        assert_eq!(list.active()[0].gain, 0.125 * 12.0);
        assert_eq!(list.active()[1].gain, 0.5);
    }

    /// The scale source's polarity and bend ride the route, resolved once —
    /// and an unscaled route carries `None` rather than a source that happens
    /// to sit at 1.0.
    #[test]
    fn scale_decisions_are_resolved_at_compile_time() {
        let mut t = Table::default();
        t.slots[0] = Slot {
            scale_src: Src::Bi,
            scale_shape: Shape::Exp,
            ..wired(Src::Uni, Dst::Plain, 1.0)
        };
        t.slots[1] = Slot {
            scale_src: Src::Uni,
            ..wired(Src::Uni, Dst::Plain, 1.0)
        };
        t.slots[2] = wired(Src::Uni, Dst::Plain, 1.0);

        let list = RouteList::compile(&t);
        assert_eq!(list.active()[0].scale, Some(Src::Bi.idx().unwrap() as u8));
        assert!(list.active()[0].scale_bipolar);
        assert_eq!(list.active()[0].scale_shape, Shape::Exp);
        assert_eq!(list.active()[1].scale, Some(Src::Uni.idx().unwrap() as u8));
        assert!(!list.active()[1].scale_bipolar);
        assert_eq!(list.active()[2].scale, None);
    }

    /// A full table compiles to a full list — the array is exactly the table's
    /// width, so nothing can overrun it and nothing is silently dropped.
    #[test]
    fn a_fully_wired_table_compiles_every_slot() {
        let mut t = Table::default();
        for s in t.slots.iter_mut() {
            *s = wired(Src::Uni, Dst::Plain, 1.0);
        }
        assert_eq!(RouteList::compile(&t).active().len(), 4);
    }

    #[test]
    fn a_default_route_list_is_empty() {
        let list: RouteList<4> = RouteList::default();
        assert!(list.is_empty());
        assert!(list.active().is_empty());
    }
}
