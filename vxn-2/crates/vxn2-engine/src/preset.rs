//! Portable VXN2 preset format (ADR 0005).
//!
//! A preset is a **TOML** text file, keyed by parameter [`ParamDesc::id`]
//! (never by CLAP index — the param table reorders freely, so any positional
//! format would rot). Enums store their
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
use crate::shared::{MatrixRowRaw, N_EG_CURVES, N_KS_CURVES, ParamModel, SharedParams};

/// Per-side KS level-curve machine names, indexed by discriminant
/// (`NegLin` / `PosLin` / `NegExp` / `PosExp`). Mirror of
/// [`vxn2_dsp::ks::KsCurve`].
const KS_CURVE_NAMES: [&str; 4] = ["neg-lin", "pos-lin", "neg-exp", "pos-exp"];

/// Default curve discriminant for a side: left = `NegLin` (0), right =
/// `NegExp` (2).
#[inline]
fn default_ks_curve(side: usize) -> u8 {
    if side == 0 { 0 } else { 2 }
}

/// Preset `params`-table key for op `op` (0-based), `side` (0 = left/below
/// BP, 1 = right/above BP): e.g. `op1-ks-l-curve`.
fn ks_curve_key(op: usize, side: usize) -> String {
    format!("op{}-ks-{}-curve", op + 1, if side == 0 { "l" } else { "r" })
}

/// Inverse of [`ks_curve_key`]: `(op0-based, side)` if `key` is a KS-curve
/// label with an in-range op, else `None`.
fn parse_ks_curve_key(key: &str) -> Option<(usize, usize)> {
    let (num, tail) = key.strip_prefix("op")?.split_once("-ks-")?;
    let op = num.parse::<usize>().ok()?;
    if op < 1 || op > N_KS_CURVES / 2 {
        return None;
    }
    let side = match tail {
        "l-curve" => 0,
        "r-curve" => 1,
        _ => return None,
    };
    Some((op - 1, side))
}

/// Curve machine name → discriminant (case-insensitive).
fn ks_curve_from_name(label: &str) -> Option<u8> {
    let lc = label.trim().to_lowercase();
    KS_CURVE_NAMES.iter().position(|n| *n == lc).map(|i| i as u8)
}

/// Per-op EG level-curve machine names, indexed by discriminant
/// (`Exp` / `Lin`). Mirror of [`vxn2_dsp::eg::EgCurve`].
const EG_CURVE_NAMES: [&str; 2] = ["exp", "lin"];

/// Default EG-curve discriminant: `Exp` (0) — the log curve is the shipped
/// default (ADR 0007).
#[inline]
fn default_eg_curve() -> u8 {
    0
}

/// Preset `params`-table key for op `op` (0-based): e.g. `op1-eg-curve`.
fn eg_curve_key(op: usize) -> String {
    format!("op{}-eg-curve", op + 1)
}

/// Inverse of [`eg_curve_key`]: `op0-based` if `key` is an EG-curve label with
/// an in-range op, else `None`.
fn parse_eg_curve_key(key: &str) -> Option<usize> {
    let num = key.strip_prefix("op")?.strip_suffix("-eg-curve")?;
    let op = num.parse::<usize>().ok()?;
    if op < 1 || op > N_EG_CURVES {
        return None;
    }
    Some(op - 1)
}

/// EG-curve machine name → discriminant (case-insensitive).
fn eg_curve_from_name(label: &str) -> Option<u8> {
    let lc = label.trim().to_lowercase();
    EG_CURVE_NAMES.iter().position(|n| *n == lc).map(|i| i as u8)
}

// Shared sparse-TOML scaffold. `Meta`, `PresetError`, `Header` and `SCHEMA`
// live in `vxn-preset`; re-exported here to keep the `crate::preset::Meta`
// path stable for `factory.rs` and `preset_io.rs`. (`Meta` is field-for-field
// the same shape as `vxn_core_app::PresetMeta`; the store converts between the
// two.)
pub use vxn_preset::{Header, Meta, PresetError, SCHEMA};
use vxn_preset::ScalarKind;

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

/// One routed matrix slot in the file. `source`/`dest`/`curve`/`scale-src` are
/// kebab machine names; `curve` defaults to `lin` and `scale-src` to `none`
/// when omitted (so presets without those keys round-trip unchanged).
#[derive(Serialize, Deserialize)]
struct MatrixRowFile {
    slot: u8,
    source: String,
    dest: String,
    #[serde(default = "default_curve")]
    curve: String,
    depth: f64,
    /// Secondary scale source. Omitted when `none`; `serde` default keeps
    /// presets with no key reading as unscaled.
    #[serde(rename = "scale-src", default = "default_scale_src", skip_serializing_if = "is_none_src")]
    scale_src: String,
}

