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

    /// Apply the descriptor's taper to map value → fader position `[0, 1]`.
    /// Used by editors and value-text formatting.
    #[inline]
    pub fn to_fader(&self, value: f32) -> f32 {
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
    fn clamp_respects_bounds() {
        let d = float(-1.0, 1.0, Taper::Linear);
        assert_eq!(d.clamp(-2.0), -1.0);
        assert_eq!(d.clamp(2.0), 1.0);
        assert_eq!(d.clamp(0.5), 0.5);
    }
}
