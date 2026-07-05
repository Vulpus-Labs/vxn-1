//! Flavour runtime — the mechanism at the heart of the voice-roster epic (ADR 0005,
//! ticket 0180).
//!
//! A **family** (an engine) has a full parameter space `P`, each param carrying
//! [`ParamMeta`] (name / unit / range / default / curve). A **flavour** is a named
//! point in that space: a [`Flavour::base`] vector, a macro-**binding** table, and
//! the macro values it ships with. Evaluation is **additive-from-base, per trig**
//! (not per sample):
//!
//! ```text
//! final(p) = clamp( base[p] + Σ_{b: b.param==p} b.curve(macro[b.slot]) · b.depth , range(p) )
//! ```
//!
//! [`resolve`] computes the whole param vector into a caller-owned scratch buffer —
//! **allocation-free**, so it runs on the audio thread when a voice triggers; the
//! per-sample SoA kernels then consume the resolved values unchanged. A flavour is
//! **data**: authored as a small record and serialised as the per-track deep patch
//! (0179 fills the reserved `clap.state` bytes with exactly these bytes).
//!
//! This is a *deliberately constrained* modulation matrix — one source type (a macro
//! knob), destination = any family param, additive depth — not the vxn-2 general
//! matrix. Small on purpose (ADR 0005).

use crate::patch::PatchReader;
use crate::track_engine::{MACRO_SLOTS, MacroUnit};

/// Byte-layout version for a serialised [`Flavour`]. Bump when the layout changes;
/// coordinated with the per-engine patch version (0179) — a flavour *is* the patch.
const FLAVOUR_VERSION: u8 = 2;

/// Response curve for a macro binding. Minimal set (0180): linear + one exponential.
/// Widen behind this enum in the flavour editor (0185) without a format break — the
/// tag is a `u8`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Curve {
    Linear,
    /// Square law — a simple ease-in (slow near 0, fast near 1).
    Exp,
}

impl Curve {
    /// Map a normalised macro value `0..1` through the curve (clamped).
    #[inline]
    pub fn apply(self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Curve::Linear => x,
            Curve::Exp => x * x,
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            Curve::Linear => 0,
            Curve::Exp => 1,
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            1 => Curve::Exp,
            _ => Curve::Linear,
        }
    }
}

/// Static metadata for one family parameter (ADR 0005 §Family). Pure **data**,
/// queryable on the main thread by the flavour editor (0185) and value-text (0172);
/// never read inside the per-sample kernel.
#[derive(Copy, Clone, Debug)]
pub struct ParamMeta {
    /// Display name (also the value-text label).
    pub name: &'static str,
    /// Physical unit, for formatting + parsing.
    pub unit: MacroUnit,
    /// Inclusive value range — [`resolve`] clamps to it.
    pub min: f32,
    pub max: f32,
    /// The value an unbound param takes in a fresh flavour's base vector.
    pub default: f32,
}

/// One macro binding: macro `slot` drives family `param` additively, its normalised
/// value scaled by `depth` through `curve`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Binding {
    pub slot: u8,
    pub param: u8,
    pub curve: Curve,
    pub depth: f32,
}

/// A named point in a family's parameter space: a fixed `base` vector (one value per
/// family param), a macro-binding table, and the macro values the flavour ships with.
/// `base.len()` equals the family's param count `P`. Serialised as the per-track deep
/// patch (0179).
#[derive(Clone, Debug, PartialEq)]
pub struct Flavour {
    pub base: Vec<f32>,
    pub bindings: Vec<Binding>,
    pub macro_defaults: [f32; MACRO_SLOTS],
    /// Per-slot user macro **name** ("" = derive from the first bound param). Editable
    /// on the faceplate (0185); shown by `value_to_text`. Not read by the audio path.
    pub macro_names: [String; MACRO_SLOTS],
}

impl Flavour {
    /// A binding-free flavour whose base is each param's `default` (a family's neutral
    /// starting point before any authoring).
    pub fn defaults_for(meta: &[ParamMeta]) -> Self {
        Self {
            base: meta.iter().map(|m| m.default).collect(),
            bindings: Vec::new(),
            macro_defaults: [0.5; MACRO_SLOTS],
            macro_names: Default::default(),
        }
    }

