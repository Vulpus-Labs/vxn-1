//! Portable VXN1b preset format (ticket 0203; ADR 0001 §6, VXN1 ADR 0005).
//!
//! A preset is a **sparse TOML** text file, keyed by [`ParamDesc::name`] (never
//! by CLAP index — the param table may reorder, so any positional format would
//! rot). Only params that deviate from their descriptor default are written, so
//! files stay small and auto-adopt improved defaults. Enums store their variant
//! **label**, bools as `true`/`false`, numbers in the descriptor's plain unit.
//!
//! The mod matrix is not part of the flat param table. Its **topology**
//! (`source`/`dest`/`curve`/`scale-src`) serialises as an `[[matrix]]` array of
//! tables, ids stored by their kebab machine name ([`SOURCE_NAMES`] etc). Only
//! *routed* slots (source and dest both non-`none`) are written; an absent key
//! or unknown name decodes to `None` (an inert slot). **Slot depths are params,
//! not topology** (ADR 0001 §5): they ride the `params` table as
//! `matrix_slotN_depth` keys and are deliberately *not* duplicated in the
//! `[[matrix]]` rows.
//!
//! Pure main-thread mapping between a [`PluginState`] and the file text — no IO,
//! no clap, no UI. Reuses the shared [`vxn_preset`] scaffold (`Meta`, `Header`,
//! `SCHEMA`, `value_for`, `PresetError`) so a third synth starts from it.

use serde::{Deserialize, Serialize};
use vxn_preset::ScalarKind;
pub use vxn_preset::{Header, Meta, PresetError, SCHEMA};

use vxn_core_app::{ParamDesc, ParamKind};

use crate::matrix::{
    CURVE_NAMES, Curve, DEST_NAMES, DestId, MatrixSlot, MatrixTable, N_SLOTS, SOURCE_NAMES,
    SourceId,
};
use crate::params::{PARAMS, ParamId, Params};
use crate::state::PluginState;

#[derive(Serialize, Deserialize)]
struct PresetFile {
    schema: u32,
    meta: Meta,
    /// `name -> typed scalar`, resolved against the descriptor by hand below.
    #[serde(default)]
    params: toml::Table,
    /// Routed matrix slots only. Slots whose source or dest is `none` are
    /// omitted on write and default-inert on read.
    #[serde(default)]
    matrix: Vec<MatrixRowFile>,
}

/// One routed matrix slot in the file. `source`/`dest` are required kebab
/// machine names; `curve` defaults to `lin` and `scale-src` to `none` when
/// omitted (so presets without those keys round-trip unchanged). **No `depth`
/// field** — depth is the `matrix_slotN_depth` param.
#[derive(Serialize, Deserialize)]
struct MatrixRowFile {
    slot: u8,
    source: String,
    dest: String,
    #[serde(default = "default_curve")]
    curve: String,
    #[serde(
        rename = "scale-src",
        default = "default_scale_src",
        skip_serializing_if = "is_none_src"
    )]
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

/// One param value as a typed TOML scalar, mapping this engine's [`ParamKind`]
/// onto the shared [`ScalarKind`].
fn value_for(desc: &ParamDesc, v: f32) -> toml::Value {
    let kind = match desc.kind {
        ParamKind::Enum { variants } => ScalarKind::Enum { variants },
        ParamKind::Bool => ScalarKind::Bool,
        ParamKind::Int { .. } => ScalarKind::Int,
        ParamKind::Float { .. } => ScalarKind::Float,
    };
    vxn_preset::value_for(kind, v)
}

/// The sparse `params` table: every value that deviates from its descriptor
/// default (slot depths included — they are ordinary params).
fn params_table(params: &Params) -> toml::Table {
    let mut t = toml::Table::new();
    for (i, d) in PARAMS.iter().enumerate() {
        let v = params.get_index(i);
        if v != d.default {
            t.insert(d.name.to_string(), value_for(d, v));
        }
    }
    t
}