fn default_curve() -> String {
    "lin".to_string()
}

fn default_scale_src() -> String {
    "none".to_string()
}

fn is_none_src(s: &str) -> bool {
    s == "none"
}

/// Decode a host-state blob into the flat value table + matrix rows. Reuses
/// [`SharedParams::load_bytes`] so every blob-version migration is honoured
/// here without re-implementing the wire format.
#[allow(clippy::type_complexity)]
fn decode_blob(
    blob: &[u8],
) -> Result<
    (
        [f32; TOTAL_PARAMS],
        [MatrixRowRaw; N_SLOTS],
        [u8; N_KS_CURVES],
        [u8; N_EG_CURVES],
    ),
    String,
> {
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
    let curves = std::array::from_fn(|k| sp.ks_curve_raw(k / 2, k % 2));
    let eg_curves = std::array::from_fn(|op| sp.eg_curve_raw(op));
    Ok((values, matrix, curves, eg_curves))
}

/// Encode a flat value table + matrix rows into a host-state blob the model
/// accepts via `restore_from_bytes`. Starts from a fresh [`SharedParams`]
/// (default-patch seed), overwrites every value and every matrix slot, then
/// snapshots — so unspecified matrix slots come back inert rather than
/// inheriting the default-patch routing.
fn encode_blob(
    values: &[f32; TOTAL_PARAMS],
    matrix: &[MatrixRowRaw; N_SLOTS],
    curves: &[u8; N_KS_CURVES],
    eg_curves: &[u8; N_EG_CURVES],
) -> Vec<u8> {
    let sp = SharedParams::new();
    for (i, v) in values.iter().enumerate() {
        sp.set(i, *v);
    }
    for (s, row) in matrix.iter().enumerate() {
        sp.set_matrix_row_raw(s, *row);
    }
    for (k, c) in curves.iter().enumerate() {
        sp.set_ks_curve_raw(k / 2, k % 2, *c);
    }
    for (op, c) in eg_curves.iter().enumerate() {
        sp.set_eg_curve_raw(op, *c);
    }
    ParamModel::snapshot_bytes(&sp)
}

/// One param's value as a typed TOML scalar, matching its descriptor kind.
/// Maps this engine's `ParamKind` onto the shared [`ScalarKind`] and delegates
/// the rendering to [`vxn_preset::value_for`].
fn value_for(desc: &ParamDesc, v: f32) -> toml::Value {
    let kind = match desc.kind {
        ParamKind::Enum { variants } => ScalarKind::Enum { variants },
        ParamKind::Bool => ScalarKind::Bool,
        ParamKind::Int { .. } => ScalarKind::Int,
        ParamKind::Float { .. } => ScalarKind::Float,
    };
    vxn_preset::value_for(kind, v)
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
        let scale_src = SOURCE_NAMES
            .get(row.scale_src as usize)
            .copied()
            .unwrap_or("none");
        out.push(MatrixRowFile {
            slot: s as u8,
            source: source.to_string(),
            dest: dest.to_string(),
            curve: curve.to_string(),
            depth: row.depth as f64,
            scale_src: scale_src.to_string(),
        });
    }
    out
}

/// Serialise a host-state blob + metadata to a sparse TOML preset.
pub fn write_preset(meta: &Meta, blob: &[u8]) -> Result<String, String> {
    let (values, matrix, curves, eg_curves) = decode_blob(blob)?;
    let mut params = params_table(&values);
    // KS level-curve selectors aren't CLAP params — write them as sparse
    // string keys in the same table (only sides that deviate from the
    // legacy default), read back by `parse_ks_curve_key` below.
    for (k, &c) in curves.iter().enumerate() {
        let (op, side) = (k / 2, k % 2);
        if c != default_ks_curve(side) {
            let label = KS_CURVE_NAMES.get(c as usize).copied().unwrap_or("neg-lin");
            params.insert(ks_curve_key(op, side), toml::Value::String(label.to_string()));
        }
    }
    // EG level-curve selectors: same sparse treatment — only ops
    // deviating from the default (`Exp`) are written, read back by
    // `parse_eg_curve_key` below.
    for (op, &c) in eg_curves.iter().enumerate() {
        if c != default_eg_curve() {
            let label = EG_CURVE_NAMES.get(c as usize).copied().unwrap_or("exp");
            params.insert(eg_curve_key(op), toml::Value::String(label.to_string()));
        }
    }
    let file = PresetFile {
        schema: SCHEMA,
        meta: meta.clone(),
        params,
        matrix: matrix_rows_file(&matrix),
    };
    // Values are clamped to finite descriptor ranges and labels come from the
    // descriptor tables, so serialisation of this shape cannot fail.
    toml::to_string_pretty(&file).map_err(|e| e.to_string())
}

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
) -> Result<
    (
        Meta,
        [f32; TOTAL_PARAMS],
        [MatrixRowRaw; N_SLOTS],
        [u8; N_KS_CURVES],
        [u8; N_EG_CURVES],
        Vec<String>,
    ),
    PresetError,
