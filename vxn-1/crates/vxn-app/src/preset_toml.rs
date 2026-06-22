//! Name-keyed TOML preset codec (E019 / 0066).
//!
//! The portable, human-readable preset format: a sparse TOML file keyed by
//! [`ParamDesc::name`], identical to the desktop build's
//! `vxn-engine::preset` format. Where [`crate::state`] is the *binary*
//! host-state blob (fast, positional, opaque), this is the *file/share* format
//! the web export/import and URL share-link use so an exported patch round-trips
//! across native and wasm (ADR 0005 §1 keeps the two formats separate).
//!
//! It lives here in `vxn-app` — wasm-clean, working purely through the
//! [`ParamModel`] + [`Vxn1Params`] trait surface and the descriptor table — the
//! same way [`crate::state`] mirrors the engine's binary blob. A drift-guard
//! test in `vxn-engine` asserts this writer is byte-identical to the engine's
//! `Performance::to_toml_string`, so the two cannot diverge.
//!
//! Layout (matches `vxn-engine::preset`):
//!
//! ```toml
//! schema = 1
//! [meta]
//! name = "…"          # author / category / comment optional
//! [performance]
//! key_mode = "Whole"  # by label
//! split_point = 60
//! [performance.global]   # sparse: only params deviating from default
//! [performance.upper]
//! [performance.lower]
//! ```

use serde::{Deserialize, Serialize};

use crate::domain::{KeyMode, Layer};
use crate::model::{ParamId, ParamModel, Vxn1Params};
use crate::params::{
    GlobalParam, ParamDesc, ParamKind, PatchParam, global_clap_id, patch_clap_id,
};
use crate::PresetMeta;

/// Preset *file-format* version. Matches `vxn-engine::preset::SCHEMA`; because
/// the format is name-keyed, most evolutions need no bump (ADR 0005 §2).
pub const SCHEMA: u32 = 1;

/// Why a TOML preset failed to parse. Unknown keys / bad enum labels are *not*
/// errors — those are non-fatal warnings (see [`read_toml_into`]).
#[derive(Debug)]
pub enum TomlError {
    /// The TOML did not parse, or required envelope fields were missing.
    Parse(String),
    /// `schema` is not a version this build understands.
    UnsupportedSchema { found: u32, expected: u32 },
}

impl std::fmt::Display for TomlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TomlError::Parse(e) => write!(f, "invalid preset TOML: {e}"),
            TomlError::UnsupportedSchema { found, expected } => write!(
                f,
                "unsupported preset schema {found} (this build expects {expected})"
            ),
        }
    }
}

impl std::error::Error for TomlError {}

// ── On-disk file shape (serde) ──────────────────────────────────────────────
//
// Mirrors `vxn-engine::preset`'s `PerformanceFile` / `PerformanceBody` exactly
// (scalar fields before table fields so TOML serialization is well-formed; the
// param maps stay dynamically typed `toml::Table` and are resolved against the
// descriptor by hand).

#[derive(Serialize, Deserialize)]
struct PerformanceFile {
    schema: u32,
    meta: PresetMeta,
    performance: PerformanceBody,
}

#[derive(Serialize, Deserialize)]
struct PerformanceBody {
    key_mode: String,
    split_point: u8,
    #[serde(default)]
    global: toml::Table,
    #[serde(default)]
    upper: toml::Table,
    #[serde(default)]
    lower: toml::Table,
}

#[derive(Deserialize)]
struct Header {
    schema: u32,
}

// ── Sparse write (model value → TOML value) ─────────────────────────────────

/// One param's value as a typed TOML scalar, matching its descriptor kind.
/// Byte-identical to `vxn-engine::preset::value_for`.
fn value_for(desc: &ParamDesc, v: f32) -> toml::Value {
    match desc.kind {
        ParamKind::Enum { variants } => {
            let i = (v.round() as usize).min(variants.len().saturating_sub(1));
            toml::Value::String(variants[i].to_string())
        }
        ParamKind::Bool => toml::Value::Boolean(v >= 0.5),
        ParamKind::Int { .. } => toml::Value::Integer(v.round() as i64),
        ParamKind::Float { .. } => toml::Value::Float(v as f64),
    }
}

/// Build one sparse `toml::Table` from a slice of (descriptor, value) pairs:
/// only entries deviating from their descriptor default are written.
fn sparse_table<'a>(entries: impl Iterator<Item = (&'a ParamDesc, f32)>) -> toml::Table {
    let mut t = toml::Table::new();
    for (d, v) in entries {
        if v != d.default {
            t.insert(d.name.to_string(), value_for(d, v));
        }
    }
    t
}

/// Serialize a model + shared state to a sparse TOML preset string. Generic over
/// any concrete model (the engine's `SharedParams`, the web `WebModel`) so the
/// one codec serves every backend.
pub fn write_toml<M: ParamModel + Vxn1Params + ?Sized>(model: &M, meta: &PresetMeta) -> String {
    let global = sparse_table(GlobalParam::all().map(|g| {
        let d = g.desc();
        (d, model.get(ParamId::new(global_clap_id(g))))
    }));
    let upper = sparse_table(PatchParam::all().map(|p| {
        let d = p.desc();
        (d, model.get(ParamId::new(patch_clap_id(Layer::Upper, p))))
    }));
    let lower = sparse_table(PatchParam::all().map(|p| {
        let d = p.desc();
        (d, model.get(ParamId::new(patch_clap_id(Layer::Lower, p))))
    }));

    let file = PerformanceFile {
        schema: SCHEMA,
        meta: meta.clone(),
        performance: PerformanceBody {
            key_mode: model.key_mode().label().to_string(),
            split_point: model.split_point(),
            global,
            upper,
            lower,
        },
    };
    // Values are finite descriptor-range numbers / valid labels, so serializing
    // this shape cannot fail (matches the engine's `.expect`).
    toml::to_string_pretty(&file).expect("performance preset serialization is infallible")
}

