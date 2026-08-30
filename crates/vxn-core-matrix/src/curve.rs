//! The **curve-shaping vocabulary**: the two axes a route's response
//! decomposes into, the flat code preset files still spell them as, and the
//! scale VCA that folds a secondary source into `[0, 1]`.
//!
//! This is the code that prompted [E049](../../../../epics/open/E049-shared-matrix-routing.md).
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
/// an `ALL` slice in discriminant order, and — for a source enum — the polarity
/// predicate.
///
/// Before this, each of those was written out separately and kept in step by
/// hand: five parallel lists per enum, all indexed by the same `u8`, in each
/// synth. The tests checked their *lengths* and the `from_u8` round-trip, so a
/// transposed name/label pair was invisible until a user read the wrong name in
/// the mod matrix. Generating them from one row list makes that transposition
/// unrepresentable rather than merely untested.
///
/// A row is `Variant = discriminant, "wire-name", "Label"`, plus `uni` / `bi`
/// when the enum declares `polarity`. Doc comments and attributes on a row pass
/// through to the variant, so `#[default]` marks the sentinel exactly as before.
///
/// `fallback` is what `from_u8` returns for an out-of-range byte — the sentinel
/// for source/dest, `Lin` for shapes.
///
/// `#[macro_export]` puts this at the crate root, so consumers reach it as
/// `use vxn_core_matrix::matrix_enum;` rather than through this module.
#[macro_export]
macro_rules! matrix_enum {
    // Entry point: with a polarity column (a source enum).
    (
        $(#[$emeta:meta])*
        $name:ident, fallback = $fallback:ident, names = $names:ident,
        labels = $labels:ident, polarity;
        $(
            $(#[$vmeta:meta])*
            $variant:ident = $disc:literal, $wire:literal, $label:literal, $pol:ident;
        )+
    ) => {
        $crate::matrix_enum! {
            @base
            $(#[$emeta])*
            $name, fallback = $fallback, names = $names, labels = $labels;
            $( $(#[$vmeta])* $variant = $disc, $wire, $label; )+
        }

        impl $name {
            /// Whether this source emits a **bipolar** `[-1, 1]` shape (vs a
            /// unipolar `[0, 1]` one). Consumed by the scale VCA
            /// (`vxn_core_matrix::curve::scale_norm`) to fold a bipolar scale
            /// source into the `[0, 1]` VCA range.
            ///
            /// The `uni` / `bi` column is not optional, so a new source still
            /// forces a polarity decision at compile time — no longer able to
            /// drift from the row it belongs to.
            #[inline]
            pub const fn is_bipolar(self) -> bool {
                match self {
                    $( $name::$variant => $crate::matrix_enum!(@pol $pol) ),+
                }
            }
        }
    };

    // Entry point: no polarity column (dest and axis enums).
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

        impl $name {
            /// Every variant, in discriminant order: `ALL[i] as u8 == i`. That
            /// is the property the name and label tables are indexed on.
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
    /// - `Direct` — passthrough; the source's native polarity reaches the dest.
    /// - `Bipolar` — AC-couple a unipolar `[0, 1]` source to `[-1, 1]` via
    ///   `2v − 1` (centred swing when routing mod-wheel/aftertouch into a
    ///   bipolar dest).
    /// - `Abs` — rectify a bipolar source to `[0, 1]` via `|v|`, so the route is
    ///   strongest at *both* extremes and silent at centre. A voice-position
    ///   source into a pan dest is the motivating case: `direct` pans each voice
    ///   in proportion to its position, `abs` instead moves only the voices at
    ///   the edges of the spread and leaves the centre ones alone. Identity for
    ///   a source already unipolar.
    ///
    ///   Depth sign covers the mirror case, so there is deliberately no
    ///   `1 − |v|` mapping: pull depth negative and the edge voices are driven
    ///   *away* from the destination's own parameter value while the centre
    ///   voices keep it. "More at the centre" falls out of the parameter
    ///   already being the offset such a mapping would re-derive.
    Polarity, fallback = Direct, names = POLARITY_NAMES,
    labels = POLARITY_LABELS;
    #[default]
    Direct = 0, "direct", "Direct";
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

/// Split a flat code back into its `(polarity, shape)` pair. Out-of-range codes
/// degrade to `(Direct, Lin)` rather than aliasing onto a real curve.
#[inline]
pub fn curve_split(code: u8) -> (Polarity, Shape) {
    if code as usize >= N_CURVES {
        return (Polarity::Direct, Shape::Lin);
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
        Polarity::Direct => pol_direct(v),
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

/// Normalise a scale source's value to the `[0, 1]` VCA range, then bend it by
/// `shape`: unipolar sources pass through, bipolar ones map `(v + 1)·0.5`.
///
/// `bipolar` is the scale *source's* own polarity — hence a bare `bool` rather
/// than a source id, which is the one thing this crate deliberately does not
/// know about. Each synth reads it off its own `SourceId` at the call site.
///
/// Clamped **before** the bend, so the bend only ever sees `[0, 1]` and
/// therefore can't leave it — on a non-negative input `Exp` is `v²` and `Log`
/// is `√v`, both fixing 0 and 1 and monotonic between. `0 → the route
/// contributes nothing`, `1 → the route at its full configured depth`,
/// whichever bend is set.
///
/// [`Shape::Lin`] is exact identity, so an unbent VCA is bit-identical to one
/// from before the bend existed.
///
/// A lane loop should **not** call this per lane: both decisions are per-slot
/// constants, so hoist them and expand the six [`fold_unipolar`] ×
/// [`bend_lin`] arms instead. Doing that cut vxn-2's fully-scaled 16-slot eval
/// by ~47%.
#[inline]
pub fn scale_norm(bipolar: bool, v: f32, shape: Shape) -> f32 {
    let n = clamp_unit(if bipolar {
        fold_bipolar(v)
    } else {
        fold_unipolar(v)
    });
    bend_unit(shape, n)
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
        assert_eq!(Polarity::from_u8(200), Polarity::Direct);
        assert_eq!(Shape::from_u8(200), Shape::Lin);
    }

    /// The dispatchers must be exactly the free functions — that equality is
    /// what lets a scalar caller and a hoisted lane loop stay bit-exact.
    #[test]
    fn dispatchers_agree_with_the_arms_bitwise() {
        let vs = [-1.0f32, -0.7, -0.25, 0.0, 0.25, 0.7, 1.0];
        for v in vs {
            assert_eq!(map_polarity(Polarity::Direct, v).to_bits(), pol_direct(v).to_bits());
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

    #[test]
    fn scale_norm_folds_clamps_then_bends() {
        // Unipolar passes through.
        assert_eq!(scale_norm(false, 0.3, Shape::Lin), 0.3);
        assert_eq!(scale_norm(false, 1.0, Shape::Lin), 1.0);
        // Bipolar folds: centre → half, extremes → the gate's ends.
        assert_eq!(scale_norm(true, 0.0, Shape::Lin), 0.5);
        assert_eq!(scale_norm(true, 1.0, Shape::Lin), 1.0);
        assert_eq!(scale_norm(true, -1.0, Shape::Lin), 0.0);
        // Out-of-range input is clamped, not wrapped.
        assert_eq!(scale_norm(false, 1.7, Shape::Lin), 1.0);
        assert_eq!(scale_norm(false, -0.4, Shape::Lin), 0.0);
        // Every bend fixes 0 and 1 and stays inside them.
        for shape in Shape::ALL {
            assert_eq!(scale_norm(false, 0.0, shape), 0.0);
            assert_eq!(scale_norm(false, 1.0, shape), 1.0);
            for v in [-2.0f32, -0.1, 0.0, 0.3, 0.9, 1.0, 3.0] {
                let n = scale_norm(false, v, shape);
                assert!((0.0..=1.0).contains(&n), "{shape:?}/{v} → {n}");
            }
        }
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
