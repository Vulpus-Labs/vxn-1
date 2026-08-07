//! Parameter schema: static descriptors, value kinds, taper math.
//!
//! Per-synth parameter enums (vxn-1's `PatchParam` / `GlobalParam`,
//! vxn-2's 380-param registry) live with each synth; this crate only
//! supplies the shape ([`ParamDesc`]) and the math ([`Taper`]).

#[derive(Clone, Copy, Debug)]
pub enum ParamKind {
    Float { unit: &'static str, taper: Taper },
    Int { unit: &'static str },
    Bool,
    Enum { variants: &'static [&'static str] },
}

/// How a Float param maps across a fader's normalised `[0, 1]` position.
/// `to_normalized` / `from_normalized` stay linear (the host range and
/// any subdivision-index lookup must not warp); `to_fader` / `from_fader`
/// apply the taper.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Taper {
    Linear,
    /// Exponential, pinned so the fader midpoint reads `mid` and the
    /// top reads `max`.
    Exp { mid: f32 },
    /// [`Taper::Exp`]'s curve mirrored about the centre of a symmetric
    /// bipolar range: centre reads 0, and **half travel each way** reads
    /// `±mid`, with the ends at `±max`.
    ///
    /// What it is for: a bipolar control whose musical range is the inner
    /// part of its span (a detune in cents, say — past a certain width the
    /// two things read as out of tune rather than wide). Linear travel puts
    /// that range in a few pixels either side of centre; this puts it across
    /// most of the slider while keeping the extremes reachable.
    ///
    /// `mid` must be **strictly less than `max/2`**, or the pinning has no
    /// solution (see [`ParamDesc::bipolar_exp_coeffs`]) — such a descriptor
    /// falls back to linear rather than emitting NaN into a fader.
    BipolarExp { mid: f32 },
}

#[derive(Clone, Copy, Debug)]
pub struct ParamDesc {
    pub name: &'static str,
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub kind: ParamKind,
}

impl ParamDesc {
    #[inline]
    pub fn clamp(&self, v: f32) -> f32 {
        v.clamp(self.min, self.max)
    }