> {
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
    // KS curves start at the legacy frozen default; sparse string keys in the
    // params table override individual sides.
    let mut curves: [u8; N_KS_CURVES] = std::array::from_fn(|k| default_ks_curve(k % 2));
    // EG curves start at the default (`Exp`); sparse string keys override per op.
    let mut eg_curves: [u8; N_EG_CURVES] = std::array::from_fn(|_| default_eg_curve());
    for (key, val) in &file.params {
        match id_of(key) {
            Some(id) => {
                if let Some(v) = parse_value(&PARAMS[id], key, val, &mut warnings) {
                    values[id] = PARAMS[id].clamp(v);
                }
            }
            None => {
                if let Some((op, side)) = parse_ks_curve_key(key) {
                    match val.as_str().and_then(ks_curve_from_name) {
                        Some(c) => curves[op * 2 + side] = c,
                        None => warnings.push(format!(
                            "params: bad KS curve `{key}` = `{val}` (using default)"
                        )),
                    }
                } else if let Some(op) = parse_eg_curve_key(key) {
                    match val.as_str().and_then(eg_curve_from_name) {
                        Some(c) => eg_curves[op] = c,
                        None => warnings.push(format!(
                            "params: bad EG curve `{key}` = `{val}` (using default)"
                        )),
                    }
                } else {
                    warnings.push(format!("params: unknown parameter `{key}` (skipped)"));
                }
            }
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
        // Secondary scale source. Absent key → "none" → 0; unknown name → 0
        // (mirrors the primary source's degrade-to-inert on a bad blob).
        let scale_src = name_to_u8(&SOURCE_NAMES, &row.scale_src).unwrap_or_else(|| {
            warnings.push(format!(
                "matrix slot {}: unknown scale source `{}` (unscaled)",
                row.slot, row.scale_src
            ));
            0
        });
        matrix[slot] = MatrixRowRaw {
            source,
            dest,
            curve,
            active: source != 0 && dest != 0,
            depth,
            scale_src,
        };
    }

    Ok((file.meta, values, matrix, curves, eg_curves, warnings))
}

/// Parse a TOML preset to `(meta, host-state blob, warnings)`. The blob is
/// ready to hand to the model's `restore_from_bytes`.
pub fn from_toml_str(s: &str) -> Result<(Meta, Vec<u8>, Vec<String>), PresetError> {
    let (meta, values, matrix, curves, eg_curves, warnings) = read_preset(s)?;
    Ok((meta, encode_blob(&values, &matrix, &curves, &eg_curves), warnings))
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

    #[test]
    fn ks_curves_round_trip_through_text() {
        let src = SharedParams::new();
        src.set_ks_curve_raw(0, 0, 3); // op1 left  → PosExp
        src.set_ks_curve_raw(0, 1, 1); // op1 right → PosLin
        src.set_ks_curve_raw(4, 1, 0); // op5 right → NegLin (deviates from default NegExp)
        let blob = ParamModel::snapshot_bytes(&src);

        let toml = write_preset(&meta("KS"), &blob).unwrap();
        // Sparse: default sides are omitted; touched sides are present.
        assert!(toml.contains("op1-ks-l-curve = \"pos-exp\""), "{toml}");
        assert!(toml.contains("op1-ks-r-curve = \"pos-lin\""), "{toml}");
        assert!(toml.contains("op5-ks-r-curve = \"neg-lin\""), "{toml}");
        assert!(!toml.contains("op2-ks-l-curve"), "default left omitted:\n{toml}");

        let (_m, blob2, warnings) = from_toml_str(&toml).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let dst = SharedParams::new();
        ParamModel::load_bytes(&dst, &blob2).unwrap();
        assert_eq!(dst.ks_curve_raw(0, 0), 3);
        assert_eq!(dst.ks_curve_raw(0, 1), 1);
        assert_eq!(dst.ks_curve_raw(4, 1), 0);
        // An untouched op keeps the legacy default.
        assert_eq!(dst.ks_curve_raw(2, 0), 0);
        assert_eq!(dst.ks_curve_raw(2, 1), 2);
    }

    #[test]
    fn eg_curves_round_trip_through_text() {
        let src = SharedParams::new();
        src.set_eg_curve_raw(0, 1); // op1 → Lin (deviates from default Exp)
        src.set_eg_curve_raw(5, 1); // op6 → Lin
        let blob = ParamModel::snapshot_bytes(&src);

        let toml = write_preset(&meta("EG"), &blob).unwrap();
        // Sparse: default `Exp` ops are omitted; touched ops are present.
        assert!(toml.contains("op1-eg-curve = \"lin\""), "{toml}");
        assert!(toml.contains("op6-eg-curve = \"lin\""), "{toml}");
        assert!(!toml.contains("op2-eg-curve"), "default op omitted:\n{toml}");

        let (_m, blob2, warnings) = from_toml_str(&toml).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let dst = SharedParams::new();
        ParamModel::load_bytes(&dst, &blob2).unwrap();
        assert_eq!(dst.eg_curve_raw(0), 1);
        assert_eq!(dst.eg_curve_raw(5), 1);
        // An untouched op keeps the default (Exp).
        assert_eq!(dst.eg_curve_raw(2), 0);
    }

    /// A snapshot blob from a default `SharedParams` (the default patch)
    /// round-trips through the text format back to bit-identical
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

    /// A slot's secondary scale source round-trips through TOML as a kebab
    /// `scale-src` key, is omitted when `none` (sparse), and reloads to the
    /// same `SourceId`.
    #[test]
    fn matrix_scale_src_round_trips_through_text() {
        let src = SharedParams::new();
        // Slot 0: LFO2 → global-pitch, gated by the mod wheel (the
        // mod-wheel-vibrato case). scale_src = ModWheel (5).
        src.set_matrix_row_raw(
            0,
            MatrixRowRaw {
                source: SourceId::Lfo2 as u8,
                dest: DestId::GlobalPitch as u8,
                curve: CurveKind::Lin as u8,
                active: true,
                depth: 0.5,
                scale_src: SourceId::ModWheel as u8,
            },
        );
        let blob = ParamModel::snapshot_bytes(&src);
        let toml = write_preset(&meta("Scale"), &blob).unwrap();
        assert!(toml.contains("scale-src = \"mod-wheel\""), "{toml}");

        let (_m, blob2, warnings) = from_toml_str(&toml).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let dst = SharedParams::new();
        ParamModel::load_bytes(&dst, &blob2).unwrap();
        assert_eq!(dst.matrix_row_raw(0).scale_src, SourceId::ModWheel as u8);
    }

    /// An unscaled slot omits the `scale-src` key entirely (sparse write).
    #[test]
    fn matrix_scale_src_omitted_when_none() {
        let src = SharedParams::new();
        src.set_matrix_row_raw(
            0,
            MatrixRowRaw {
                source: SourceId::Lfo1 as u8,
                dest: DestId::GlobalPitch as u8,
                curve: CurveKind::Lin as u8,
                active: true,
                depth: 0.5,
                scale_src: 0,
            },
        );
        let blob = ParamModel::snapshot_bytes(&src);
        let toml = write_preset(&meta("NoScale"), &blob).unwrap();
        assert!(!toml.contains("scale-src"), "none must be omitted:\n{toml}");
    }

    /// Absent key → `None` (0); an unknown scale-src name → `None` with a
    /// warning (mirrors the primary source's degrade-to-inert).
    #[test]
    fn matrix_scale_src_absent_and_unknown_degrade_to_none() {
        // Absent: no scale-src key at all.
        let absent = r#"
schema = 1
[meta]
name = "A"
[[matrix]]
slot = 0
source = "lfo1"
dest = "global-pitch"
depth = 0.5
"#;
        let (_m, _v, mtx, _c, _e, warnings) = read_preset(absent).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(mtx[0].scale_src, 0);

        // Unknown: a bogus name warns and leaves the slot unscaled.
        let unknown = r#"
schema = 1
[meta]
name = "U"
[[matrix]]
slot = 0
source = "lfo1"
dest = "global-pitch"
depth = 0.5
scale-src = "bogus"
"#;
        let (_m, _v, mtx, _c, _e, warnings) = read_preset(unknown).unwrap();
        assert_eq!(mtx[0].scale_src, 0);
        assert!(
            warnings.iter().any(|w| w.contains("unknown scale source")),
            "{warnings:?}"
        );
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
        let (_m, values, _mtx, _curves, _eg, warnings) = read_preset(s).unwrap();
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
        let (_m, values, _mtx, _curves, _eg, warnings) = read_preset(s).unwrap();
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
        let (_m, values, _mtx, _curves, _eg, _w) = read_preset(s).unwrap();
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
        let (_m, _v, mtx, _curves, _eg, warnings) = read_preset(s).unwrap();
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
        let (_m, _v, mtx, _curves, _eg, warnings) = read_preset(s).unwrap();
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
