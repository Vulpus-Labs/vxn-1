//! Portable VXN2 preset format (E007 lineage, ADR 0005).
//!
//! A preset is a **TOML** text file, keyed by parameter [`ParamDesc::id`]
//! (never by CLAP index — the param table reorders freely, so any positional
//! format would rot; [[vxn1-id-stability-dropped]]). Enums store their
//! **variant label** (`lfo2-shape = "Sine"`), bools as `true`/`false`,
//! numbers in the descriptor's plain unit. Files are **sparse**: only params
//! that deviate from their descriptor default are written, so presets stay
//! small, diffable, and auto-adopt improved defaults.
//!
//! The mod matrix is not part of the flat param table — it's 16 slots of
//! `(source, dest, curve, depth)` topology ([`crate::matrix`]). It serialises
//! as an `[[matrix]]` array of tables, source/dest/curve stored by their
//! kebab machine name ([`crate::matrix::SOURCE_NAMES`] etc). Only routed
//! slots (`source` and `dest` both non-`none`) are written.
//!
//! This module is the **pure** mapping between the engine's host-state blob
//! ([`crate::shared::ParamModel::snapshot_bytes`]) and the file format. It
//! bridges the two by round-tripping through a throwaway [`SharedParams`] —
//! the blob's binary layout (and all its version migrations) stay owned by
//! `shared.rs`, never duplicated here. No IO, no embedding, no clap, no UI:
//! that is [`crate::preset_io`] (user-dir IO + the `PresetStore` impl) and
//! [`crate::factory`] (the embedded bank). Main-thread only; the audio path
//! never gains a serde/toml dependency.

use serde::{Deserialize, Serialize};

use crate::matrix::{
    CURVE_NAMES, DEST_NAMES, N_SLOTS, SOURCE_NAMES,
};
use crate::params::{PARAMS, ParamDesc, ParamKind, TOTAL_PARAMS, id_of};
use crate::shared::{MatrixRowRaw, ParamModel, SharedParams};

/// Preset *file-format* version (independent of the binary state blob
/// version). Because the format is name-keyed, most evolutions need no bump;
/// reserve this for structural changes (ADR 0005 §2).
pub const SCHEMA: u32 = 1;

/// Free-form preset metadata (the `[meta]` table). Only `name` is required.
/// Category is the **only** discriminator the browser groups on — there is no
/// tag list. Field-for-field the same shape as [`vxn_core_app::PresetMeta`];
/// the store converts between the two.
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

/// Why a preset failed to parse. Unknown keys / bad enum labels do **not**
/// land here — those are non-fatal warnings (see [`from_toml_str`]).
#[derive(Debug)]
pub enum PresetError {
    /// The TOML did not parse, or the envelope (`schema`, `meta`) was missing
    /// or the wrong type.
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

// ── On-disk file shape (serde) ──────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct PresetFile {
    schema: u32,
    meta: Meta,
    /// `id -> typed scalar`, resolved against the descriptor by hand below.
    #[serde(default)]
    params: toml::Table,
    /// Routed matrix slots only. Slots whose source or dest is `none` are
    /// omitted on write and default-inert on read.
    #[serde(default)]
    matrix: Vec<MatrixRowFile>,
}

/// One routed matrix slot in the file. `source`/`dest`/`curve` are kebab
/// machine names; `curve` defaults to `lin` when omitted.
#[derive(Serialize, Deserialize)]
struct MatrixRowFile {
    slot: u8,
    source: String,
    dest: String,
    #[serde(default = "default_curve")]
    curve: String,
    depth: f64,
}

fn default_curve() -> String {
    "lin".to_string()
}

/// Just enough of the envelope to validate the schema before committing to a
/// body shape.
#[derive(Deserialize)]
struct Header {
    schema: u32,
}

// ── blob <-> arrays (round-trip through SharedParams) ───────────────────────

/// Decode a host-state blob into the flat value table + matrix rows. Reuses
/// [`SharedParams::load_bytes`] so every blob-version migration is honoured
/// here without re-implementing the wire format.
fn decode_blob(blob: &[u8]) -> Result<([f32; TOTAL_PARAMS], [MatrixRowRaw; N_SLOTS]), String> {
    let sp = SharedParams::new();
    ParamModel::load_bytes(&sp, blob).map_err(|e| e.to_string())?;
    let mut values = [0.0_f32; TOTAL_PARAMS];
    for (i, v) in values.iter_mut().enumerate() {
        *v = sp.get(i);
    }
    let mut matrix = [MatrixRowRaw::default(); N_SLOTS];
    for (s, row) in matrix.iter_mut().enumerate() {
        *row = sp.matrix_row_raw(s);
    }
    Ok((values, matrix))
}

