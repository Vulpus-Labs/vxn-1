//! The **curve-shaping vocabulary**: the two axes a route's response
//! decomposes into, the flat code preset files still spell them as, and the
//! scale VCA that maps a secondary source into `[0, 1]`.
//!
//! This is the code that prompted [E049](../../../../epics/closed/E049-shared-matrix-routing.md).
//! Adding the `abs` polarity, the polarity/shape split and the scale-VCA bend
//! meant writing the same ~200 lines into vxn-2 and then into vxn-1b, by hand,
//! 96 minutes apart (commits `bbff167`, `868faef`). None of it is specific to
//! FM or subtractive synthesis: it is arithmetic on a normalised source value.
//!
//! ## The two axes
//!
//! A slot's shaping is a [`Polarity`] that maps the source's *range*, then a
//! [`Shape`] that bends the *response* within it. Polarity runs first, so
//! `Bipolar` + `Exp` squares the AC-coupled value rather than the raw one.
//!
//! The **scale VCA has the same two axes** (0341). It used to have only the
//! bend, because its output has to land in `[0, 1]` and the fold that gets it
//! there was taken to leave no room for a polarity choice. It does: the
//! polarity *is* the choice of how to land there, and [`ScaleFold`] enumerates
//! the four maps the three settings resolve to. `Abs` is the one that costs
//! real behaviour when missing — `voice-position` scaling a route can only mean
//! "the voices on one side of the spread" without it, never "the voices at both
//! edges", which is exactly the case `Abs` was added to the primary axis for.
//!
//! ## Two spellings of the same dispatch
//!
//! Every arm is a `pub` free function ([`pol_bipolar`], [`shape_log`],
//! [`bend_exp`], …) *and* is reachable through a matching dispatcher
//! ([`map_polarity`], [`bend`], [`bend_unit`], [`shape`]). That is deliberate,
//! not redundancy:
//!
//! - The **free functions** are what a lane loop wants. Both synths dispatch
//!   the `(polarity, shape)` pair once per route and expand a straight-line
//!   loop per arm; hoisting `scale_norm`'s two decisions out of vxn-2's lane
//!   loop cut a fully-scaled 16-slot eval by ~47%.
//! - The **dispatchers** are what a scalar, one-value-at-a-time caller wants —
//!   vxn-1b's per-voice evaluator and its Amp factoring, where there is no loop
//!   to hoist out of and the match is one branch on a per-slot constant.
//!
//! Both spell the *same* arithmetic exactly once, because the dispatchers call
//! the free functions. Before this module they were written out twice per
//! synth and four times across the repo.
//!
//! ## Do not "tidy" the arithmetic
//!
//! This runs in two products' audio threads and the loop is unforgiving. Two
//! rewrites that look like improvements and are not, both measured:
//!
//! - [`shape_log`] keeps a **branch**, not `f32::copysign`. `copysign` lost.
//! - [`clamp_unit`] keeps `max`/`min`, not [`f32::clamp`], whose `min > max`
//!   assertion drops a panic path into the loop and cost ~7% of the whole eval.
//!
//! Neither claim should be taken on faith after a change that moves this code
//! across a crate boundary — LTO inlining is not the same problem as
//! within-crate inlining. Re-measure with `vxn2-osc-bench`'s `matrix_eval_full`
//! / `matrix_eval_scaled`.

// ── matrix_enum! ───────────────────────────────────────────────────────────

