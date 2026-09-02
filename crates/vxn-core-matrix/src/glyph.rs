//! **Curve glyphs**: each `(polarity, shape)` pair as an SVG polyline, plotted
//! from this crate's own arithmetic.
//!
//! The faceplates draw a route's shaping as a picture rather than as two words
//! (ticket 0340) — a small button per curve, opening a 3×3 picker. The picture
//! only earns that job if it cannot disagree with the sound, so the points come
//! from [`shape`](crate::curve::shape) itself rather than from a JavaScript
//! re-implementation of it. A renderer that re-derived `sign(v)·√|v|` in the
//! page would be a second spelling of the arithmetic, which is the exact
//! duplication [`crate::curve`] exists to have deleted.
//!
//! ## What crosses the bridge
//!
//! A `points` string and a band rectangle per curve — geometry, not vocabulary.
//! The names and labels a picker shows are already
//! [`CURVE_NAMES`](crate::curve::CURVE_NAMES) /
//! [`CURVE_LABELS`](crate::curve::CURVE_LABELS), which elide the resting
//! polarity exactly the way a picker wants to (`Lin`, not `None Lin`), so this
//! module deliberately grows no label table of its own.
//!
//! ## Coordinates
//!
//! A `0 0 100 100` viewBox with `v` running left to right over `[-1, 1]` and
//! `y` running bottom to top over the same, so `(50, 50)` is the origin and the
//! identity is the box's leading diagonal. `preserveAspectRatio="none"` lets one
//! set of points serve both the 38×22 row button and the 82×62 picker cell.
//!
//! The **band** is the source range a polarity is written for, not a clamp:
//! `Bipolar` expects a unipolar `[0, 1]` source and maps it to the full swing,
//! so its band covers the right half. Feed it a bipolar source anyway and the
//! formula still applies — it just spends half its travel below the box. The
//! picker shades the band to say which source a mapping is *for*; the curve is
//! drawn only across it, because that is the part a player is choosing.

use crate::curve::{N_CURVES, N_SHAPES, Polarity, Shape, curve_split, shape};

/// Points per polyline. Enough that `Log`'s near-vertical approach to the
/// origin reads as a curve rather than a corner at picker size (82×62 px), and
/// few enough that all nine inline into a faceplate descriptor in under 10 KB.
const SAMPLES: usize = 97;

/// One curve's drawable geometry. Pairs with `CURVE_NAMES[code]` /
/// `CURVE_LABELS[code]` for the text a picker shows.
#[derive(Clone, Debug, PartialEq)]
pub struct CurveGlyph {
    /// The flat `(polarity, shape)` code this draws — see
    /// [`curve_code`](crate::curve::curve_code).
    pub code: u8,
    /// SVG `points` for a `<polyline>`: `"x,y x,y …"` in the viewBox above,
    /// spanning only [`Self::band_x`]`..`[`Self::band_x`]` + `[`Self::band_w`].
    pub points: String,
    /// Left edge of the source range this polarity is written for, in viewBox
    /// units.
    pub band_x: f32,
    /// Width of that range.
    pub band_w: f32,
}

/// The source range a polarity is written for, as `(lo, hi)` in `v`.
///
/// `Bipolar` is the only one that narrows: it AC-couples a **unipolar** source,
/// so `[0, 1]` in is the whole of its intended travel. The other two take a
/// source of either polarity — `None` passes whatever arrives through, and
/// `Abs` is the identity on a unipolar source and a fold on a bipolar one.
#[inline]
fn native_range(polarity: Polarity) -> (f32, f32) {
    match polarity {
        Polarity::Bipolar => (0.0, 1.0),
        Polarity::None | Polarity::Abs => (-1.0, 1.0),
    }
}

/// `v` → viewBox x.
#[inline]
fn vx(v: f32) -> f32 {
    (v + 1.0) * 50.0
}

/// A shaped value → viewBox y. Inverted: SVG y grows downward.
#[inline]
fn vy(y: f32) -> f32 {
    (1.0 - y) * 50.0
}

/// Every curve's geometry, indexed by flat code — the whole 3×3, in
/// [`curve_code`](crate::curve::curve_code) order.
///
/// Allocates, so a caller builds this **once** into its page descriptor rather
/// than per repaint. Nothing here is on an audio path.
pub fn curve_glyphs() -> Vec<CurveGlyph> {
    (0..N_CURVES as u8).map(curve_glyph).collect()
}