// ── Default-fill read (TOML value → model, collecting warnings) ──────────────

/// Resolve one TOML scalar to a plain-unit `f32` for `desc`. On any type /
/// label mismatch, push a warning and return `None` (caller keeps the default).
/// Byte-identical logic to `vxn-engine::preset::parse_value`.
fn parse_value(
    desc: &ParamDesc,
    ctx: &str,
    key: &str,
    val: &toml::Value,
    warnings: &mut Vec<String>,
) -> Option<f32> {
    match desc.kind {
        ParamKind::Enum { .. } => match val.as_str() {
            Some(s) => match desc.variant_index(s) {
                Some(i) => Some(i as f32),
                None => {
                    warnings.push(format!("{ctx}.{key}: unknown enum label `{s}` (using default)"));
                    None
                }
            },
            None => {
                warnings.push(format!("{ctx}.{key}: expected a string label (using default)"));
                None
            }
        },
        ParamKind::Bool => match val.as_bool() {
            Some(b) => Some(if b { 1.0 } else { 0.0 }),
            None => {
                warnings.push(format!("{ctx}.{key}: expected true/false (using default)"));
                None
            }
        },
        ParamKind::Int { .. } | ParamKind::Float { .. } => {
            if let Some(fv) = val.as_float() {
                Some(fv as f32)
            } else if let Some(iv) = val.as_integer() {
                Some(iv as f32)
            } else {
                warnings.push(format!("{ctx}.{key}: expected a number (using default)"));
                None
            }
        }
    }
}

/// Reset every param in the model to its descriptor default (a sparse file omits
/// params that are at default, so the target must start clean).
fn reset_to_defaults<M: ParamModel + ?Sized>(model: &M) {
    for g in GlobalParam::all() {
        let d = g.desc();
        model.set(ParamId::new(global_clap_id(g)), d.default);
    }
    for p in PatchParam::all() {
        let d = p.desc();
        model.set(ParamId::new(patch_clap_id(Layer::Upper, p)), d.default);
        model.set(ParamId::new(patch_clap_id(Layer::Lower, p)), d.default);
    }
}

/// Apply a sparse patch table for one layer into the model, clamping each value
/// to its descriptor range. Unknown keys warn and are skipped.
fn apply_patch_table<M: ParamModel + ?Sized>(
    model: &M,
    table: &toml::Table,
    ctx: &str,
    layer: Layer,
    warnings: &mut Vec<String>,
) {
    for (key, val) in table {
        match PatchParam::from_name(key) {
            Some(p) => {
                let d = p.desc();
                if let Some(v) = parse_value(d, ctx, key, val, warnings) {
                    model.set(ParamId::new(patch_clap_id(layer, p)), d.clamp(v));
                }
            }
            None => warnings.push(format!("{ctx}: unknown parameter `{key}` (skipped)")),
        }
    }
}

/// Apply the sparse global table into the model.
fn apply_global_table<M: ParamModel + ?Sized>(
    model: &M,
    table: &toml::Table,
    ctx: &str,
    warnings: &mut Vec<String>,
) {
    for (key, val) in table {
        match GlobalParam::from_name(key) {
            Some(g) => {
                let d = g.desc();
                if let Some(v) = parse_value(d, ctx, key, val, warnings) {
                    model.set(ParamId::new(global_clap_id(g)), d.clamp(v));
                }
            }
            None => warnings.push(format!("{ctx}: unknown parameter `{key}` (skipped)")),
        }
    }
}

/// Parse a TOML preset and apply it into `model`, returning the preset's
/// [`PresetMeta`] plus any non-fatal warnings (unknown keys, bad enum labels,
/// type mismatches — each fell back to the descriptor default). Only a malformed
/// envelope (`schema` / structure) is a hard [`TomlError`]; on a hard error the
/// model is left untouched (the parse fails before any mutation).
///
/// The model is first reset to descriptor defaults, then the sparse tables are
/// applied — so absent params land on default exactly as the desktop loader
/// (which builds a fresh `PluginState::default`) does.
pub fn read_toml_into<M: ParamModel + Vxn1Params + ?Sized>(
    model: &M,
    s: &str,
) -> Result<(PresetMeta, Vec<String>), TomlError> {
    let header: Header = toml::from_str(s).map_err(|e| TomlError::Parse(e.to_string()))?;
    if header.schema != SCHEMA {
        return Err(TomlError::UnsupportedSchema {
            found: header.schema,
            expected: SCHEMA,
        });
    }
    let file: PerformanceFile = toml::from_str(s).map_err(|e| TomlError::Parse(e.to_string()))?;
    let body = file.performance;

    let mut warnings = Vec::new();
    let key_mode = KeyMode::from_label(&body.key_mode).unwrap_or_else(|| {
        warnings.push(format!(
            "performance.key_mode: unknown `{}` (using Whole)",
            body.key_mode
        ));
        KeyMode::Whole
    });

    // Parse succeeded — commit. Reset first so a sparse file's omitted params
    // land on default rather than keeping the model's current values.
    reset_to_defaults(model);
    apply_global_table(model, &body.global, "performance.global", &mut warnings);
    apply_patch_table(model, &body.upper, "performance.upper", Layer::Upper, &mut warnings);
    apply_patch_table(model, &body.lower, "performance.lower", Layer::Lower, &mut warnings);
    model.set_key_mode(key_mode);
    model.set_split_point(body.split_point);

    Ok((file.meta, warnings))
}