/// Encode a flat value table + matrix rows into a host-state blob the model
/// accepts via `restore_from_bytes`. Starts from a fresh [`SharedParams`]
/// (default-patch seed), overwrites every value and every matrix slot, then
/// snapshots — so unspecified matrix slots come back inert rather than
/// inheriting the default-patch routing.
fn encode_blob(values: &[f32; TOTAL_PARAMS], matrix: &[MatrixRowRaw; N_SLOTS]) -> Vec<u8> {
    let sp = SharedParams::new();
    for (i, v) in values.iter().enumerate() {
        sp.set(i, *v);
    }
    for (s, row) in matrix.iter().enumerate() {
        sp.set_matrix_row_raw(s, *row);
    }
    ParamModel::snapshot_bytes(&sp)
}

// ── Sparse write (engine values → TOML) ─────────────────────────────────────

/// One param's value as a typed TOML scalar, matching its descriptor kind.
fn value_for(desc: &ParamDesc, v: f32) -> toml::Value {
    match desc.kind {
        ParamKind::Enum { variants } => {
            let i = (v.round().max(0.0) as usize).min(variants.len().saturating_sub(1));
            toml::Value::String(variants[i].to_string())
        }
        ParamKind::Bool => toml::Value::Boolean(v >= 0.5),
        ParamKind::Int { .. } => toml::Value::Integer(v.round() as i64),
        // f32 → f64 widening is exact, and the f64 narrows back to the same
        // f32 on read, so the stored value round-trips precisely.
        ParamKind::Float { .. } => toml::Value::Float(v as f64),
    }
}

fn params_table(values: &[f32; TOTAL_PARAMS]) -> toml::Table {
    let mut t = toml::Table::new();
    for (i, d) in PARAMS.iter().enumerate() {
        if values[i] != d.default {
            t.insert(d.id.to_string(), value_for(d, values[i]));
        }
    }
    t
}

fn matrix_rows_file(matrix: &[MatrixRowRaw; N_SLOTS]) -> Vec<MatrixRowFile> {
    let mut out = Vec::new();
    for (s, row) in matrix.iter().enumerate() {
        // Only routed slots: both endpoints must be real (non-`none`).
        if row.source == 0 || row.dest == 0 {
            continue;
        }
        let source = SOURCE_NAMES
            .get(row.source as usize)
            .copied()
            .unwrap_or("none");
        let dest = DEST_NAMES.get(row.dest as usize).copied().unwrap_or("none");
        let curve = CURVE_NAMES.get(row.curve as usize).copied().unwrap_or("lin");
        out.push(MatrixRowFile {
            slot: s as u8,
            source: source.to_string(),
            dest: dest.to_string(),
            curve: curve.to_string(),
            depth: row.depth as f64,
        });
    }
    out
}

/// Serialise a host-state blob + metadata to a sparse TOML preset.
pub fn write_preset(meta: &Meta, blob: &[u8]) -> Result<String, String> {
    let (values, matrix) = decode_blob(blob)?;
    let file = PresetFile {
        schema: SCHEMA,
        meta: meta.clone(),
        params: params_table(&values),
        matrix: matrix_rows_file(&matrix),
    };
    // Values are clamped to finite descriptor ranges and labels come from the
    // descriptor tables, so serialisation of this shape cannot fail.
    toml::to_string_pretty(&file).map_err(|e| e.to_string())
}

// ── Default-fill read (TOML → engine values, collecting warnings) ───────────

/// Case-insensitive variant lookup for an `Enum` descriptor.
fn variant_index(desc: &ParamDesc, label: &str) -> Option<usize> {
    let lc = label.trim().to_lowercase();
    desc.variants()
        .iter()
        .position(|v| v.to_lowercase() == lc)
}

/// Resolve one TOML scalar to a plain-unit `f32` for `desc`. On any type or
/// label mismatch, push a warning and return `None` (the caller leaves the
/// descriptor default in place).
fn parse_value(
    desc: &ParamDesc,
    key: &str,
    val: &toml::Value,
    warnings: &mut Vec<String>,
) -> Option<f32> {
    match desc.kind {
        ParamKind::Enum { .. } => match val.as_str() {
            Some(s) => match variant_index(desc, s) {
                Some(i) => Some(i as f32),
                None => {
                    warnings.push(format!("params.{key}: unknown enum label `{s}` (using default)"));
                    None
                }
            },
            None => {
                warnings.push(format!("params.{key}: expected a string label (using default)"));
                None
            }
        },
        ParamKind::Bool => match val.as_bool() {
            Some(b) => Some(if b { 1.0 } else { 0.0 }),
            None => {
                warnings.push(format!("params.{key}: expected true/false (using default)"));
                None
            }
        },
        ParamKind::Int { .. } | ParamKind::Float { .. } => {
            if let Some(fv) = val.as_float() {
                Some(fv as f32)
            } else if let Some(iv) = val.as_integer() {
                Some(iv as f32)
            } else {
                warnings.push(format!("params.{key}: expected a number (using default)"));
                None
            }
        }
    }
}