/// One curve's geometry. An out-of-range code degrades through
/// [`curve_split`], the same way every other decode of a flat code does, so a
/// corrupt row draws the plain curve rather than nothing.
pub fn curve_glyph(code: u8) -> CurveGlyph {
    let (polarity, bend) = curve_split(code);
    let (lo, hi) = native_range(polarity);
    let mut points = String::with_capacity(SAMPLES * 12);
    for i in 0..SAMPLES {
        let t = i as f32 / (SAMPLES - 1) as f32;
        let v = lo + (hi - lo) * t;
        if i > 0 {
            points.push(' ');
        }
        // The shipped arithmetic, not a copy of it: polarity then bend, in the
        // order `shape` fixes.
        points.push_str(&format!("{:.1},{:.1}", vx(v), vy(shape(polarity, bend, v))));
    }
    CurveGlyph {
        code,
        points,
        band_x: vx(lo),
        band_w: vx(hi) - vx(lo),
    }
}

/// Display order for the picker's **rows**: `None`, `Abs`, `Bipolar` — the
/// resting mapping first, then the two that reshape it.
///
/// Display order only. The discriminants are pinned by
/// [`curve_code`](crate::curve::curve_code)'s `polarity · N_SHAPES + shape`
/// stride, which is what keeps the four pre-split preset spellings (`0 = lin`,
/// `1 = exp`, `2 = log`, `3 = bipolar`) meaning what they always meant in both
/// synths. Renumbering the enum to get this order would silently remap every
/// saved route; a three-element table costs nothing and cannot.
pub const POLARITY_ROWS: [Polarity; 3] = [Polarity::None, Polarity::Abs, Polarity::Bipolar];