/// The `[[matrix]]` rows: one per routed slot, topology only.
fn matrix_rows(matrix: &MatrixTable) -> Vec<MatrixRowFile> {
    let mut out = Vec::new();
    for (s, slot) in matrix.slots.iter().enumerate() {
        if !slot.is_active() {
            continue;
        }
        out.push(MatrixRowFile {
            slot: s as u8,
            source: SOURCE_NAMES[slot.source as usize].to_string(),
            dest: DEST_NAMES[slot.dest as usize].to_string(),
            curve: CURVE_NAMES[slot.curve as usize].to_string(),
            scale_src: SOURCE_NAMES[slot.scale_src as usize].to_string(),
        });
    }
    out
}

/// Serialise a [`PluginState`] + metadata to a sparse TOML preset.
pub fn write_preset(meta: &Meta, state: &PluginState) -> Result<String, String> {
    let file = PresetFile {
        schema: SCHEMA,
        meta: meta.clone(),
        params: params_table(&state.params),
        matrix: matrix_rows(&state.matrix),
    };
    // Values are clamped to finite ranges and labels come from the descriptor
    // tables, so serialisation of this shape cannot fail.
    toml::to_string_pretty(&file).map_err(|e| e.to_string())
}

/// Resolve one TOML scalar to a plain-unit `f32` for `desc`. On any type or
/// label mismatch, push a warning and return `None` (leaving the default).
fn parse_value(
    desc: &ParamDesc,
    key: &str,
    val: &toml::Value,
    warnings: &mut Vec<String>,
) -> Option<f32> {
    match desc.kind {
        ParamKind::Enum { .. } => match val.as_str() {
            Some(s) => match desc.variant_index(s) {
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

/// Look a kebab machine name up in a matrix label table, returning its `u8`
/// discriminant. Case-insensitive; `None` on miss.
fn name_to_u8(table: &[&str], name: &str) -> Option<u8> {
    let lc = name.trim().to_lowercase();
    table
        .iter()
        .position(|n| n.eq_ignore_ascii_case(&lc))
        .map(|i| i as u8)
}

/// Parse a TOML preset into `(meta, params, matrix, warnings)`. Unspecified
/// params fall back to their descriptor default; unspecified matrix slots are
/// inert. Unknown keys / bad enum labels / type mismatches each fall back and
/// emit a non-fatal warning. Only a malformed envelope is a hard
/// [`PresetError`].
///
/// The returned [`MatrixTable`] has depths seeded from the parsed params (the
/// param block is the depth authority), so the topology is render-ready.
pub fn read_preset(
    s: &str,
) -> Result<(Meta, Params, MatrixTable, Vec<String>), PresetError> {
    let header: Header = toml::from_str(s)?;
    if header.schema != SCHEMA {
        return Err(PresetError::UnsupportedSchema {
            found: header.schema,
            expected: SCHEMA,
        });
    }

    let file: PresetFile = toml::from_str(s)?;
    let mut warnings = Vec::new();

    // Params start at descriptor defaults; sparse keys override. `set` clamps.
    let mut params = Params::default();
    for (key, val) in &file.params {
        match ParamId::from_name(key) {
            Some(id) => {
                if let Some(v) = parse_value(id.desc(), key, val, &mut warnings) {
                    params.set(id, v);
                }
            }
            None => warnings.push(format!("params: unknown parameter `{key}` (skipped)")),
        }
    }

    // Matrix starts all-inert; each routed row sets one slot's topology.
    let mut matrix = MatrixTable::default();
    for row in &file.matrix {
        let slot = row.slot as usize;
        if slot >= N_SLOTS {
            warnings.push(format!("matrix: slot {} out of range (skipped)", row.slot));
            continue;
        }
        let Some(source) = name_to_u8(&SOURCE_NAMES, &row.source) else {
            warnings.push(format!(
                "matrix slot {}: unknown source `{}` (slot left inert)",
                row.slot, row.source
            ));
            continue;
        };
        let Some(dest) = name_to_u8(&DEST_NAMES, &row.dest) else {
            warnings.push(format!(
                "matrix slot {}: unknown dest `{}` (slot left inert)",
                row.slot, row.dest
            ));
            continue;
        };
        let curve = name_to_u8(&CURVE_NAMES, &row.curve).unwrap_or_else(|| {
            warnings.push(format!(
                "matrix slot {}: unknown curve `{}` (using lin)",
                row.slot, row.curve
            ));
            0
        });
        let scale_src = name_to_u8(&SOURCE_NAMES, &row.scale_src).unwrap_or_else(|| {
            warnings.push(format!(
                "matrix slot {}: unknown scale source `{}` (unscaled)",
                row.slot, row.scale_src
            ));
            0
        });
        matrix.slots[slot] = MatrixSlot {
            source: SourceId::from_u8(source),
            dest: DestId::from_u8(dest),
            curve: Curve::from_u8(curve),
            depth: 0.0, // seeded from params below
            scale_src: SourceId::from_u8(scale_src),
        };
    }
    // Depth authority is the param block — seed every slot from it.
    for (i, slot) in matrix.slots.iter_mut().enumerate() {
        slot.depth = params.slot_depth(i);
    }

    Ok((file.meta, params, matrix, warnings))
}

/// Parse a TOML preset to `(meta, state, warnings)`.
pub fn from_toml_str(s: &str) -> Result<(Meta, PluginState, Vec<String>), PresetError> {
    let (meta, params, matrix, warnings) = read_preset(s)?;
    Ok((meta, PluginState { params, matrix }, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::TOTAL_PARAMS;

    fn meta(name: &str) -> Meta {
        Meta {
            name: name.to_string(),
            ..Meta::default()
        }
    }

    fn sample_state() -> PluginState {
        let mut params = Params::default();
        params.set(ParamId::Cutoff, 1234.0);
        params.set(ParamId::Osc1Wave, 3.0); // Pulse
        params.set(ParamId::MatrixSlot0Depth, 1.0);
        params.set(ParamId::MatrixSlot2Depth, -0.25);
        let mut matrix = MatrixTable::default();
        matrix.slots[0] = MatrixSlot {
            source: SourceId::Env2,
            dest: DestId::Amp,
            depth: 1.0,
            curve: Curve::Lin,
            scale_src: SourceId::None,
        };
        matrix.slots[2] = MatrixSlot {
            source: SourceId::Lfo1,
            dest: DestId::Pitch,
            depth: -0.25,
            curve: Curve::Bipolar,
            scale_src: SourceId::ModWheel,
        };
        PluginState { params, matrix }
    }

    #[test]
    fn topology_round_trips_through_text() {
        let st = sample_state();
        let toml = write_preset(&meta("RT"), &st).unwrap();
        let (m, back, warnings) = from_toml_str(&toml).unwrap();
        assert_eq!(m.name, "RT");
        assert!(warnings.is_empty(), "{warnings:?}");

        assert_eq!(back.params.get(ParamId::Cutoff), 1234.0);
        assert_eq!(back.params.get(ParamId::Osc1Wave), 3.0);

        let s0 = back.matrix.slots[0];
        assert_eq!(s0.source, SourceId::Env2);
        assert_eq!(s0.dest, DestId::Amp);
        assert_eq!(s0.curve, Curve::Lin);
        assert_eq!(s0.scale_src, SourceId::None);
        assert_eq!(s0.depth, 1.0); // from the param

        let s2 = back.matrix.slots[2];
        assert_eq!(s2.source, SourceId::Lfo1);
        assert_eq!(s2.dest, DestId::Pitch);
        assert_eq!(s2.curve, Curve::Bipolar);
        assert_eq!(s2.scale_src, SourceId::ModWheel);
        assert_eq!(s2.depth, -0.25);
    }

    #[test]
    fn write_is_sparse_and_omits_inactive_slots() {
        let st = sample_state();
        let toml = write_preset(&meta("Sparse"), &st).unwrap();
        let doc: toml::Table = toml::from_str(&toml).unwrap();

        // Only deviating params are written, far fewer than the full table.
        let params = doc.get("params").and_then(|v| v.as_table()).unwrap();
        assert!(params.len() < TOTAL_PARAMS, "expected sparse, got {}", params.len());
        assert!(params.contains_key("cutoff"));
        // Depth is a param key, not a matrix-row field.
        assert!(params.contains_key("matrix_slot0_depth"));

        // Exactly the two routed slots appear; the other 14 are omitted.
        let rows = doc.get("matrix").and_then(|v| v.as_array()).unwrap();
        assert_eq!(rows.len(), 2);
        // No depth field ever appears inside a matrix row.
        for row in rows {
            assert!(row.get("depth").is_none(), "matrix row must not carry depth");
        }
    }

    #[test]
    fn depth_is_not_duplicated_in_matrix_rows() {
        let st = sample_state();
        let toml = write_preset(&meta("D"), &st).unwrap();
        assert!(!toml.contains("[[matrix]]\ndepth"), "{toml}");
        // The only depth occurrences are the param keys.
        assert!(toml.contains("matrix_slot0_depth"), "{toml}");
    }

    #[test]
    fn scale_src_omitted_when_none() {
        let mut st = sample_state();
        st.matrix.slots[2].scale_src = SourceId::None;
        let toml = write_preset(&meta("NoScale"), &st).unwrap();
        // slot 0 already has scale_src none; neither routed slot writes the key.
        assert!(!toml.contains("scale-src"), "none must be omitted:\n{toml}");
    }

    #[test]
    fn absent_curve_and_scale_default() {
        let s = r#"
schema = 1
[meta]
name = "A"
[[matrix]]
slot = 0
source = "lfo1"
dest = "pitch"
"#;
        let (_m, _p, matrix, warnings) = read_preset(s).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(matrix.slots[0].source, SourceId::Lfo1);
        assert_eq!(matrix.slots[0].dest, DestId::Pitch);
        assert_eq!(matrix.slots[0].curve, Curve::Lin);
        assert_eq!(matrix.slots[0].scale_src, SourceId::None);
    }

    #[test]
    fn unknown_source_warns_and_leaves_slot_inert() {
        let s = r#"
schema = 1
[meta]
name = "U"
[[matrix]]
slot = 2
source = "nope"
dest = "cutoff"
"#;
        let (_m, _p, matrix, warnings) = read_preset(s).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("unknown source"), "{warnings:?}");
        assert!(!matrix.slots[2].is_active());
    }

    #[test]
    fn unknown_scale_src_degrades_to_none_with_warning() {
        let s = r#"
schema = 1
[meta]
name = "U"
[[matrix]]
slot = 0
source = "lfo1"
dest = "pitch"
scale-src = "bogus"
"#;
        let (_m, _p, matrix, warnings) = read_preset(s).unwrap();
        assert_eq!(matrix.slots[0].scale_src, SourceId::None);
        assert!(
            warnings.iter().any(|w| w.contains("unknown scale source")),
            "{warnings:?}"
        );
    }

    #[test]
    fn unknown_param_key_warns_and_skips() {
        let s = r#"
schema = 1
[meta]
name = "X"
[params]
not_a_param = 5.0
cutoff = 800.0
"#;
        let (_m, params, _mtx, warnings) = read_preset(s).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("not_a_param"), "{warnings:?}");
        assert_eq!(params.get(ParamId::Cutoff), 800.0);
    }

    #[test]
    fn value_clamps_on_read() {
        let s = r#"
schema = 1
[meta]
name = "X"
[params]
resonance = 9.0
"#;
        let (_m, params, _mtx, _w) = read_preset(s).unwrap();
        assert_eq!(params.get(ParamId::Resonance), 1.0);
    }

    #[test]
    fn enum_label_is_case_insensitive() {
        let s = r#"
schema = 1
[meta]
name = "X"
[params]
osc1_wave = "pulse"
"#;
        let (_m, params, _mtx, warnings) = read_preset(s).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(params.get(ParamId::Osc1Wave), 3.0);
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

    #[test]
    fn slot_out_of_range_warns_and_skips() {
        let s = r#"
schema = 1
[meta]
name = "X"
[[matrix]]
slot = 99
source = "lfo1"
dest = "pitch"
"#;
        let (_m, _p, matrix, warnings) = read_preset(s).unwrap();
        assert!(warnings.iter().any(|w| w.contains("out of range")), "{warnings:?}");
        assert!(matrix.slots.iter().all(|s| !s.is_active()));
    }
}