/// Declare a matrix enum and everything keyed on its discriminants: the enum
/// itself, the wire-name table, the display-label table, the `from_u8` decoder,
/// an `ALL` slice in discriminant order, the sentinel-free roster tables, and —
/// depending on which entry form is used — the source polarity predicate or the
/// destination's gain, depth taper, tier and smoothing class.
///
/// Before this, each of those was written out separately and kept in step by
/// hand: five parallel lists per enum, all indexed by the same `u8`, in each
/// synth. The tests checked their *lengths* and the `from_u8` round-trip, so a
/// transposed name/label pair was invisible until a user read the wrong name in
/// the mod matrix. Generating them from one row list makes that transposition
/// unrepresentable rather than merely untested.
///
/// # The three entry forms
///
/// **Axis enums** (`Polarity`, `Shape`) have no sentinel and no columns beyond
/// the name pair — a row is `Variant = discriminant, "wire-name", "Label"`.
///
/// **Source enums** add a `sentinel` row and two mandatory columns — a
/// `uni` / `bi` polarity, and a `tier`:
///
/// ```text
/// Lfo1     = 1, "lfo1",     "LFO 1",    bi,  tier = patch_global;
/// Velocity = 7, "velocity", "Velocity", uni, tier = per_stack;
/// ```
///
/// The tier column is the same vocabulary as a destination's and is read by the
/// same rule ([`coherence`](crate::coherence)). It is here rather than in a
/// hand-written `tier()` match because that match was the last parallel list
/// left in vxn-2's roster after 0332 — the one place a new source could still
/// be added without a granularity decision, and the decision it skipped is the
/// one that silently collapses eight lanes into one. Omit it and the row
/// matches no rule, so the enum is never declared:
///
/// ```compile_fail
/// use vxn_core_matrix::matrix_enum;
/// matrix_enum! {
///     Src, fallback = None, names = S_NAMES, labels = S_LABELS,
///     roster_names = RS_NAMES, roster_labels = RS_LABELS, polarity;
///     sentinel None = 0, "none", "—";
///     Lfo1 = 1, "lfo1", "LFO 1", bi;
/// }
/// ```
///
/// **Destination enums** add a `sentinel` row and four columns, all mandatory:
///
/// ```text
/// Cutoff = 4, "cutoff", "Cutoff", gain = 48.0, taper = linear,
///              tier = per_stack, smooth = block;
/// Pitch  = 1, "pitch",  "Pitch",  gain = 12.0, taper = cubic,
///              tier = per_lane,  smooth = quantum_cascade;
/// ```
///
/// `taper` is `linear` or `cubic`; `tier` is `patch_global` / `per_stack` /
/// `per_lane` ([`Tier`](crate::roster::Tier)); `smooth` is `block` / `quantum` /
/// `quantum_cascade` / `per_sample` ([`Smoothing`](crate::roster::Smoothing)).
/// The property this buys is the one the name tables already buy: **you cannot
/// add a destination without deciding**, because a row with a column missing
/// does not match the rule and the macro does not expand:
///
/// ```compile_fail
/// use vxn_core_matrix::matrix_enum;
/// matrix_enum! {
///     Dest, fallback = None, names = D_NAMES, labels = D_LABELS,
///     roster_names = RD_NAMES, roster_labels = RD_LABELS, roster_gains = RD_GAIN;
///     sentinel None = 0, "none", "—";
///     // `smooth` is missing, so this row matches no rule and the enum is
///     // never declared. Same for any other omitted column.
///     Cutoff = 1, "cutoff", "Cutoff", gain = 48.0, taper = linear,
///                 tier = per_stack;
/// }
/// ```
///
/// # The sentinel row is spelled out
///
/// Source and destination enums reserve discriminant 0 for the "empty slot"
/// sentinel, so it is declared with a `sentinel` keyword rather than as an
/// ordinary row. That is what lets the macro emit **two** name tables from one
/// row list: the synth's `[&str; N + 1]` wire tables (sentinel at 0, for
/// decoding a patch blob) and the roster's `[&str; N]` tables, whose index is
/// the *storage* index a compiled route carries — see
/// [ADR 0003](../../../../adrs/0003-vxn-core-matrix.md) §2 for why the two are
/// held apart. The macro also supplies the sentinel's `#[default]` attribute and
/// its inert column values, so neither can drift.
///
/// `fallback` is what `from_u8` returns for an out-of-range byte — the sentinel
/// for source/dest, `Lin` for shapes.
///
/// `#[macro_export]` puts this at the crate root, so consumers reach it as
/// `use vxn_core_matrix::matrix_enum;` rather than through this module.
#[macro_export]
macro_rules! matrix_enum {
    // Entry point: a destination enum — a sentinel row plus four mandatory
    // columns per row.
    (
        $(#[$emeta:meta])*
        $name:ident, fallback = $fallback:ident, names = $names:ident,
        labels = $labels:ident, roster_names = $rnames:ident,
        roster_labels = $rlabels:ident, roster_gains = $rgains:ident;
        $(#[$smeta:meta])*
        sentinel $sname:ident = $sdisc:literal, $swire:literal, $slabel:literal;
        $(
            $(#[$vmeta:meta])*
            $variant:ident = $disc:literal, $wire:literal, $label:literal,
            gain = $gain:literal, taper = $taper:ident, tier = $tier:ident,
            smooth = $smooth:ident;
        )+
    ) => {
        $crate::matrix_enum! {
            @base
            $(#[$emeta])*
            $name, fallback = $fallback, names = $names, labels = $labels;
            $(#[$smeta])* #[default] $sname = $sdisc, $swire, $slabel;
            $( $(#[$vmeta])* $variant = $disc, $wire, $label; )+
        }

        $crate::matrix_enum! {
            @roster $name, $rnames, $rlabels; $( $wire, $label; )+
        }

        #[doc = concat!(
            "Native-unit gain for each [`", stringify!($name), "`], indexed by **storage** ",
            "index like [`", stringify!($rnames), "`] — the factor that turns a normalised ",
            "`[-1, 1]` route product into the destination's own unit, so that a depth of 1.0 ",
            "means something musically comparable across dest kinds. Per-dest rationale lives ",
            "on the row that declares it."
        )]
        pub const $rgains: [f32; [$($disc),+].len()] = [ $($gain),+ ];

        impl $name {
            #[doc = concat!(
                "Native-unit gain for this destination — the `gain =` column of its row. ",
                "The sentinel reports `1.0`: it is inert (a slot with no dest is skipped ",
                "before any gain is read), and the identity is the answer that cannot ",
                "mislead if one ever is."
            )]
            #[inline]
            pub const fn gain(self) -> f32 {
                match self {
                    $name::$sname => 1.0,
                    $( $name::$variant => $gain, )+
                }
            }

            #[doc = concat!(
                "Taper applied to a slot's raw depth for this destination, **before** ",
                "[`", stringify!($name), "::gain`]. `linear` passes through; `cubic` is ",
                "`d³`, which keeps the sign and the full reach while widening the musical ",
                "low end — semitone dests need it because vibrato-scale amounts otherwise ",
                "live in the bottom couple of percent of fader travel."
            )]
            #[inline]
            pub fn cook_depth(self, depth: f32) -> f32 {
                match self {
                    $name::$sname => depth,
                    $( $name::$variant => $crate::matrix_enum!(@taper $taper, depth), )+
                }
            }

            #[doc = concat!(
                "Granularity tier of this destination — the `tier =` column. The sentinel ",
                "reports the finest tier; it is inert, and the coherence predicate ",
                "short-circuits an empty slot before it reads a tier."
            )]
            #[inline]
            pub const fn tier(self) -> $crate::roster::Tier {
                match self {
                    $name::$sname => $crate::roster::Tier::PerLane,
                    $( $name::$variant => $crate::matrix_enum!(@tier $tier), )+
                }
            }

            #[doc = concat!(
                "Smoothing class for this destination's summed total — the `smooth =` ",
                "column. Post-sum and per-destination, per ADR 0003 §3; the sentinel is ",
                "`Block`, which is also the class that costs nothing."
            )]
            #[inline]
            pub const fn smoothing(self) -> $crate::roster::Smoothing {
                match self {
                    $name::$sname => $crate::roster::Smoothing::Block,
                    $( $name::$variant => $crate::matrix_enum!(@smooth $smooth), )+
                }
            }
        }
    };

    // Entry point: a source enum — a sentinel row plus a polarity and a tier
    // column.
    (
        $(#[$emeta:meta])*
        $name:ident, fallback = $fallback:ident, names = $names:ident,
        labels = $labels:ident, roster_names = $rnames:ident,
        roster_labels = $rlabels:ident, polarity;
        $(#[$smeta:meta])*
        sentinel $sname:ident = $sdisc:literal, $swire:literal, $slabel:literal;
        $(
            $(#[$vmeta:meta])*
            $variant:ident = $disc:literal, $wire:literal, $label:literal, $pol:ident,
            tier = $tier:ident;
        )+
    ) => {
        $crate::matrix_enum! {
            @base
            $(#[$emeta])*
            $name, fallback = $fallback, names = $names, labels = $labels;
            $(#[$smeta])* #[default] $sname = $sdisc, $swire, $slabel;
            $( $(#[$vmeta])* $variant = $disc, $wire, $label; )+
        }

        $crate::matrix_enum! {
            @roster $name, $rnames, $rlabels; $( $wire, $label; )+
        }

        impl $name {
            /// Whether this source emits a **bipolar** `[-1, 1]` shape (vs a
            /// unipolar `[0, 1]` one). Consumed by the scale VCA
            /// (`vxn_core_matrix::curve::scale_norm`) to fold a bipolar scale
            /// source into the `[0, 1]` VCA range.
            ///
            /// The `uni` / `bi` column is not optional, so a new source still
            /// forces a polarity decision at compile time — no longer able to
            /// drift from the row it belongs to. The sentinel is unipolar,
            /// which is the passthrough fold.
            #[inline]
            pub const fn is_bipolar(self) -> bool {
                match self {
                    $name::$sname => false,
                    $( $name::$variant => $crate::matrix_enum!(@pol $pol), )+
                }
            }

            #[doc = concat!(
                "Granularity tier of this source — the `tier =` column. The sentinel ",
                "reports the *coarsest* tier, mirroring the destination sentinel's finest: ",
                "each is the value that cannot itself provoke a verdict, and the coherence ",
                "predicate short-circuits an empty slot before it reads either."
            )]
            #[inline]
            pub const fn tier(self) -> $crate::roster::Tier {
                match self {
                    $name::$sname => $crate::roster::Tier::PatchGlobal,
                    $( $name::$variant => $crate::matrix_enum!(@tier $tier), )+
                }
            }
        }
    };

    // Entry point: an axis enum — no sentinel, no columns.
    (
        $(#[$emeta:meta])*
        $name:ident, fallback = $fallback:ident, names = $names:ident,
        labels = $labels:ident;
        $(
            $(#[$vmeta:meta])*
            $variant:ident = $disc:literal, $wire:literal, $label:literal;
        )+
    ) => {
        $crate::matrix_enum! {
            @base
            $(#[$emeta])*
            $name, fallback = $fallback, names = $names, labels = $labels;
            $( $(#[$vmeta])* $variant = $disc, $wire, $label; )+
        }
    };

    (@pol bi) => { true };
    (@pol uni) => { false };

    (@taper linear, $depth:ident) => { $depth };
    (@taper cubic, $depth:ident) => { $depth * $depth * $depth };

    (@tier patch_global) => { $crate::roster::Tier::PatchGlobal };
    (@tier per_stack) => { $crate::roster::Tier::PerStack };
    (@tier per_lane) => { $crate::roster::Tier::PerLane };

    (@smooth block) => { $crate::roster::Smoothing::Block };
    (@smooth quantum) => { $crate::roster::Smoothing::Quantum };
    (@smooth quantum_cascade) => { $crate::roster::Smoothing::QuantumCascade };
    (@smooth per_sample) => { $crate::roster::Smoothing::PerSample };

    // The sentinel-free half: the tables the roster seam is indexed by.
    (@roster $name:ident, $rnames:ident, $rlabels:ident; $( $wire:literal, $label:literal; )+) => {
        #[doc = concat!(
            "Machine ids for each routable [`", stringify!($name), "`], **sentinel ",
            "excluded**. Index = storage index, `0..N`, which is one less than the wire ",
            "discriminant and is what a compiled route carries (ADR 0003 §2)."
        )]
        pub const $rnames: [&str; [$($wire),+].len()] = [ $($wire),+ ];

        #[doc = concat!(
            "Display label for each routable [`", stringify!($name), "`]. Same indexing as [`",
            stringify!($rnames), "`]."
        )]
        pub const $rlabels: [&str; [$($label),+].len()] = [ $($label),+ ];
    };

    // The shared half: enum, both tables, decoder, ALL.
    (
        @base
        $(#[$emeta:meta])*
        $name:ident, fallback = $fallback:ident, names = $names:ident, labels = $labels:ident;
        $(
            $(#[$vmeta:meta])*
            $variant:ident = $disc:literal, $wire:literal, $label:literal;
        )+
    ) => {
        $(#[$emeta])*
        #[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
        #[repr(u8)]
        pub enum $name {
            $( $(#[$vmeta])* $variant = $disc, )+
        }

        #[doc = concat!(
            "Machine id (kebab-case wire name) for each [`", stringify!($name),
            "`]. Index = discriminant."
        )]
        pub const $names: [&str; [$($disc),+].len()] = [ $($wire),+ ];

        #[doc = concat!(
            "Display label for each [`", stringify!($name), "`]. Same indexing as [`",
            stringify!($names), "`]."
        )]
        pub const $labels: [&str; [$($disc),+].len()] = [ $($label),+ ];

        // Every generated table is positional, so the row list's order *is* the
        // discriminant order or nothing lines up. Since 0332 that invariant
        // carries gains and tapers as well as names: a row inserted next to its
        // siblings rather than at its discriminant would shift every later
        // dest's gain and wire name at once, and no test downstream could see
        // it, because they would all read the same shifted tables. Pinned here
        // so it is a compile error instead.
        const _: () = {
            let mut i = 0;
            while i < $name::ALL.len() {
                assert!(
                    $name::ALL[i] as usize == i,
                    concat!(
                        stringify!($name),
                        ": rows must be listed in discriminant order, one per discriminant, ",
                        "starting at 0 — every generated table is indexed by position"
                    )
                );
                i += 1;
            }
        };

        impl $name {
            /// Every variant, in discriminant order: `ALL[i] as u8 == i`. That
            /// is the property the name and label tables are indexed on, and it
            /// is asserted at compile time beside this declaration.
            pub const ALL: [$name; [$($disc),+].len()] = [ $($name::$variant),+ ];

            #[doc = concat!(
                "Decode a wire-format `u8`. Out of range → [`", stringify!($name), "::",
                stringify!($fallback), "`], so a corrupt patch blob degrades to an inert ",
                "slot rather than panicking."
            )]
            #[inline]
            pub fn from_u8(v: u8) -> Self {
                match v {
                    $( $disc => $name::$variant, )+
                    _ => $name::$fallback,
                }
            }
        }
    };
}

// ── the axes ────────────────────────────────────────────────────────────────

matrix_enum! {
    /// Range mapping applied to a source value, **before** the [`Shape`] bend.
    ///
    /// - `None` — passthrough; the source's native polarity reaches the dest.
    /// - `Bipolar` — AC-couple a unipolar `[0, 1]` source to `[-1, 1]` via
    ///   `2v − 1` (centred swing when routing mod-wheel/aftertouch into a
    ///   bipolar dest).
    /// - `Abs` — rectify a bipolar source to `[0, 1]` via `|v|`, so the route is
    ///   strongest at *both* extremes and silent at centre. A voice-position
    ///   source into a pan dest is the motivating case: `none` pans each voice
    ///   in proportion to its position, `abs` instead moves only the voices at
    ///   the edges of the spread and leaves the centre ones alone. Identity for
    ///   a source already unipolar.
    ///
    ///   Depth sign covers the mirror case, so there is deliberately no
    ///   `1 − |v|` mapping: pull depth negative and the edge voices are driven
    ///   *away* from the destination's own parameter value while the centre
    ///   voices keep it. "More at the centre" falls out of the parameter
    ///   already being the offset such a mapping would re-derive.
    ///
    /// ## The resting variant is `None`, and was `Direct` until 0340
    ///
    /// Renamed with the glyph picker, where "Direct" read as a fourth mapping
    /// rather than as the absence of one. Three separately-scoped edits went
    /// with it — the label, the wire name (`"direct"`, still accepted on read
    /// by [`polarity_from_name`]), and this variant — and **none** of them
    /// touched a discriminant. `curve_code` is `polarity · N_SHAPES + shape`, so
    /// renumbering would silently remap every saved route in both synths; the
    /// four pre-split preset spellings (`0 = lin`, `1 = exp`, `2 = log`,
    /// `3 = bipolar`) still mean what they always did.
    ///
    /// The passthrough arm keeps its old name, [`pol_direct`]: it says what the
    /// function *does*, which the rename did not change.
    Polarity, fallback = None, names = POLARITY_NAMES,
    labels = POLARITY_LABELS;
    #[default]
    None = 0, "none", "None";
    Bipolar = 1, "bipolar", "Bipolar";
    Abs = 2, "abs", "Abs";
}

matrix_enum! {
    /// Response bend applied **after** the [`Polarity`] mapping.
    ///
    /// - `Lin` — identity passthrough.
    /// - `Exp` — signed square `sign(v)·v²`: more extreme excursions.
    /// - `Log` — signed root `sign(v)·√|v|`: compresses toward 0.
    ///
    /// Both bends preserve sign, so neither moves a value across zero.
    Shape, fallback = Lin, names = SHAPE_NAMES,
    labels = SHAPE_LABELS;
    #[default]
    Lin = 0, "lin", "Lin";
    Exp = 1, "exp", "Exp";
    Log = 2, "log", "Log";
}

/// Count of polarity variants.
pub const N_POLARITIES: usize = POLARITY_NAMES.len();
/// Count of shape variants. No sentinel — `Lin` is a real shape.
pub const N_SHAPES: usize = SHAPE_NAMES.len();

// ── the flat preset code ────────────────────────────────────────────────────

/// Count of `(polarity, shape)` combinations — the width of the flat curve code
/// that **preset files** (and, in vxn-2, the state blob) carry.
pub const N_CURVES: usize = N_POLARITIES * N_SHAPES;

/// Compose a `(polarity, shape)` pair into the flat code,
/// `polarity · N_SHAPES + shape`.
///
/// This exists for one reason: some surfaces still spell the two axes as a
/// single `curve` value. The stride is chosen so the four pre-split codes keep
/// their exact meanings — `0 = lin`, `1 = exp`, `2 = log`, `3 = bipolar` (which
/// was always bipolar with a linear bend) — and [`CURVE_NAMES`] elides the
/// `direct` polarity and the `lin` shape, so those four spellings still parse.
/// Presets written before the split load unchanged, **in both synths**.
///
/// Which surfaces use it is per-synth and not this crate's business: vxn-2
/// nibble-packs it into its state blob for blob compatibility, while vxn-1b
/// stores the two axes as separate bytes and rejects older blobs on read, so it
/// needs the flat code only for preset TOML. Neither synth's UI edit wire uses
/// it — those address one axis at a time.
#[inline]
pub const fn curve_code(polarity: Polarity, shape: Shape) -> u8 {
    polarity as u8 * N_SHAPES as u8 + shape as u8
}

/// Decode a [`Polarity`]'s wire name, accepting the spelling it had before
/// 0340 alongside the current one.
///
/// `Polarity::None` was `Direct` and spelled `"direct"` on the wire until the
/// glyph picker made "Direct" read as a fourth mapping rather than the absence
/// of one. Nothing else about the axis moved — not the discriminant, not
/// [`curve_code`] — so an old spelling names exactly the same range map and is
/// simply accepted rather than warned about.
///
/// Case-insensitive, and `None` (the `Option`) for a name in neither vocabulary,
/// so a caller can warn and fall back rather than guess.
///
/// Lives here rather than in a synth because there is only one polarity
/// vocabulary and only one legacy spelling of it; a per-synth reader would be
/// the same table written twice, which is what [`crate::curve`] exists to stop.
pub fn polarity_from_name(name: &str) -> Option<Polarity> {
    let lc = name.trim();
    if lc.eq_ignore_ascii_case("direct") {
        return Some(Polarity::None);
    }
    POLARITY_NAMES
        .iter()
        .position(|n| n.eq_ignore_ascii_case(lc))
        .map(|i| Polarity::from_u8(i as u8))
}

/// Split a flat code back into its `(polarity, shape)` pair. Out-of-range codes
/// degrade to `(None, Lin)` rather than aliasing onto a real curve.
#[inline]
pub fn curve_split(code: u8) -> (Polarity, Shape) {
    if code as usize >= N_CURVES {
        return (Polarity::None, Shape::Lin);
    }
    (
        Polarity::from_u8(code / N_SHAPES as u8),
        Shape::from_u8(code % N_SHAPES as u8),
    )
}

/// Flat curve machine id, indexed by [`curve_code`]. The four legacy spellings
/// are load-bearing — see [`curve_code`].
pub const CURVE_NAMES: [&str; N_CURVES] = [
    "lin",
    "exp",
    "log",
    "bipolar",
    "bipolar-exp",
    "bipolar-log",
    "abs",
    "abs-exp",
    "abs-log",
];

/// Flat curve display label. Same indexing as [`CURVE_NAMES`].
///
/// Only vxn-2 has a UI surface that offers the flat code as one pick-list
/// today; vxn-1b splits the axes in its faceplate and reads
/// [`POLARITY_LABELS`] / [`SHAPE_LABELS`] instead. The table lives here anyway
/// because it is [`CURVE_NAMES`]' twin and the pair is exactly what the
/// hand-maintained form kept getting out of step — a synth that grows the flat
/// pick-list should not have to re-derive it.
pub const CURVE_LABELS: [&str; N_CURVES] = [
    "Lin",
    "Exp",
    "Log",
    "Bipolar",
    "Bipolar Exp",
    "Bipolar Log",
    "Abs",
    "Abs Exp",
    "Abs Log",
];

// ── the arms ────────────────────────────────────────────────────────────────

/// Polarity map: passthrough. Applied to the raw source value, before the bend.
#[inline(always)]
pub fn pol_direct(v: f32) -> f32 {
    v
}

/// Polarity map: AC-couple a unipolar `[0, 1]` source to `[-1, 1]`.
#[inline(always)]
pub fn pol_bipolar(v: f32) -> f32 {
    2.0 * v - 1.0
}

/// Polarity map: rectify a bipolar source to `[0, 1]`.
#[inline(always)]
pub fn pol_abs(v: f32) -> f32 {
    v.abs()
}

/// Shape bend: identity.
#[inline(always)]
pub fn shape_lin(v: f32) -> f32 {
    v
}

/// Shape bend: signed square, `sign(v)·v²`. Sign-preserving, so it never moves
/// a value across zero.
#[inline(always)]
pub fn shape_exp(v: f32) -> f32 {
    v.abs() * v
}

/// Shape bend: signed root, `sign(v)·√|v|`. Sign-preserving.
///
/// The branch is deliberate and **measured**: `f32::copysign` was tried here as
/// the branch-free spelling and lost to this. An earlier revision of this
/// comment claimed the opposite of what the code does — it read "`copysign`,
/// not a branch" above a body that is a branch — which is what a hand-copied
/// note does when only one of the two copies gets corrected.
#[inline(always)]
pub fn shape_log(v: f32) -> f32 {
    let mag = v.abs().sqrt();
    if v < 0.0 { -mag } else { mag }
}

/// Scale-source fold: a unipolar source is already in the VCA's range.
///
/// Passthrough is not laziness — folding a `[0, 1)` random into `[0.5, 1)`
/// would mean it could never gate the route to zero.
#[inline(always)]
pub fn fold_unipolar(v: f32) -> f32 {
    v
}

/// Scale-source fold: map a bipolar `[-1, 1]` source onto the VCA's `[0, 1]`.
#[inline(always)]
pub fn fold_bipolar(v: f32) -> f32 {
    (v + 1.0) * 0.5
}

/// The scale VCA's **resolved** range map: one of four arms, with the slot's
/// [`Polarity`] and the scale source's own polarity already collapsed together.
///
/// Two decisions become one per-slot constant here, which is the whole point.
/// `Polarity::None` is the only setting that consults the source — it means
/// "land in `[0, 1]` the way this source naturally does" — while `Abs` and
/// `Bipolar` are range maps in their own right and ignore it. So the lane loop
/// dispatches on **four** arms, not six, and the arm count grew from 6 to 12
/// with the bend rather than to 18.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ScaleFold {
    /// [`fold_unipolar`] — `None` on a unipolar source. The default, and the
    /// identity.
    #[default]
    Passthrough,
    /// [`fold_bipolar`] — `None` on a bipolar source: `(v + 1)·0.5`, so the
    /// source's centre sits the VCA half open.
    Fold,
    /// [`pol_abs`] — `Abs`, whichever way the source swings: `|v|`. The gate
    /// opens at **both** extremes of a bipolar source and shuts at its centre;
    /// the identity on a unipolar one, exactly as on the primary axis.
    Rectify,
    /// [`pol_bipolar`] — `Bipolar`, whichever way the source swings: `2v − 1`,
    /// which the clamp then floors at 0. The gate stays shut over the source's
    /// lower half and opens across the upper — a threshold-ish gate. On an
    /// already-bipolar source only `v ≥ 0.5` opens it at all; blunt, but a
    /// legitimate setting rather than a case to design around.
    AcCouple,
}

impl ScaleFold {
    /// Collapse a slot's scale [`Polarity`] and its scale source's own polarity
    /// into the one arm the lane loop dispatches on.
    ///
    /// `bipolar` is read **only** under [`Polarity::None`]. That is the
    /// asymmetry worth stating: the other two settings are absolute range maps,
    /// so a patch that picks one sounds the same whichever source is wired into
    /// the VCA.
    #[inline]
    pub const fn resolve(polarity: Polarity, bipolar: bool) -> Self {
        match polarity {
            Polarity::None => {
                if bipolar {
                    ScaleFold::Fold
                } else {
                    ScaleFold::Passthrough
                }
            }
            Polarity::Abs => ScaleFold::Rectify,
            Polarity::Bipolar => ScaleFold::AcCouple,
        }
    }
}

/// Clamp to the VCA's `[0, 1]`.
///
/// `max`/`min` rather than [`f32::clamp`]: `clamp` carries a `min > max`
/// assertion whose panic path lands in the hot loop and cost ~7% of the whole
/// eval when measured in vxn-2. The two agree on every finite input; they
/// differ only on NaN, where this returns `0.0` (`f32::max` yields the non-NaN
/// operand) instead of propagating. Shutting the gate on a NaN source is the
/// better failure mode anyway — the alternative poisons the dest accumulator
/// for the whole block.
#[inline(always)]
pub fn clamp_unit(v: f32) -> f32 {
    v.max(0.0).min(1.0)
}

/// Scale bend: identity.
#[inline(always)]
pub fn bend_lin(v: f32) -> f32 {
    v
}

/// Scale bend: square. Input is already clamped to `[0, 1]`, so this needs none
/// of the sign handling [`shape_exp`] carries.
#[inline(always)]
pub fn bend_exp(v: f32) -> f32 {
    v * v
}

/// Scale bend: root. Input is already clamped to `[0, 1]`.
#[inline(always)]
pub fn bend_log(v: f32) -> f32 {
    v.sqrt()
}

// ── the dispatchers ─────────────────────────────────────────────────────────

/// The polarity half of a slot's shaping, dispatched on a value at a time.
#[inline]
pub fn map_polarity(polarity: Polarity, v: f32) -> f32 {
    match polarity {
        Polarity::None => pol_direct(v),
        Polarity::Bipolar => pol_bipolar(v),
        Polarity::Abs => pol_abs(v),
    }
}

/// The shape half of a slot's shaping, dispatched on a value at a time.
#[inline]
pub fn bend(shape: Shape, v: f32) -> f32 {
    match shape {
        Shape::Lin => shape_lin(v),
        Shape::Exp => shape_exp(v),
        Shape::Log => shape_log(v),
    }
}

/// Map a source value's range, then bend its response — both axes, in order.
/// Polarity runs **first**, so `Bipolar` + `Exp` squares the AC-coupled value
/// rather than the raw one.
#[inline]
pub fn shape(polarity: Polarity, shape: Shape, v: f32) -> f32 {
    bend(shape, map_polarity(polarity, v))
}

/// The scale bend on an already-clamped `[0, 1]` value. Separate from [`bend`]
/// because the input's sign is known, so none of the sign handling is needed.
#[inline]
pub fn bend_unit(shape: Shape, v: f32) -> f32 {
    match shape {
        Shape::Lin => bend_lin(v),
        Shape::Exp => bend_exp(v),
        Shape::Log => bend_log(v),
    }
}

/// The scale VCA's range map, dispatched on a value at a time. The scalar twin
/// of the arms a lane loop expands.
#[inline]
pub fn scale_fold(fold: ScaleFold, v: f32) -> f32 {
    match fold {
        ScaleFold::Passthrough => fold_unipolar(v),
        ScaleFold::Fold => fold_bipolar(v),
        ScaleFold::Rectify => pol_abs(v),
        ScaleFold::AcCouple => pol_bipolar(v),
    }
}

/// Map a scale source's value into the `[0, 1]` VCA range by `polarity`, clamp
/// it there, then bend it by `shape` — the scale VCA's own two axes, in order.
///
/// `bipolar` is the scale *source's* own polarity — hence a bare `bool` rather
/// than a source id, which is the one thing this crate deliberately does not
/// know about. Each synth reads it off its own `SourceId` at the call site. It
/// is consulted only under [`Polarity::None`]; see [`ScaleFold::resolve`].
///
/// ## Why the clamp sits between them
///
/// The VCA has to land in `[0, 1]` whatever the polarity does, and that
/// requirement is what fixes the order rather than an argument against a
/// polarity axis existing at all (0341 — an earlier revision of this note read
/// the requirement as the latter). Clamping **before** the bend means the bend
/// only ever sees `[0, 1]` and therefore can't leave it: on a non-negative
/// input `Exp` is `v²` and `Log` is `√v`, both fixing 0 and 1 and monotonic
/// between. `0 → the route contributes nothing`, `1 → the route at its full
/// configured depth`, whichever bend is set.
///
/// The tempting alternative — map by polarity, then fold whatever comes out —
/// degenerates: a unipolar source under `Bipolar` would be `2v − 1` folded back
/// by `(v + 1)·0.5`, an exact round trip to `v`, making the setting a no-op.
///
/// [`Polarity::None`] with [`Shape::Lin`] is the pre-0341 arithmetic
/// unchanged — it *is* the fold-and-bend this used to be, so a patch that
/// predates the polarity axis renders bit-identically.
///
/// A lane loop should **not** call this per lane: both decisions are per-slot
/// constants, so hoist them into a [`ScaleFold`] and expand the arms instead.
/// Doing that cut vxn-2's fully-scaled 16-slot eval by ~47%. Dispatch the range
/// map and the bend **separately** — six expanded loops rather than the twelve
/// a fused `(fold, bend)` match costs; [`crate::eval::eval_dests_bank`] carries
/// the measurement.
#[inline]
pub fn scale_norm(bipolar: bool, v: f32, polarity: Polarity, shape: Shape) -> f32 {
    bend_unit(
        shape,
        clamp_unit(scale_fold(ScaleFold::resolve(polarity, bipolar), v)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The flat code is what preset files carry in **both** synths, so the four
    /// spellings that predate the axis split must still land on their original
    /// meanings — codes 0..=3 are load-bearing.
    #[test]
    fn curve_code_preserves_pre_split_encoding() {
        let legacy = [
            (0u8, Polarity::None, Shape::Lin, "lin"),
            (1, Polarity::None, Shape::Exp, "exp"),
            (2, Polarity::None, Shape::Log, "log"),
            (3, Polarity::Bipolar, Shape::Lin, "bipolar"),
        ];
        for (code, pol, shape, name) in legacy {
            assert_eq!(curve_code(pol, shape), code, "{name} code moved");
            assert_eq!(curve_split(code), (pol, shape), "{name} decode moved");
            assert_eq!(CURVE_NAMES[code as usize], name);
        }
    }

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
        assert_eq!(curve_split(N_CURVES as u8), (Polarity::None, Shape::Lin));
        assert_eq!(curve_split(255), (Polarity::None, Shape::Lin));
    }

    #[test]
    fn tables_are_sized_and_describe_their_own_variant() {
        assert_eq!(POLARITY_NAMES.len(), N_POLARITIES);
        assert_eq!(POLARITY_LABELS.len(), N_POLARITIES);
        assert_eq!(SHAPE_NAMES.len(), N_SHAPES);
        assert_eq!(SHAPE_LABELS.len(), N_SHAPES);
        assert_eq!(CURVE_NAMES.len(), N_CURVES);
        assert_eq!(CURVE_LABELS.len(), N_CURVES);
        for (i, p) in Polarity::ALL.iter().enumerate() {
            assert_eq!(*p as usize, i);
            assert_eq!(Polarity::from_u8(i as u8), *p);
        }
        for (i, s) in Shape::ALL.iter().enumerate() {
            assert_eq!(*s as usize, i);
            assert_eq!(Shape::from_u8(i as u8), *s);
        }
        assert_eq!(
            (POLARITY_NAMES[1], POLARITY_LABELS[1]),
            ("bipolar", "Bipolar")
        );
        assert_eq!((SHAPE_NAMES[2], SHAPE_LABELS[2]), ("log", "Log"));
    }

    #[test]
    fn from_u8_degrades_out_of_range() {
        assert_eq!(Polarity::from_u8(200), Polarity::None);
        assert_eq!(Shape::from_u8(200), Shape::Lin);
    }

    /// The dispatchers must be exactly the free functions — that equality is
    /// what lets a scalar caller and a hoisted lane loop stay bit-exact.
    #[test]
    fn dispatchers_agree_with_the_arms_bitwise() {
        let vs = [-1.0f32, -0.7, -0.25, 0.0, 0.25, 0.7, 1.0];
        for v in vs {
            assert_eq!(map_polarity(Polarity::None, v).to_bits(), pol_direct(v).to_bits());
            assert_eq!(map_polarity(Polarity::Bipolar, v).to_bits(), pol_bipolar(v).to_bits());
            assert_eq!(map_polarity(Polarity::Abs, v).to_bits(), pol_abs(v).to_bits());
            assert_eq!(bend(Shape::Lin, v).to_bits(), shape_lin(v).to_bits());
            assert_eq!(bend(Shape::Exp, v).to_bits(), shape_exp(v).to_bits());
            assert_eq!(bend(Shape::Log, v).to_bits(), shape_log(v).to_bits());
        }
        for v in [0.0f32, 0.25, 0.5, 1.0] {
            assert_eq!(bend_unit(Shape::Lin, v).to_bits(), bend_lin(v).to_bits());
            assert_eq!(bend_unit(Shape::Exp, v).to_bits(), bend_exp(v).to_bits());
            assert_eq!(bend_unit(Shape::Log, v).to_bits(), bend_log(v).to_bits());
        }
    }

    #[test]
    fn bends_preserve_sign_and_fix_the_endpoints() {
        for v in [-1.0f32, -0.5, 0.5, 1.0] {
            assert_eq!(shape_exp(v).signum(), v.signum());
            assert_eq!(shape_log(v).signum(), v.signum());
        }
        assert_eq!(shape_exp(1.0), 1.0);
        assert_eq!(shape_log(1.0), 1.0);
        assert_eq!(shape_exp(-1.0), -1.0);
        assert_eq!(shape_log(-1.0), -1.0);
    }

    /// `None` is the pre-0341 arithmetic, unchanged. Every assertion here
    /// predates the polarity axis and none of its numbers moved — which is the
    /// bit-identical-patch claim at its smallest scale.
    #[test]
    fn scale_norm_none_folds_clamps_then_bends() {
        use Polarity::None;
        // Unipolar passes through.
        assert_eq!(scale_norm(false, 0.3, None, Shape::Lin), 0.3);
        assert_eq!(scale_norm(false, 1.0, None, Shape::Lin), 1.0);
        // Bipolar folds: centre → half, extremes → the gate's ends.
        assert_eq!(scale_norm(true, 0.0, None, Shape::Lin), 0.5);
        assert_eq!(scale_norm(true, 1.0, None, Shape::Lin), 1.0);
        assert_eq!(scale_norm(true, -1.0, None, Shape::Lin), 0.0);
        // Out-of-range input is clamped, not wrapped.
        assert_eq!(scale_norm(false, 1.7, None, Shape::Lin), 1.0);
        assert_eq!(scale_norm(false, -0.4, None, Shape::Lin), 0.0);
        // Every bend fixes 0 and 1 and stays inside them.
        for shape in Shape::ALL {
            assert_eq!(scale_norm(false, 0.0, None, shape), 0.0);
            assert_eq!(scale_norm(false, 1.0, None, shape), 1.0);
            for v in [-2.0f32, -0.1, 0.0, 0.3, 0.9, 1.0, 3.0] {
                let n = scale_norm(false, v, None, shape);
                assert!((0.0..=1.0).contains(&n), "{shape:?}/{v} → {n}");
            }
        }
    }

    /// `Abs` is the setting the axis was added for: the gate opens at **both**
    /// extremes of a bipolar source — "the voices at both edges of the spread" —
    /// and is the identity on a unipolar one, exactly as on the primary axis.
    #[test]
    fn scale_norm_abs_opens_at_both_extremes() {
        use Polarity::Abs;
        assert_eq!(scale_norm(true, -1.0, Abs, Shape::Lin), 1.0);
        assert_eq!(scale_norm(true, 1.0, Abs, Shape::Lin), 1.0);
        assert_eq!(scale_norm(true, 0.0, Abs, Shape::Lin), 0.0);
        assert_eq!(scale_norm(true, -0.5, Abs, Shape::Lin), 0.5);
        // Identity on a unipolar source — nothing to rectify.
        for v in [0.0f32, 0.25, 0.6, 1.0] {
            assert_eq!(scale_norm(false, v, Abs, Shape::Lin), v);
        }
        // The source's own polarity is not consulted at all here.
        for v in [-2.0f32, -0.3, 0.0, 0.4, 1.0, 3.0] {
            assert_eq!(
                scale_norm(true, v, Abs, Shape::Lin),
                scale_norm(false, v, Abs, Shape::Lin)
            );
        }
    }

    /// `Bipolar` is the threshold-ish gate: shut over the source's lower half,
    /// opening across its upper. On an already-bipolar source the clamp does
    /// most of the work — only `v ≥ 0.5` opens it at all.
    #[test]
    fn scale_norm_bipolar_gates_on_the_upper_half() {
        use Polarity::Bipolar;
        assert_eq!(scale_norm(false, 0.0, Bipolar, Shape::Lin), 0.0);
        assert_eq!(scale_norm(false, 0.5, Bipolar, Shape::Lin), 0.0);
        assert_eq!(scale_norm(false, 0.75, Bipolar, Shape::Lin), 0.5);
        assert_eq!(scale_norm(false, 1.0, Bipolar, Shape::Lin), 1.0);
        // Everything at or below the halfway point is floored by the clamp.
        for v in [-1.0f32, -0.2, 0.0, 0.3, 0.49] {
            assert_eq!(scale_norm(false, v, Bipolar, Shape::Lin), 0.0);
        }
    }

    /// Whatever the polarity, the VCA lands in `[0, 1]` — the invariant the
    /// clamp-then-bend order exists to hold. A NaN source shuts the gate rather
    /// than poisoning the dest accumulator (see [`clamp_unit`]).
    #[test]
    fn scale_norm_lands_in_unit_range_for_every_combination() {
        for polarity in Polarity::ALL {
            for shape in Shape::ALL {
                for bipolar in [false, true] {
                    for v in [-3.0f32, -1.0, -0.4, 0.0, 0.3, 0.5, 1.0, 4.0] {
                        let n = scale_norm(bipolar, v, polarity, shape);
                        assert!(
                            (0.0..=1.0).contains(&n),
                            "{polarity:?}/{shape:?}/bipolar={bipolar}/{v} → {n}"
                        );
                    }
                    assert_eq!(scale_norm(bipolar, f32::NAN, polarity, shape), 0.0);
                }
            }
        }
    }

    /// [`ScaleFold::resolve`] collapses two decisions into four arms, and
    /// [`scale_fold`] must be exactly the free functions those arms are — the
    /// same equality [`dispatchers_agree_with_the_arms_bitwise`] asserts for the
    /// primary axis, and for the same reason.
    #[test]
    fn scale_fold_resolves_to_four_arms_bitwise() {
        assert_eq!(
            ScaleFold::resolve(Polarity::None, false),
            ScaleFold::Passthrough
        );
        assert_eq!(ScaleFold::resolve(Polarity::None, true), ScaleFold::Fold);
        // `Abs` and `Bipolar` ignore the source's own polarity — this is what
        // keeps the lane loop's range-map dispatch at four arms, not six.
        for bipolar in [false, true] {
            assert_eq!(
                ScaleFold::resolve(Polarity::Abs, bipolar),
                ScaleFold::Rectify
            );
            assert_eq!(
                ScaleFold::resolve(Polarity::Bipolar, bipolar),
                ScaleFold::AcCouple
            );
        }
        for v in [-2.0f32, -1.0, -0.25, 0.0, 0.25, 1.0, 3.0] {
            assert_eq!(
                scale_fold(ScaleFold::Passthrough, v).to_bits(),
                fold_unipolar(v).to_bits()
            );
            assert_eq!(
                scale_fold(ScaleFold::Fold, v).to_bits(),
                fold_bipolar(v).to_bits()
            );
            assert_eq!(
                scale_fold(ScaleFold::Rectify, v).to_bits(),
                pol_abs(v).to_bits()
            );
            assert_eq!(
                scale_fold(ScaleFold::AcCouple, v).to_bits(),
                pol_bipolar(v).to_bits()
            );
        }
    }

    /// The wire name moved at 0340 and the old one still reads. Both halves
    /// matter: `"direct"` must land on the *same* range map rather than on the
    /// fallback, or a preset written before the rename would quietly lose its
    /// polarity — which for a `bipolar`-scaled route is an audible change, not
    /// a cosmetic one.
    #[test]
    fn polarity_names_read_both_spellings_of_the_resting_map() {
        assert_eq!(polarity_from_name("none"), Some(Polarity::None));
        assert_eq!(polarity_from_name("direct"), Some(Polarity::None));
        assert_eq!(polarity_from_name("  Direct "), Some(Polarity::None));
        assert_eq!(polarity_from_name("ABS"), Some(Polarity::Abs));
        assert_eq!(polarity_from_name("bipolar"), Some(Polarity::Bipolar));
        // A name in neither vocabulary is a miss, not a silent fallback: the
        // caller warns.
        assert_eq!(polarity_from_name("bogus"), Option::None);
        assert_eq!(polarity_from_name(""), Option::None);
        // Every current name round-trips through it.
        for p in Polarity::ALL {
            assert_eq!(polarity_from_name(POLARITY_NAMES[p as usize]), Some(p));
        }
    }

    /// "Direct" is gone from the vocabulary the faceplates read. The label is
    /// what a player sees in the picker; the machine name is what a preset
    /// spells.
    #[test]
    fn the_resting_polarity_is_spelled_none() {
        assert_eq!(POLARITY_NAMES[Polarity::None as usize], "none");
        assert_eq!(POLARITY_LABELS[Polarity::None as usize], "None");
        assert!(!POLARITY_LABELS.contains(&"Direct"));
        assert!(!POLARITY_NAMES.contains(&"direct"));
        // …and the discriminant did not move with it, which is what keeps
        // every saved route meaning what it meant.
        assert_eq!(Polarity::None as u8, 0);
        assert_eq!(curve_code(Polarity::None, Shape::Lin), 0);
        assert_eq!(curve_code(Polarity::Bipolar, Shape::Lin), 3);
    }

    /// The documented divergence from [`f32::clamp`]: a NaN closes the gate
    /// rather than poisoning the dest accumulator for the whole block.
    #[test]
    fn clamp_unit_saturates_and_swallows_nan() {
        assert_eq!(clamp_unit(-1.0), 0.0);
        assert_eq!(clamp_unit(0.5), 0.5);
        assert_eq!(clamp_unit(2.0), 1.0);
        assert_eq!(clamp_unit(f32::NAN), 0.0);
        assert!(f32::NAN.clamp(0.0, 1.0).is_nan());
    }
}