    /// Linear position in `[0, 1]` — what CLAP's `param_value_to_normalized`
    /// returns. Taper is NOT applied here.
    #[inline]
    pub fn to_normalized(&self, v: f32) -> f32 {
        if self.max > self.min {
            ((v - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    #[inline]
    pub fn from_normalized(&self, n: f32) -> f32 {
        self.min + n.clamp(0.0, 1.0) * (self.max - self.min)
    }

    /// Resolve an enum **variant label** to its index (case-insensitive).
    pub fn variant_index(&self, label: &str) -> Option<usize> {
        match self.kind {
            ParamKind::Enum { variants } => {
                variants.iter().position(|v| v.eq_ignore_ascii_case(label))
            }
            _ => None,
        }
    }

    #[inline]
    pub fn taper(&self) -> Taper {
        match self.kind {
            ParamKind::Float { taper, .. } => taper,
            _ => Taper::Linear,
        }
    }

    /// Coefficients of the [`Taper::BipolarExp`] magnitude curve
    /// `|v| = a·(exp(k·t) − 1)`, pinned at `t = 0 → 0`, `t = 0.5 → mid` and
    /// `t = 1 → max` (`t` = travel from centre, `[0, 1]`).
    ///
    /// `None` when the pinning has no solution: `r = max/mid − 1` must exceed
    /// 1 — i.e. `mid < max/2` — or `a` divides by zero (at `mid == max/2`) or
    /// goes negative (above it), and `k = 2·ln r` stops being a rise. A
    /// descriptor in that state falls back to linear, which is wrong-feeling
    /// but finite; the alternative is NaN in a fader position.
    #[inline]
    fn bipolar_exp_coeffs(&self, mid: f32) -> Option<(f32, f32, f32)> {
        let max = self.max;
        if !(mid > 0.0 && max > 0.0 && mid < max * 0.5) {
            return None;
        }
        let r = max / mid - 1.0;
        Some((mid / (r - 1.0), 2.0 * r.ln(), max))
    }

    /// Apply the descriptor's taper to map value → fader position `[0, 1]`.
    /// Used by editors and value-text formatting.
    #[inline]
    pub fn to_fader(&self, value: f32) -> f32 {
        if let Taper::BipolarExp { mid } = self.taper() {
            let Some((a, k, max)) = self.bipolar_exp_coeffs(mid) else {
                return self.to_normalized(value);
            };
            let v = value.clamp(-max, max);
            // Invert |v| = a·(exp(k·t) − 1) for the travel from centre, then
            // place it either side of 0.5. The sign of `v` picks the side, so
            // 0 lands exactly on centre from both directions.
            let t = ((v.abs() / a + 1.0).ln() / k).clamp(0.0, 1.0);
            return if v < 0.0 { 0.5 - 0.5 * t } else { 0.5 + 0.5 * t };
        }
        let Taper::Exp { mid } = self.taper() else {
            return self.to_normalized(value);
        };
        if !(self.min > 0.0 && mid > self.min && self.max > mid) {
            // min == 0 (or degenerate): single exponential pinned at
            // (0, 0), (0.5, mid), (1, max). Preserves the shape for
            // params whose floor is genuinely zero.
            let r = self.max / mid - 1.0;
            let a = mid / (r - 1.0);
            let k = 2.0 * r.ln();
            return ((value / a + 1.0).ln() / k).clamp(0.0, 1.0);
        }
        let v = value.clamp(self.min, self.max);
        if v <= mid {
            0.5 * (v / self.min).ln() / (mid / self.min).ln()
        } else {
            0.5 + 0.5 * (v / mid).ln() / (self.max / mid).ln()
        }
    }

    /// Inverse of [`Self::to_fader`].
    #[inline]
    pub fn from_fader(&self, n: f32) -> f32 {
        if let Taper::BipolarExp { mid } = self.taper() {
            let Some((a, k, max)) = self.bipolar_exp_coeffs(mid) else {
                return self.from_normalized(n);
            };
            let n = n.clamp(0.0, 1.0);
            let t = (2.0 * n - 1.0).abs();
            let mag = (a * ((k * t).exp() - 1.0)).min(max);
            return if n < 0.5 { -mag } else { mag };
        }
        let Taper::Exp { mid } = self.taper() else {
            return self.from_normalized(n);
        };
        let n = n.clamp(0.0, 1.0);
        if !(self.min > 0.0 && mid > self.min && self.max > mid) {
            let r = self.max / mid - 1.0;
            let a = mid / (r - 1.0);
            let k = 2.0 * r.ln();
            return a * ((k * n).exp() - 1.0);
        }
        if n <= 0.5 {
            self.min * (mid / self.min).powf(2.0 * n)
        } else {
            mid * (self.max / mid).powf(2.0 * n - 1.0)
        }
    }

    /// Parse host type-in text (`param_text_to_value`) back to a plain value.
    ///
    /// Routes through the descriptor's kind rather than blindly parsing a
    /// leading number: an enum/bool param accepts its **variant label**
    /// (case-insensitive, e.g. "Saw", "On") as well as a numeric index, and a
    /// float/int clamps the parsed number to range. Returns `None` when the
    /// text matches neither a label nor a leading number.
    pub fn parse(&self, text: &str) -> Option<f32> {
        let t = text.trim();
        match self.kind {
            ParamKind::Enum { .. } => match self.variant_index(t) {
                Some(i) => Some(i as f32),
                None => leading_number(t).map(|v| self.clamp(v)),
            },
            ParamKind::Bool => {
                if t.eq_ignore_ascii_case("on") || t.eq_ignore_ascii_case("true") {
                    Some(1.0)
                } else if t.eq_ignore_ascii_case("off") || t.eq_ignore_ascii_case("false") {
                    Some(0.0)
                } else {
                    leading_number(t).map(|v| if v >= 0.5 { 1.0 } else { 0.0 })
                }
            }
            ParamKind::Int { .. } | ParamKind::Float { .. } => {
                leading_number(t).map(|v| self.clamp(v))
            }
        }
    }

    /// Format `value` for display (host's `param_value_to_text`).
    pub fn display(&self, value: f32) -> String {
        match self.kind {
            ParamKind::Enum { variants } => {
                let i = (value.round() as usize).min(variants.len().saturating_sub(1));
                variants[i].to_string()
            }
            ParamKind::Bool => if value >= 0.5 { "On" } else { "Off" }.to_string(),
            ParamKind::Int { unit } => format!("{} {}", value.round() as i64, unit),
            ParamKind::Float { unit, .. } => {
                if unit.is_empty() {
                    format!("{value:.3}")
                } else {
                    format!("{value:.2} {unit}")
                }
            }
        }
    }
}

/// Parse the leading numeric run of `s` (digits, `.`, leading `-`), ignoring
/// any trailing unit suffix. `None` if there's no number at the front.
fn leading_number(s: &str) -> Option<f32> {
    let num: String = s
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    num.parse::<f32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn float(min: f32, max: f32, taper: Taper) -> ParamDesc {
        ParamDesc {
            name: "t",
            label: "t",
            min,
            max,
            default: min,
            kind: ParamKind::Float { unit: "", taper },
        }
    }

    #[test]
    fn linear_taper_roundtrips() {
        let d = float(0.0, 10.0, Taper::Linear);
        for v in [0.0_f32, 1.0, 5.0, 9.99] {
            let n = d.to_fader(v);
            let back = d.from_fader(n);
            assert!((back - v).abs() < 1e-5, "{} -> {} -> {}", v, n, back);
        }
    }

    #[test]
    fn exp_taper_midpoint_reads_mid() {
        let d = float(20.0, 20_000.0, Taper::Exp { mid: 1_000.0 });
        let n = d.to_fader(1_000.0);
        assert!((n - 0.5).abs() < 1e-4, "n = {}", n);
        let v = d.from_fader(0.5);
        assert!((v - 1_000.0).abs() < 1e-3, "v = {}", v);
    }

    #[test]
    fn exp_taper_top_reads_max() {
        let d = float(20.0, 20_000.0, Taper::Exp { mid: 1_000.0 });
        assert!((d.to_fader(20_000.0) - 1.0).abs() < 1e-4);
        assert!((d.from_fader(1.0) - 20_000.0).abs() < 1e-2);
    }

    #[test]
    fn exp_taper_with_zero_floor_pins_origin() {
        // min == 0: single-exp shape pinned at (0, 0), (0.5, mid), (1, max).
        let d = float(0.0, 100.0, Taper::Exp { mid: 25.0 });
        assert!((d.to_fader(0.0) - 0.0).abs() < 1e-5);
        assert!((d.to_fader(25.0) - 0.5).abs() < 1e-3);
        assert!((d.to_fader(100.0) - 1.0).abs() < 1e-3);
    }

    // ── BipolarExp (0263) ───────────────────────────────────────────────────

    /// The calibration the taper exists for: centre is 0, **half travel each
    /// way** is `±mid`, the ends are `±max`.
    #[test]
    fn bipolar_exp_pins_centre_half_travel_and_ends() {
        let d = float(-50.0, 50.0, Taper::BipolarExp { mid: 20.0 });
        assert_eq!(d.from_fader(0.5), 0.0, "centre must be exactly zero");
        assert!((d.from_fader(0.75) - 20.0).abs() < 1e-3, "{}", d.from_fader(0.75));
        assert!((d.from_fader(0.25) + 20.0).abs() < 1e-3, "{}", d.from_fader(0.25));
        assert!((d.from_fader(1.0) - 50.0).abs() < 1e-3, "{}", d.from_fader(1.0));
        assert!((d.from_fader(0.0) + 50.0).abs() < 1e-3, "{}", d.from_fader(0.0));
    }

    #[test]
    fn bipolar_exp_round_trips_including_the_endpoints() {
        let d = float(-50.0, 50.0, Taper::BipolarExp { mid: 20.0 });
        for v in [-50.0_f32, -37.5, -20.0, -3.0, 0.0, 3.0, 20.0, 37.5, 50.0] {
            let back = d.from_fader(d.to_fader(v));
            assert!((back - v).abs() < 1e-3, "{v} -> {} -> {back}", d.to_fader(v));
        }
    }

    /// The curve is a rise, and symmetric: the magnitude at `0.5 + t` matches
    /// the magnitude at `0.5 − t`, and more travel always means more value.
    #[test]
    fn bipolar_exp_is_symmetric_and_monotonic() {
        let d = float(-50.0, 50.0, Taper::BipolarExp { mid: 20.0 });
        for i in 0..=10 {
            let t = i as f32 / 20.0; // 0 .. 0.5
            let up = d.from_fader(0.5 + t);
            let down = d.from_fader(0.5 - t);
            assert!((up + down).abs() < 1e-4, "asymmetric at t={t}: {up} vs {down}");
        }
        let mut prev = f32::NEG_INFINITY;
        for i in 0..=100 {
            let v = d.from_fader(i as f32 / 100.0);
            assert!(v >= prev - 1e-4, "not monotonic at {i}: {v} after {prev}");
            prev = v;
        }
    }

    /// `mid` must be under half of `max` or the pinning has no solution
    /// (`a = mid/(r−1)` divides by zero at exactly `max/2`). Such a descriptor
    /// degrades to linear rather than putting NaN into a fader position.
    #[test]
    fn bipolar_exp_degenerate_mid_falls_back_to_linear() {
        for mid in [25.0_f32, 30.0, 50.0, 0.0, -5.0] {
            let d = float(-50.0, 50.0, Taper::BipolarExp { mid });
            let lin = float(-50.0, 50.0, Taper::Linear);
            for n in [0.0_f32, 0.25, 0.5, 0.75, 1.0] {
                let v = d.from_fader(n);
                assert!(v.is_finite(), "mid={mid} n={n} produced {v}");
                assert_eq!(v, lin.from_fader(n), "mid={mid} must fall back to linear");
            }
            for v in [-50.0_f32, 0.0, 12.5, 50.0] {
                assert!(d.to_fader(v).is_finite(), "mid={mid} v={v} produced NaN");
                assert_eq!(d.to_fader(v), lin.to_fader(v));
            }
        }
    }

    /// Taper is an editor-side mapping only: the host's normalised range and
    /// the preset/state formats stay linear.
    #[test]
    fn bipolar_exp_leaves_the_linear_normalised_pair_alone() {
        let d = float(-50.0, 50.0, Taper::BipolarExp { mid: 20.0 });
        assert!((d.to_normalized(0.0) - 0.5).abs() < 1e-6);
        assert!((d.from_normalized(0.75) - 25.0).abs() < 1e-4);
        // ... and the tapered pair genuinely differs from it, or the taper
        // would be doing nothing.
        assert!((d.from_fader(0.75) - d.from_normalized(0.75)).abs() > 1.0);
    }

    #[test]
    fn enum_display_round_trips() {
        let d = ParamDesc {
            name: "wave",
            label: "wave",
            min: 0.0,
            max: 3.0,
            default: 0.0,
            kind: ParamKind::Enum { variants: &["Sine", "Tri", "Saw", "Pulse"] },
        };
        assert_eq!(d.display(2.0), "Saw");
        assert_eq!(d.variant_index("saw"), Some(2));
    }

    #[test]
    fn parse_enum_label_and_out_of_range_float() {
        let e = ParamDesc {
            name: "wave",
            label: "wave",
            min: 0.0,
            max: 3.0,
            default: 0.0,
            kind: ParamKind::Enum { variants: &["Sine", "Tri", "Saw", "Pulse"] },
        };
        // Enum label (case-insensitive) → its index, not a leading-number parse.
        assert_eq!(e.parse("saw"), Some(2.0));
        assert_eq!(e.parse(" Pulse "), Some(3.0));
        // A bare index still parses, clamped to range.
        assert_eq!(e.parse("1"), Some(1.0));
        assert_eq!(e.parse("99"), Some(3.0));
        // Nonsense → None.
        assert_eq!(e.parse("banjo"), None);

        // Float type-in clamps to the descriptor range.
        let f = float(20.0, 20_000.0, Taper::Linear);
        assert_eq!(f.parse("440 Hz"), Some(440.0));
        assert_eq!(f.parse("50000"), Some(20_000.0));
        assert_eq!(f.parse("-5"), Some(20.0));
        assert_eq!(f.parse("nope"), None);
    }

    #[test]
    fn parse_bool_accepts_labels_and_numbers() {
        let b = ParamDesc {
            name: "sync",
            label: "sync",
            min: 0.0,
            max: 1.0,
            default: 0.0,
            kind: ParamKind::Bool,
        };
        assert_eq!(b.parse("On"), Some(1.0));
        assert_eq!(b.parse("off"), Some(0.0));
        assert_eq!(b.parse("1"), Some(1.0));
        assert_eq!(b.parse("0"), Some(0.0));
    }

    #[test]
    fn clamp_respects_bounds() {
        let d = float(-1.0, 1.0, Taper::Linear);
        assert_eq!(d.clamp(-2.0), -1.0);
        assert_eq!(d.clamp(2.0), 1.0);
        assert_eq!(d.clamp(0.5), 0.5);
    }
}