    /// Append the explicit LE byte layout (version-tagged). Mirrors the outer state
    /// blob's field-explicit discipline — these bytes are the 0179 deep patch.
    ///
    /// ```text
    /// version        : u8  (= FLAVOUR_VERSION)
    /// n_params       : u8  (= base.len() = family P)
    /// base           : f32 LE × n_params
    /// n_bindings     : u8
    /// bindings       : { slot u8 ; param u8 ; curve u8 ; depth f32 LE } × n_bindings
    /// macro_defaults : f32 LE × MACRO_SLOTS
    /// macro_names    : { len u8 ; utf8 bytes } × MACRO_SLOTS   (v2+, 0185)
    /// ```
    pub fn serialize(&self, out: &mut Vec<u8>) {
        out.push(FLAVOUR_VERSION);
        out.push(self.base.len() as u8);
        for v in &self.base {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.push(self.bindings.len() as u8);
        for b in &self.bindings {
            out.push(b.slot);
            out.push(b.param);
            out.push(b.curve.as_u8());
            out.extend_from_slice(&b.depth.to_le_bytes());
        }
        for v in &self.macro_defaults {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for nm in &self.macro_names {
            let b = nm.as_bytes();
            let len = b.len().min(255);
            out.push(len as u8);
            out.extend_from_slice(&b[..len]);
        }
    }

    /// Parse a flavour previously written by [`serialize`], for a family whose param
    /// count is `p`. Three outcomes, mirroring the 0179 deep-patch contract:
    ///
    /// `Ok(Some)` = parsed; `Ok(None)` = version or `n_params` mismatch (keep the
    /// default flavour, don't fail the whole state load); `Err(())` = **truncated**
    /// within a known version (rejected). A **v1** blob (no macro names) parses with
    /// empty names.
    #[allow(clippy::result_unit_err)] // parse-failure sentinel; mirrors the state reader
    pub fn deserialize(bytes: &[u8], p: usize) -> Result<Option<Flavour>, ()> {
        let mut r = PatchReader::new(bytes);
        let ver = r.u8()?;
        if ver != 1 && ver != FLAVOUR_VERSION {
            return Ok(None); // newer/unknown layout → keep default
        }
        let n = r.u8()? as usize;
        if n != p {
            return Ok(None); // shape mismatch (e.g. a family whose P changed) → keep default
        }
        let mut base = Vec::with_capacity(n);
        for _ in 0..n {
            base.push(r.f32()?);
        }
        let nb = r.u8()? as usize;
        let mut bindings = Vec::with_capacity(nb);
        for _ in 0..nb {
            let slot = r.u8()?;
            let param = r.u8()?;
            let curve = Curve::from_u8(r.u8()?);
            let depth = r.f32()?;
            bindings.push(Binding { slot, param, curve, depth });
        }
        let mut macro_defaults = [0.0; MACRO_SLOTS];
        for m in macro_defaults.iter_mut() {
            *m = r.f32()?;
        }
        let mut macro_names: [String; MACRO_SLOTS] = Default::default();
        if ver >= 2 {
            for nm in macro_names.iter_mut() {
                let len = r.u8()? as usize;
                *nm = String::from_utf8_lossy(r.take(len)?).into_owned();
            }
        }
        Ok(Some(Flavour { base, bindings, macro_defaults, macro_names }))
    }

    /// The display label for a macro slot: the user override, else the first bound
    /// param's name, else `None` (an unbound, unnamed slot).
    pub fn macro_label<'a>(&'a self, meta: &'a [ParamMeta], slot: usize) -> Option<&'a str> {
        if let Some(nm) = self.macro_names.get(slot) {
            if !nm.is_empty() {
                return Some(nm.as_str());
            }
        }
        self.bindings
            .iter()
            .find(|b| b.slot as usize == slot)
            .and_then(|b| meta.get(b.param as usize))
            .map(|m| m.name)
    }
}

/// Resolve a flavour to its per-trig param vector: additive-from-base, clamped to
/// each param's range. **Allocation-free** — writes into caller-owned `out` (len `P`).
/// Called at a voice's trig, when the macros + flavour are stable; the per-sample
/// kernel consumes `out` unchanged.
///
/// `O(P · bindings)` with tiny constants (P and the binding table are both small);
/// no per-param binding index is needed.
#[inline]
pub fn resolve(meta: &[ParamMeta], base: &[f32], bindings: &[Binding], macros: &[f32], out: &mut [f32]) {
    for (p, slot) in out.iter_mut().enumerate().take(meta.len().min(base.len())) {
        let mut v = base[p];
        for b in bindings {
            if b.param as usize == p {
                let m = macros.get(b.slot as usize).copied().unwrap_or(0.0);
                v += b.curve.apply(m) * b.depth;
            }
        }
        *slot = v.clamp(meta[p].min, meta[p].max);
    }
}

/// Flavour-aware macro readout (ADR 0005 §value_to_text; 0172 becomes flavour-aware):
/// a macro slot's text reflects the **param the current flavour bound it to** and that
/// param's resolved physical value, rather than a fixed per-engine map. Renders "—"
/// for an unbound slot. Pure — no engine instance — so it stays callable on the main
/// thread. Shows the first binding for the slot (the primary target).
pub fn flavour_macro_display(
    meta: &[ParamMeta],
    flavour: &Flavour,
    slot: usize,
    norm: f32,
    out: &mut impl core::fmt::Write,
) -> core::fmt::Result {
    let Some(b) = flavour.bindings.iter().find(|b| b.slot as usize == slot) else {
        return out.write_str("—");
    };
    let p = b.param as usize;
    let (Some(m), Some(&base)) = (meta.get(p), flavour.base.get(p)) else {
        return out.write_str("—");
    };
    let value = (base + b.curve.apply(norm) * b.depth).clamp(m.min, m.max);
    let label = flavour.macro_label(meta, slot).unwrap_or(m.name);
    crate::track_engine::format_macro_value(label, m.unit, value, out)
}

/// Host `value_to_text` for a macro slot (0185): the flavour's macro **name** (override,
/// else first-bound-param name, else `M<n>`) plus the raw knob position as a percent —
/// e.g. "Punch 65%". Chosen over the bound param's physical value because it round-trips
/// cleanly with [`flavour_macro_parse`] even when a macro drives several params.
pub fn flavour_macro_text(
    meta: &[ParamMeta],
    flavour: &Flavour,
    slot: usize,
    norm: f32,
    out: &mut impl core::fmt::Write,
) -> core::fmt::Result {
    let pct = norm.clamp(0.0, 1.0) * 100.0;
    match flavour.macro_label(meta, slot) {
        Some(label) => write!(out, "{label} {pct:.0}%"),
        None => write!(out, "M{} {pct:.0}%", slot + 1),
    }
}

/// Inverse of [`flavour_macro_text`]: read the trailing percent back to a normalised
/// `0..1` knob value. Reads the numeric run just before `%` (so a name containing digits,
/// e.g. "M1", doesn't confuse it). `None` if unparsable.
pub fn flavour_macro_parse(text: &str) -> Option<f32> {
    let pre = text.split('%').next()?;
    let start = pre.rfind(|c: char| !(c.is_ascii_digit() || c == '.')).map_or(0, |i| i + 1);
    let n: f32 = pre[start..].parse().ok()?;
    Some((n / 100.0).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    // A tiny 2-param family for unit tests: a decay (s) and a pitch (semitones).
    const META: [ParamMeta; 2] = [
        ParamMeta { name: "Decay", unit: MacroUnit::Seconds, min: 0.05, max: 1.5, default: 0.3 },
        ParamMeta { name: "Pitch", unit: MacroUnit::Semitones, min: 0.0, max: 48.0, default: 12.0 },
    ];

    fn flav() -> Flavour {
        Flavour {
            base: vec![0.3, 12.0],
            bindings: vec![
                Binding { slot: 0, param: 0, curve: Curve::Linear, depth: 1.0 },
                Binding { slot: 1, param: 1, curve: Curve::Exp, depth: 24.0 },
            ],
            macro_defaults: [0.5, 0.5, 0.0],
            macro_names: [String::from("Punch"), String::new(), String::new()],
        }
    }

    #[test]
    fn resolve_is_additive_from_base_and_clamped() {
        let f = flav();
        let mut out = [0.0; 2];
        // Macros at 0 → base exactly.
        resolve(&META, &f.base, &f.bindings, &[0.0, 0.0, 0.0], &mut out);
        assert_eq!(out, [0.3, 12.0]);
        // slot0 linear depth 1.0 at 0.5 → 0.3 + 0.5 = 0.8; slot1 exp depth 24 at 0.5 → 12 + 0.25*24 = 18.
        resolve(&META, &f.base, &f.bindings, &[0.5, 0.5, 0.0], &mut out);
        assert!((out[0] - 0.8).abs() < 1e-6, "decay {}", out[0]);
        assert!((out[1] - 18.0).abs() < 1e-6, "pitch {}", out[1]);
        // Over-range is clamped, not wrapped.
        resolve(&META, &f.base, &f.bindings, &[1.0, 1.0, 0.0], &mut out);
        assert_eq!(out[0], 1.3_f32.min(1.5)); // 0.3+1.0 = 1.3 within range
        assert_eq!(out[1], 36.0_f32.min(48.0)); // 12+24 = 36 within range
    }

    #[test]
    fn multiple_bindings_on_one_param_sum() {
        let f = Flavour {
            base: vec![0.1, 0.0],
            bindings: vec![
                Binding { slot: 0, param: 0, curve: Curve::Linear, depth: 0.5 },
                Binding { slot: 1, param: 0, curve: Curve::Linear, depth: 0.4 },
            ],
            macro_defaults: [0.0; MACRO_SLOTS],
            macro_names: Default::default(),
        };
        let mut out = [0.0; 2];
        resolve(&META, &f.base, &f.bindings, &[1.0, 1.0, 0.0], &mut out);
        assert!((out[0] - (0.1 + 0.5 + 0.4)).abs() < 1e-6, "both bindings sum: {}", out[0]);
    }

    #[test]
    fn byte_layout_round_trips() {
        let f = flav();
        let mut bytes = Vec::new();
        f.serialize(&mut bytes);
        let back = Flavour::deserialize(&bytes, 2).unwrap().unwrap();
        assert_eq!(f, back);
    }

    #[test]
    fn deserialize_shape_and_truncation() {
        let f = flav();
        let mut bytes = Vec::new();
        f.serialize(&mut bytes);
        // Wrong family P → keep default (Ok(None)), not an error.
        assert_eq!(Flavour::deserialize(&bytes, 3), Ok(None));
        // Unknown version → keep default.
        assert_eq!(Flavour::deserialize(&[0xFF], 2), Ok(None));
        // Truncated within a known version+shape → Err.
        assert!(Flavour::deserialize(&[FLAVOUR_VERSION, 2, 0x00], 2).is_err());
    }

    #[test]
    fn display_reflects_the_binding() {
        let f = flav();
        let mut s = String::new();
        // Slot 0 has a user macro name ("Punch") → it wins over the bound param's name.
        flavour_macro_display(&META, &f, 0, 0.5, &mut s).unwrap();
        assert!(s.starts_with("Punch"), "slot 0 uses the macro name: {s}");
        s.clear();
        // Slot 1 unnamed → first bound param's name.
        flavour_macro_display(&META, &f, 1, 0.5, &mut s).unwrap();
        assert!(s.starts_with("Pitch"), "slot 1 bound to Pitch: {s}");
        // Unbound slot → sentinel.
        s.clear();
        flavour_macro_display(&META, &f, 2, 0.5, &mut s).unwrap();
        assert_eq!(s, "—");
    }

    #[test]
    fn macro_names_round_trip_and_v1_compat() {
        // v2 round-trip carries the names.
        let f = flav();
        let mut bytes = Vec::new();
        f.serialize(&mut bytes);
        assert_eq!(Flavour::deserialize(&bytes, 2).unwrap().unwrap().macro_names[0], "Punch");
        // A v1 blob (no macro-name section) still parses, with empty names.
        let mut v1 = Vec::new();
        v1.push(1u8); // version 1
        v1.push(2u8); // n_params
        v1.extend_from_slice(&0.3f32.to_le_bytes());
        v1.extend_from_slice(&12.0f32.to_le_bytes());
        v1.push(0u8); // n_bindings
        for _ in 0..MACRO_SLOTS { v1.extend_from_slice(&0.5f32.to_le_bytes()); }
        let back = Flavour::deserialize(&v1, 2).unwrap().unwrap();
        assert_eq!(back.macro_names, <[String; MACRO_SLOTS]>::default());
    }

    #[test]
    fn value_text_uses_macro_name_and_round_trips() {
        let f = flav();
        let mut s = String::new();
        flavour_macro_text(&META, &f, 0, 0.65, &mut s).unwrap();
        assert_eq!(s, "Punch 65%");
        assert!((flavour_macro_parse(&s).unwrap() - 0.65).abs() < 0.01);
        // Unbound, unnamed slot → "M<n> <pct>", and it still round-trips (name has a digit).
        s.clear();
        flavour_macro_text(&META, &f, 2, 0.4, &mut s).unwrap();
        assert_eq!(s, "M3 40%");
        assert!((flavour_macro_parse(&s).unwrap() - 0.4).abs() < 0.01);
    }
}
