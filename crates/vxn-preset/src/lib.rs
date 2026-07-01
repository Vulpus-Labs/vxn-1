//! Shared sparse-TOML preset-codec scaffold (ticket 0143).
//!
//! Both VXN engines hand-roll an almost-identical name-keyed, sparse-vs-default
//! TOML preset codec (E007 / ADR 0005). The *bodies* differ legitimately —
//! vxn-1 has a three-namespace `PerformanceBody`, vxn-2 a flat `params + matrix`
//! shape — but the housekeeping is byte-identical. This crate owns that shared
//! housekeeping so a third synth (or the not-yet-built vxn-2 preset epic) starts
//! from it instead of copy-pasting:
//!
//! - [`Meta`] — the `[meta]` table (`name` + optional author/category/comment).
//! - [`PresetError`] — the hard-failure type (malformed envelope / bad schema).
//! - [`Header`] / [`SCHEMA`] — the schema-probe envelope read before committing
//!   to a body shape.
//! - [`value_for`] — render one param value as a typed TOML scalar.
//!
//! Pure: depends only on `serde` + `toml`. The per-engine `ParamDesc` /
//! `ParamKind` types stay in their engines; [`value_for`] takes the reduced
//! [`ScalarKind`] each engine maps its own descriptor kind onto.

use serde::{Deserialize, Serialize};

/// Preset *file-format* version (independent of any binary state-blob version).
/// Because the format is name-keyed, most evolutions need no bump; reserve this
/// for structural changes (ADR 0005 §2).
pub const SCHEMA: u32 = 1;

/// Free-form preset metadata (the `[meta]` table). Only `name` is required.
/// Category is the **only** discriminator the browser groups on — there is no
/// tag list. Field-for-field the same shape as `vxn_core_app::PresetMeta` (the
/// view/event projection); each store converts between the two.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Browser grouping. Free-form string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// Why a preset failed to parse. Unknown keys / bad enum labels do **not** land
/// here — those are non-fatal warnings the engine codec collects separately.
#[derive(Debug)]
pub enum PresetError {
    /// The TOML did not parse, or the envelope (`schema`, `meta`, the body
    /// table) was missing or the wrong type.
    Toml(toml::de::Error),
    /// `schema` is not a version this build understands.
    UnsupportedSchema { found: u32, expected: u32 },
}

impl std::fmt::Display for PresetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PresetError::Toml(e) => write!(f, "invalid preset TOML: {e}"),
            PresetError::UnsupportedSchema { found, expected } => write!(
                f,
                "unsupported preset schema {found} (this build expects {expected})"
            ),
        }
    }
}

impl std::error::Error for PresetError {}

impl From<toml::de::Error> for PresetError {
    fn from(e: toml::de::Error) -> Self {
        PresetError::Toml(e)
    }
}

/// Just enough of the envelope to validate the schema before committing to a
/// body shape. The engine codec deserialises this first, checks `schema`
/// against [`SCHEMA`], then deserialises the full body.
#[derive(Deserialize)]
pub struct Header {
    pub schema: u32,
}

/// The reduced descriptor-kind discriminant [`value_for`] needs to render a
/// param value as a typed TOML scalar. The engine `ParamKind` types differ
/// (different crates), so each engine maps its own onto this.
pub enum ScalarKind<'a> {
    Enum { variants: &'a [&'a str] },
    Bool,
    Int,
    Float,
}

/// One param's value as a typed TOML scalar, matching its descriptor kind.
///
/// Enum values store the variant **label**; the index is clamped into range
/// (and floored at 0) so a stray out-of-range value renders the nearest valid
/// label rather than panicking. `f32 → f64` widening is exact and narrows back
/// to the same `f32` on read, so floats round-trip precisely.
pub fn value_for(kind: ScalarKind<'_>, v: f32) -> toml::Value {
    match kind {
        ScalarKind::Enum { variants } => {
            let i = (v.round().max(0.0) as usize).min(variants.len().saturating_sub(1));
            toml::Value::String(variants[i].to_string())
        }
        ScalarKind::Bool => toml::Value::Boolean(v >= 0.5),
        ScalarKind::Int => toml::Value::Integer(v.round() as i64),
        ScalarKind::Float => toml::Value::Float(v as f64),
    }
}
