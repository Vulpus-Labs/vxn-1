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

// ── Modulation sources (ADR 0007 §7, ticket 0351) ─────────────────────────────
//
// [`Binding::slot`] indexes the source vector [`resolve`] reads. Slots
// `0..MACRO_SLOTS` are the host macro params of ADR 0003 §2, unchanged. Index
// [`SRC_LATENESS`] is the per-trig lateness of ADR 0007 §7 — a *source*, not a
// destination: the destination space, the curve set, the depth-additive form and
// `MACRO_SLOTS` itself are all exactly what ADR 0005 specified, and no new routing
// mechanism is introduced. `resolve` needed no change to read it, because it has
// always indexed the source slice by `b.slot` and reads a missing slot as zero.

/// Source index of the per-trig **lateness**: where the hit sat in its own subdivision
/// slot *after* the swing warp (ADR 0007 §7). Sits directly above the macro slots, so
/// every index below it still names a macro.
///
/// A binding on this slot was **inert** before 0351 (`resolve` was handed only the
/// three macro values, and a missing slot reads as zero), so a flavour blob authored
/// against an older build that happens to carry `slot: 3` changes meaning rather than
/// failing to parse. No shipped flavour does; the format version is deliberately not
/// bumped for it, on ADR 0007's "no user base, redefine rather than migrate" rule.
pub const SRC_LATENESS: usize = MACRO_SLOTS;

/// Length of the source vector [`resolve`] reads — the macro slots plus
/// [`SRC_LATENESS`].
pub const N_SOURCES: usize = MACRO_SLOTS + 1;

/// Out-of-band channel value marking a hit that carries **no colour** (ADR 0007 §7).
///
/// Colour channels are normalised `0.00–1.00`, so a negative channel cannot be
/// confused with one that is merely dark. The distinction is load-bearing: black
/// (`[0, 0, 0]`) is a legitimate macro vector that sends **zero** to all three slots,
/// while an uncoloured hit sends nothing at all and leaves those slots to the p-lock
/// or base of the block.
pub const NO_COLOUR: f32 = -1.0;

const _: () = assert!(
    MACRO_SLOTS == 3,
    "a hit's colour *is* its macro vector (ADR 0007 §7): three channels, three slots"
);