/// Look a kebab machine name up in one of the matrix label tables, returning
/// its `u8` discriminant. Case-insensitive; `None` on miss.
fn name_to_u8(table: &[&str], name: &str) -> Option<u8> {
    let lc = name.trim().to_lowercase();
    table.iter().position(|n| n.to_lowercase() == lc).map(|i| i as u8)
}

/// Parse a TOML preset into `(meta, values, matrix, warnings)`. Unspecified
/// params fall back to their descriptor default; unspecified matrix slots are
/// inert. Unknown keys / bad enum labels / type mismatches each fall back to
/// the default and emit a non-fatal warning. Only a malformed envelope is a
/// hard [`PresetError`].
#[allow(clippy::type_complexity)]
pub fn read_preset(
    s: &str,
) -> Result<(Meta, [f32; TOTAL_PARAMS], [MatrixRowRaw; N_SLOTS], Vec<String>), PresetError> {
    let header: Header = toml::from_str(s)?;
    if header.schema != SCHEMA {
        return Err(PresetError::UnsupportedSchema {
            found: header.schema,
            expected: SCHEMA,
        });
    }

    let file: PresetFile = toml::from_str(s)?;
    let mut warnings = Vec::new();

    // Values start at the bare descriptor defaults (not the default patch),
    // so a sparse preset round-trips to exactly the params it names.
    let mut values = [0.0_f32; TOTAL_PARAMS];
    for (i, d) in PARAMS.iter().enumerate() {
        values[i] = d.default;
    }
    for (key, val) in &file.params {
        match id_of(key) {
            Some(id) => {
                if let Some(v) = parse_value(&PARAMS[id], key, val, &mut warnings) {
                    values[id] = PARAMS[id].clamp(v);
                }
            }
            None => warnings.push(format!("params: unknown parameter `{key}` (skipped)")),
        }
    }

    let mut matrix = [MatrixRowRaw::default(); N_SLOTS];
    for row in &file.matrix {
        let slot = row.slot as usize;
        if slot >= N_SLOTS {
            warnings.push(format!("matrix: slot {} out of range (skipped)", row.slot));
            continue;
        }
        let source = match name_to_u8(&SOURCE_NAMES, &row.source) {
            Some(v) => v,
            None => {
                warnings.push(format!(
                    "matrix slot {}: unknown source `{}` (slot left inert)",
                    row.slot, row.source
                ));
                continue;
            }
        };
        let dest = match name_to_u8(&DEST_NAMES, &row.dest) {
            Some(v) => v,
            None => {
                warnings.push(format!(
                    "matrix slot {}: unknown dest `{}` (slot left inert)",
                    row.slot, row.dest
                ));
                continue;
            }
        };
        let curve = name_to_u8(&CURVE_NAMES, &row.curve).unwrap_or_else(|| {
            warnings.push(format!(
                "matrix slot {}: unknown curve `{}` (using lin)",
                row.slot, row.curve
            ));
            0
        });
        let depth = (row.depth as f32).clamp(-1.0, 1.0);
        matrix[slot] = MatrixRowRaw {
            source,
            dest,
            curve,
            active: source != 0 && dest != 0,
            depth,
        };
    }

    Ok((file.meta, values, matrix, warnings))
}