/// Flat codes in picker layout order — [`POLARITY_ROWS`] down, [`Shape::ALL`]
/// across. Spelled once so a panel lays the grid out without re-deriving the
/// stride.
pub fn picker_codes() -> Vec<u8> {
    POLARITY_ROWS
        .iter()
        .flat_map(|p| Shape::ALL.map(|s| *p as u8 * N_SHAPES as u8 + s as u8))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::{CURVE_LABELS, CURVE_NAMES, curve_code};

    /// Parse a `points` string back into `(x, y)` pairs.
    fn pts(g: &CurveGlyph) -> Vec<(f32, f32)> {
        g.points
            .split(' ')
            .map(|p| {
                let (x, y) = p.split_once(',').expect("an x,y pair");
                (x.parse().unwrap(), y.parse().unwrap())
            })
            .collect()
    }

    #[test]
    fn every_curve_has_a_glyph_in_code_order() {
        let gs = curve_glyphs();
        assert_eq!(gs.len(), N_CURVES);
        for (i, g) in gs.iter().enumerate() {
            assert_eq!(g.code as usize, i);
            assert_eq!(pts(g).len(), SAMPLES);
        }
    }

    /// The whole point of the module: the drawn `y` is what the evaluator would
    /// compute for that `v`, to the precision the string carries. A JS
    /// re-implementation is what this asserts the absence of.
    #[test]
    fn points_are_the_shipped_arithmetic() {
        for code in 0..N_CURVES as u8 {
            let (polarity, bend) = curve_split(code);
            let (lo, hi) = native_range(polarity);
            let g = curve_glyph(code);
            for (i, (x, y)) in pts(&g).into_iter().enumerate() {
                // Re-derive `v` from the sample index, not from the rounded
                // `x`: at one decimal place an `x` recovers a `v` off by up to
                // 0.002, and `Exp`'s slope turns that into a `y` off by more
                // than the rounding this is trying to measure. The index is
                // what the generator itself walks.
                let v = lo + (hi - lo) * i as f32 / (SAMPLES - 1) as f32;
                // Compare through the *same* rounding the generator writes
                // with, so this is an equality rather than an epsilon: the
                // question is whether the string carries the arithmetic's
                // number, not whether it is close to it. An epsilon here would
                // have to be wider than half a tick to survive `f32`, at which
                // point it stops distinguishing "rounded" from "wrong".
                let at1dp = |f: f32| format!("{f:.1}").parse::<f32>().unwrap();
                assert_eq!(x, at1dp(vx(v)), "code {code}: x at sample {i}, v={v}");
                assert_eq!(
                    y,
                    at1dp(vy(shape(polarity, bend, v))),
                    "code {code}: at v={v} drew y={y}, arithmetic says {}",
                    vy(shape(polarity, bend, v))
                );
            }
        }
    }

    /// `Bipolar` is the one polarity written for a unipolar source, so its band
    /// is the right half of the box and its curve is drawn only there. The
    /// other two span the full width.
    #[test]
    fn the_band_is_the_range_the_polarity_is_written_for() {
        for shape in Shape::ALL {
            let bi = curve_glyph(curve_code(Polarity::Bipolar, shape));
            assert_eq!((bi.band_x, bi.band_w), (50.0, 50.0));
            assert_eq!(pts(&bi).first().unwrap().0, 50.0);
            assert_eq!(pts(&bi).last().unwrap().0, 100.0);

            for polarity in [Polarity::None, Polarity::Abs] {
                let g = curve_glyph(curve_code(polarity, shape));
                assert_eq!((g.band_x, g.band_w), (0.0, 100.0));
                assert_eq!(pts(&g).first().unwrap().0, 0.0);
                assert_eq!(pts(&g).last().unwrap().0, 100.0);
            }
        }
    }

    /// Every glyph stays inside the box over its own band — which is what lets
    /// the row button drop the frame entirely and still read.
    #[test]
    fn glyphs_stay_inside_the_view_box() {
        for code in 0..N_CURVES as u8 {
            for (x, y) in pts(&curve_glyph(code)) {
                assert!((0.0..=100.0).contains(&x), "code {code}: x={x}");
                assert!((0.0..=100.0).contains(&y), "code {code}: y={y}");
            }
        }
    }

    /// `None · Lin` is the identity — the box's leading diagonal — and `Abs ·
    /// Lin` is the V it rectifies to. Two hand-checkable anchors, so a
    /// coordinate-convention slip (a flipped y, a half-scale x) fails here
    /// rather than merely looking odd.
    #[test]
    fn the_two_hand_checkable_glyphs_are_right() {
        let ident = curve_glyph(curve_code(Polarity::None, Shape::Lin));
        let p = pts(&ident);
        assert_eq!(p.first().copied(), Some((0.0, 100.0)));
        assert_eq!(p.last().copied(), Some((100.0, 0.0)));

        // `Abs · Lin` is the V that rectification makes: full height at both
        // ends, down to the zero line — the box's vertical centre, since `y`
        // spans `[-1, 1]` like `v` — at the middle.
        let abs = curve_glyph(curve_code(Polarity::Abs, Shape::Lin));
        let p = pts(&abs);
        assert_eq!(p.first().copied(), Some((0.0, 0.0)));
        assert_eq!(p.last().copied(), Some((100.0, 0.0)));
        assert_eq!(p[SAMPLES / 2], (50.0, 50.0));
    }

    /// The picker lays out `None / Abs / Bipolar` down and `Lin / Exp / Log`
    /// across, **without** the enum's discriminants moving — the codes it emits
    /// are still `polarity · 3 + shape`.
    #[test]
    fn picker_order_is_display_only() {
        assert_eq!(
            POLARITY_ROWS.map(|p| p as u8),
            [Polarity::None as u8, Polarity::Abs as u8, Polarity::Bipolar as u8]
        );
        assert_eq!(picker_codes(), vec![0, 1, 2, 6, 7, 8, 3, 4, 5]);
        // Every code appears exactly once, so no combination is unreachable
        // from the grid.
        let mut seen = picker_codes();
        seen.sort_unstable();
        assert_eq!(seen, (0..N_CURVES as u8).collect::<Vec<_>>());
        // The legacy spellings still sit where they always did.
        assert_eq!(CURVE_NAMES[0], "lin");
        assert_eq!(CURVE_NAMES[3], "bipolar");
        assert_eq!(CURVE_LABELS[6], "Abs");
    }

    /// An out-of-range code degrades to the plain curve rather than panicking
    /// or drawing nothing — the same contract `curve_split` has, because it
    /// *is* `curve_split`.
    #[test]
    fn an_out_of_range_code_draws_the_plain_curve() {
        let plain = curve_glyph(curve_code(Polarity::None, Shape::Lin));
        for bad in [N_CURVES as u8, 200, 255] {
            let g = curve_glyph(bad);
            assert_eq!(g.points, plain.points, "code {bad}");
            assert_eq!(g.code, bad, "the code is reported as given");
        }
    }
}