/// Decode a hit's stored `rgb` into a per-trig macro override, or `None` when the hit
/// carries no colour ([`NO_COLOUR`], or any other out-of-band channel).
///
/// Channels are clamped into the normalised `0.00–1.00` the macro slots take — there is
/// no `0–255` representation anywhere in this path. Pure and `Copy`-only, so it runs on
/// the audio thread at resolve time.
#[inline]
pub fn colour_override(rgb: [f32; 3]) -> Option<[f32; MACRO_SLOTS]> {
    // One out-of-band channel disqualifies the whole vector: a colour is three channels
    // or it is nothing, and half a macro vector has no meaning.
    if rgb.iter().any(|c| !c.is_finite() || *c < 0.0) {
        return None;
    }
    let mut out = [0.0; MACRO_SLOTS];
    for (o, c) in out.iter_mut().zip(rgb) {
        *o = c.min(1.0);
    }
    Some(out)
}

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
    ///
    /// Macro slots only. Since 0351 a binding can also sit on [`SRC_LATENESS`], which is
    /// a source rather than a host knob and has no label to give — without the guard it
    /// would answer for slot 3 as though a fourth macro existed.
    pub fn macro_label<'a>(&'a self, meta: &'a [ParamMeta], slot: usize) -> Option<&'a str> {
        if slot >= MACRO_SLOTS {
            return None;
        }
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
/// Called at a voice's trig, when the sources + flavour are stable; the per-sample
/// kernel consumes `out` unchanged.
///
/// `sources` is indexed by [`Binding::slot`]: `0..MACRO_SLOTS` are the macro slots and
/// [`SRC_LATENESS`] is the per-trig lateness. A slot the caller did not supply reads as
/// `0.0`, so a short slice is legal and an out-of-range binding is inert rather than a
/// panic on the audio thread.
///
/// # Precedence of the macro slots (ADR 0007 §7, ticket 0351)
///
/// Three layers can each want to name a macro slot's value at a trig. They are ordered,
/// highest first:
///
/// 1. **The firing hit's colour** — its `rgb`, decoded by [`colour_override`], per trig.
/// 2. **A p-lock** on that slot's lock param (`Decay`/`Tone`/`Pitch`), resolved by
///    [`crate::lane::LaneState::override_value`].
/// 3. **The host macro param** — base value or automation (ADR 0003 §2).
///
/// Layer 1 is applied per **trig**, here. Layers 2 and 3 are applied per **block**, by
/// [`crate::track::Track::apply_effective`], and that asymmetry is pre-existing rather
/// than introduced with the colour: a p-lock whose whole `Revert` hold opens and closes
/// inside one block is not seen by that block's trigs at all. Per-trig p-lock
/// resolution is not this ticket's, and nothing here forecloses it — a colour would
/// still outrank it.
///
/// **Per-hit colour beats a p-lock**, and not for symmetry: a colour is attached to the
/// hit being fired, whereas a p-lock hold — a `Latch` especially — can be an accident of
/// a lock left running from an *earlier* position, which the hit under it never asked
/// for. So a latched p-lock on a macro slot governs exactly the hits carrying no colour.
/// Black is not "no colour": `[0, 0, 0]` is a colour that sends zero to all three slots,
/// and only [`NO_COLOUR`] falls through to layer 2.
///
/// A per-hit override lives entirely in the `sources` vector handed to this call and is
/// **never written back** to host macro state (what `TrackEngine::set_macro` holds), so
/// a coloured hit cannot leave the host's automated value changed behind it.
///
/// `O(P · bindings)` with tiny constants (P and the binding table are both small);
/// no per-param binding index is needed.
#[inline]
pub fn resolve(meta: &[ParamMeta], base: &[f32], bindings: &[Binding], sources: &[f32], out: &mut [f32]) {
    for (p, slot) in out.iter_mut().enumerate().take(meta.len().min(base.len())) {
        let mut v = base[p];
        for b in bindings {
            if b.param as usize == p {
                let m = sources.get(b.slot as usize).copied().unwrap_or(0.0);
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
    // Macro slots only — [`SRC_LATENESS`] is a source, not a knob with a readout.
    if slot >= MACRO_SLOTS {
        return out.write_str("—");
    }
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

    /// AC: black is a colour and sends **zero** to all three slots; only an
    /// out-of-band channel means "no colour". The two must never collapse — one is a
    /// macro vector the user chose, the other is a hit that has not been painted.
    #[test]
    fn black_is_a_colour_and_no_colour_is_not() {
        assert_eq!(colour_override([0.0, 0.0, 0.0]), Some([0.0, 0.0, 0.0]));
        assert_eq!(colour_override([NO_COLOUR; 3]), None);
        // A hit is uncoloured the moment any channel is out of band — half a macro
        // vector is not a colour.
        assert_eq!(colour_override([0.5, NO_COLOUR, 0.5]), None);
        assert_eq!(colour_override([0.5, f32::NAN, 0.5]), None);
        assert_eq!(colour_override([0.5, f32::INFINITY, 0.5]), None);
        // And the default hit of the sequencer is uncoloured, not black.
        assert_eq!(colour_override(crate::sequencer::Hit::default().rgb), None);
    }

    /// AC: values are normalised `0.00–1.00` end to end — no `0–255` anywhere in the
    /// value path. An over-unity channel clamps rather than scaling.
    #[test]
    fn colour_channels_reach_the_slots_normalised() {
        assert_eq!(colour_override([1.0, 0.5, 0.0]), Some([1.0, 0.5, 0.0]));
        assert_eq!(colour_override([255.0, 128.0, 1.5]), Some([1.0, 1.0, 1.0]));
    }

    /// `f` is addressable by the **unchanged** binding table: a binding on
    /// [`SRC_LATENESS`] reads the trig's in-slot position exactly as one on a macro
    /// slot reads its knob. A new source, no new destination, `MACRO_SLOTS` still 3.
    #[test]
    fn lateness_is_a_bindable_source_beside_the_macro_slots() {
        assert_eq!(MACRO_SLOTS, 3);
        assert_eq!(SRC_LATENESS, 3);
        assert_eq!(N_SOURCES, 4);
        let f = Flavour {
            base: vec![0.3, 12.0],
            bindings: vec![Binding {
                slot: SRC_LATENESS as u8,
                param: 1,
                curve: Curve::Linear,
                depth: 24.0,
            }],
            macro_defaults: [0.0; MACRO_SLOTS],
            macro_names: Default::default(),
        };
        let mut out = [0.0; 2];
        resolve(&META, &f.base, &f.bindings, &[0.0, 0.0, 0.0, 0.0], &mut out);
        assert_eq!(out[1], 12.0, "dead on the marker adds nothing");
        resolve(&META, &f.base, &f.bindings, &[0.0, 0.0, 0.0, 0.5], &mut out);
        assert!((out[1] - 24.0).abs() < 1e-6, "half a slot late: {}", out[1]);
        // A source the caller did not supply is inert, not a panic on the audio thread.
        resolve(&META, &f.base, &f.bindings, &[0.0; MACRO_SLOTS], &mut out);
        assert_eq!(out[1], 12.0);
    }

    /// A lateness change must only re-resolve when the flavour actually reads lateness.
    /// Lateness differs between any two hits with different `f` or `nudge`, so a plain
    /// `!=` here would re-resolve and re-cook on every trig of any humanised lane, for
    /// a patch that came out bit-identical.
    #[test]
    fn a_lateness_change_only_re_resolves_when_something_binds_it() {
        use crate::track_engine::TrigMod;
        let plain = flav(); // binds slots 0 and 1 only
        let bound = Flavour {
            bindings: vec![Binding {
                slot: SRC_LATENESS as u8,
                param: 0,
                curve: Curve::Linear,
                depth: 0.5,
            }],
            ..flav()
        };
        let a = TrigMod { macros: None, lateness: 0.1 };
        let b = TrigMod { macros: None, lateness: 0.9 };
        assert!(!a.differs_for(b, &plain.bindings), "no binding reads lateness");
        assert!(a.differs_for(b, &bound.bindings), "this flavour does read it");
        assert!(!a.differs_for(a, &bound.bindings), "an identical trig changes nothing");
        // A colour change always re-resolves, whatever the table binds.
        let c = TrigMod { macros: Some([0.0; MACRO_SLOTS]), lateness: 0.1 };
        assert!(a.differs_for(c, &plain.bindings), "gaining a colour must re-resolve");
        assert!(c.differs_for(a, &plain.bindings), "losing one must too");
    }

    /// [`SRC_LATENESS`] is a source, not a fourth macro slot: the host-facing readouts
    /// must not answer for it just because a binding sits there.
    #[test]
    fn the_lateness_source_is_not_a_fourth_macro_slot() {
        let f = Flavour {
            bindings: vec![Binding {
                slot: SRC_LATENESS as u8,
                param: 0,
                curve: Curve::Linear,
                depth: 0.5,
            }],
            ..flav()
        };
        assert_eq!(f.macro_label(&META, SRC_LATENESS), None);
        let mut s = String::new();
        flavour_macro_display(&META, &f, SRC_LATENESS, 0.5, &mut s).unwrap();
        assert_eq!(s, "—");
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