/// Parse a TOML preset to `(meta, host-state blob, warnings)`. The blob is
/// ready to hand to the model's `restore_from_bytes`.
pub fn from_toml_str(s: &str) -> Result<(Meta, Vec<u8>, Vec<String>), PresetError> {
    let (meta, values, matrix, warnings) = read_preset(s)?;
    Ok((meta, encode_blob(&values, &matrix), warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matrix::{CurveKind, DestId, SourceId};
    use crate::params::id_of;

    fn meta(name: &str) -> Meta {
        Meta {
            name: name.to_string(),
            ..Meta::default()
        }
    }

    /// A snapshot blob from a default `SharedParams` (the E.PIANO default
    /// patch) round-trips through the text format back to bit-identical
    /// param values + matrix topology.
    #[test]
    fn default_patch_round_trips_through_text() {
        let src = SharedParams::new();
        let blob = ParamModel::snapshot_bytes(&src);
        let toml = write_preset(&meta("RT"), &blob).unwrap();
        let (m, blob2, warnings) = from_toml_str(&toml).unwrap();
        assert_eq!(m.name, "RT");
        assert!(warnings.is_empty(), "{warnings:?}");

        let dst = SharedParams::new();
        ParamModel::load_bytes(&dst, &blob2).unwrap();
        for i in 0..TOTAL_PARAMS {
            assert_eq!(src.get(i), dst.get(i), "param {} ({})", i, PARAMS[i].id);
        }
        for s in 0..N_SLOTS {
            let a = src.matrix_row_raw(s);
            let b = dst.matrix_row_raw(s);
            assert_eq!(a.source, b.source, "slot {s} source");
            assert_eq!(a.dest, b.dest, "slot {s} dest");
            assert_eq!(a.curve, b.curve, "slot {s} curve");
            assert!((a.depth - b.depth).abs() < 1e-6, "slot {s} depth");
        }
    }

    #[test]
    fn write_is_sparse() {
        // A fresh model deviates from descriptor defaults only where the
        // default patch sets non-default values — the params table is small,
        // not 179 entries.
        let src = SharedParams::new();
        let blob = ParamModel::snapshot_bytes(&src);
        let toml = write_preset(&meta("Sparse"), &blob).unwrap();
        let doc: toml::Table = toml::from_str(&toml).unwrap();
        let params = doc.get("params").and_then(|v| v.as_table()).unwrap();
        assert!(params.len() < TOTAL_PARAMS, "expected sparse, got {}", params.len());
        // feedback 6.0 is part of the default patch, so it must be written.
        assert!(params.contains_key("feedback"));
    }

    #[test]
    fn enum_label_is_case_insensitive() {
        let s = r#"
schema = 1
[meta]
name = "X"
[params]
lfo2-shape = "pulse"
"#;
        let (_m, values, _mtx, warnings) = read_preset(s).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        // "Pulse" is index 4 in LFO_SHAPES.
        assert_eq!(values[id_of("lfo2-shape").unwrap()], 4.0);
    }

    #[test]
    fn unknown_param_warns_and_skips() {
        let s = r#"
schema = 1
[meta]
name = "X"
[params]
not-a-param = 5.0
feedback = 3.0
"#;
        let (_m, values, _mtx, warnings) = read_preset(s).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("not-a-param"), "{warnings:?}");
        assert_eq!(values[id_of("feedback").unwrap()], 3.0);
    }

    #[test]
    fn value_clamps_on_read() {
        let s = r#"
schema = 1
[meta]
name = "X"
[params]
feedback = 99.0
"#;
        let (_m, values, _mtx, _w) = read_preset(s).unwrap();
        assert_eq!(values[id_of("feedback").unwrap()], 7.0);
    }

    #[test]
    fn matrix_row_round_trips_by_name() {
        let s = r#"
schema = 1
[meta]
name = "X"
[[matrix]]
slot = 0
source = "lfo2"
dest = "global-pitch"
curve = "lin"
depth = 0.5
"#;
        let (_m, _v, mtx, warnings) = read_preset(s).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(mtx[0].source, SourceId::Lfo2 as u8);
        assert_eq!(mtx[0].dest, DestId::GlobalPitch as u8);
        assert_eq!(mtx[0].curve, CurveKind::Lin as u8);
        assert!(mtx[0].active);
        assert!((mtx[0].depth - 0.5).abs() < 1e-6);
    }

    #[test]
    fn unknown_matrix_source_warns_and_skips_slot() {
        let s = r#"
schema = 1
[meta]
name = "X"
[[matrix]]
slot = 2
source = "nope"
dest = "op1-level"
depth = 0.3
"#;
        let (_m, _v, mtx, warnings) = read_preset(s).unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(mtx[2].source, 0);
        assert_eq!(mtx[2].dest, 0);
    }

    #[test]
    fn schema_mismatch_is_typed_error() {
        let s = r#"
schema = 2
[meta]
name = "X"
"#;
        match read_preset(s) {
            Err(PresetError::UnsupportedSchema { found: 2, expected: 1 }) => {}
            other => panic!("expected UnsupportedSchema, got {other:?}"),
        }
    }

    #[test]
    fn malformed_toml_is_error() {
        assert!(matches!(read_preset("nonsense ===="), Err(PresetError::Toml(_))));
    }
}
