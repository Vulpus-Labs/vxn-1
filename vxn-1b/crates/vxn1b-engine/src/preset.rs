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
//! **Two layers (0221, ADR 0002 §4).** A VXN1b patch is two layers plus the
//! keyboard record, so the file is:
//!
//! ```text
//! schema / [meta]                        envelope
//! [params] / [[matrix]]                  Layer 1 — unchanged, at the top level
//! [layer2.params] / [[layer2.matrix]]    Layer 2 — optional
//! [keys]                                 mode / split-point / lfo2-link — optional
//! ```
//!
//! Layer 1 stays at the top level and both new sections are optional, so **every
//! pre-0221 single-layer preset still loads**: no `[layer2]` gives Layer 2 the
//! factory patch, no `[keys]` gives Layer 2 off (= `Single`), which is exactly
//! what those files meant when they were written. That is the whole migration —
//! the format is name-keyed and sparse rather than positional, so absence can
//! carry meaning. Both sections are omitted again on write when they are at
//! their defaults, so a single-layer patch saves as the same text it always did.
//!
//! **`KeyMode` is written, the toggles are derived.** The file stores `mode =
//! "single" | "dual" | "split"` rather than [`KeyState`]'s two booleans: the
//! mode is the user-facing control (ADR 0002 §3), and it round-trips a preset
//! exactly. The one thing it does not preserve is a split *armed while Layer 2
//! is off* — that reads back as plain `Single`. The host-state blob keeps both
//! toggles verbatim, so nothing is lost across a DAW save; only an explicit
//! preset save normalises it.
//!
//! Pure main-thread mapping between a [`PluginState`] and the file text — no IO,
//! no clap, no UI. Reuses the shared [`vxn_preset`] scaffold (`Meta`, `Header`,
//! `SCHEMA`, `value_for`, `PresetError`) so a third synth starts from it.

use serde::{Deserialize, Serialize};
use vxn_preset::ScalarKind;
pub use vxn_preset::{Header, Meta, PresetError, SCHEMA};

use vxn_core_app::{ParamDesc, ParamKind};

use crate::engine::{DEFAULT_SPLIT_POINT, KeyOp, KeyState};
use crate::matrix::{
    CURVE_NAMES, DEST_NAMES, DestId, MatrixSlot, MatrixTable, MatrixTableExt, N_SLOTS,
    POLARITY_NAMES, Polarity, SHAPE_NAMES, SOURCE_NAMES, Shape, SourceId, curve_code, curve_split,
};
use crate::params::{PARAMS, ParamId, Params};
use crate::state::{LayerState, PluginState};

/// Machine names for the three [`crate::KeyMode`]s, indexed by the mode's
/// position in `Single / Dual / Split` order — the same 0/1/2 encoding the UI's
/// `set_key_mode` opcode uses.
const KEY_MODE_NAMES: [&str; 3] = ["single", "dual", "split"];

#[derive(Serialize, Deserialize)]
struct PresetFile {
    schema: u32,
    meta: Meta,
    /// Layer 1's `name -> typed scalar`, resolved against the descriptor by hand
    /// below. Top-level (not under a `[layer1]`) so pre-0221 files still parse.
    #[serde(default)]
    params: toml::Table,
    /// Layer 1's routed matrix slots only. Slots whose source or dest is `none`
    /// are omitted on write and default-inert on read.
    #[serde(default)]
    matrix: Vec<MatrixRowFile>,
    /// Layer 2's patch. Absent → the factory patch, i.e. a single-layer
    /// preset. Declared **after** the top-level array-of-tables so the emitted
    /// TOML puts `[layer2]` past the last `[[matrix]]` row rather than inside it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    layer2: Option<LayerFile>,
    /// Keyboard record. Absent → `Single` at the default split point with
    /// no LFO 2 link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    keys: Option<KeysFile>,
}

/// One non-primary layer's patch: the same `params` + `matrix` pair the top
/// level carries for Layer 1, nested under `[layer2]`.
#[derive(Default, Serialize, Deserialize)]
struct LayerFile {
    #[serde(default)]
    params: toml::Table,
    #[serde(default)]
    matrix: Vec<MatrixRowFile>,
}

/// The keyboard record. Every field defaults, so a partial `[keys]` (say, only
/// `split-point`) is valid and the rest fall back.
#[derive(Serialize, Deserialize)]
struct KeysFile {
    /// `single` | `dual` | `split`. Case-insensitive on read.
    #[serde(default = "default_key_mode")]
    mode: String,
    #[serde(rename = "split-point", default = "default_split_point")]
    split_point: u8,
    #[serde(rename = "lfo2-link", default)]
    lfo2_link: bool,
}

fn default_key_mode() -> String {
    KEY_MODE_NAMES[0].to_string()
}

fn default_split_point() -> u8 {
    DEFAULT_SPLIT_POINT
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
    /// Range mapping on the scale VCA — the VCA's own polarity axis (0341),
    /// spelled as a [`POLARITY_NAMES`] name. Omitted when `direct`, which *is*
    /// the fold the VCA applied before the axis existed, so a preset written
    /// without the key loads with its scaling unchanged.
    #[serde(
        rename = "scale-polarity",
        default = "default_polarity",
        skip_serializing_if = "is_direct"
    )]
    scale_polarity: String,
    /// Response bend on the scale VCA. Omitted when `lin` (the identity), so a
    /// preset with a straight-line VCA round-trips exactly as before.
    #[serde(
        rename = "scale-shape",
        default = "default_curve",
        skip_serializing_if = "is_lin"
    )]
    scale_shape: String,
    /// The player's on/off switch. Absent → `true`: every preset written before
    /// the toggle existed listed only routed slots, and meant them to sound.
    /// Omitted when on, so an all-on patch is unchanged on disk.
    #[serde(default = "default_enabled", skip_serializing_if = "is_true")]
    enabled: bool,
}

fn default_curve() -> String {
    "lin".to_string()
}

fn default_polarity() -> String {
    "direct".to_string()
}

fn is_direct(s: &str) -> bool {
    s == "direct"
}

fn default_enabled() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

fn is_lin(s: &str) -> bool {
    s == "lin"
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

/// The `[[matrix]]` rows: one per **wired** slot, topology only.
///
/// Wired, not active — a route the player has switched off still has endpoints,
/// shaping and a scale source worth persisting, and dropping it would turn the
/// toggle into a destructive delete across a save/load.
fn matrix_rows(matrix: &MatrixTable) -> Vec<MatrixRowFile> {
    let mut out = Vec::new();
    for (s, slot) in matrix.slots.iter().enumerate() {
        if !slot.is_wired() {
            continue;
        }
        out.push(MatrixRowFile {
            slot: s as u8,
            source: SOURCE_NAMES[slot.source as usize].to_string(),
            dest: DEST_NAMES[slot.dest as usize].to_string(),
            curve: CURVE_NAMES[curve_code(slot.polarity, slot.shape) as usize].to_string(),
            scale_src: SOURCE_NAMES[slot.scale_src as usize].to_string(),
            scale_polarity: POLARITY_NAMES[slot.scale_polarity as usize].to_string(),
            scale_shape: SHAPE_NAMES[slot.scale_shape as usize].to_string(),
            enabled: slot.enabled,
        });
    }
    out
}

/// Whether a layer is the factory patch exactly — the test for "there is nothing
/// here worth writing". Params compare by value across the whole block (they are
/// plain `f32`s, and an exact compare is the right one: a param nudged and
/// nudged back *is* the default again); the topology compares whole.
fn is_factory_layer(layer: &LayerState) -> bool {
    let factory = LayerState::factory_default();
    layer.matrix == factory.matrix
        && (0..PARAMS.len()).all(|i| layer.params.get_index(i) == factory.params.get_index(i))
}

/// The keyboard record to write, or `None` when it is entirely default and can
/// be left out. `split_enabled` is only meaningful with Layer 2 on, so a patch
/// with Layer 2 off and nothing else touched writes no `[keys]` at all.
fn keys_file(key: &KeyState) -> Option<KeysFile> {
    if !key.layer2_on && key.split_point == DEFAULT_SPLIT_POINT && !key.lfo2_link {
        return None;
    }
    Some(KeysFile {
        mode: KEY_MODE_NAMES[key.key_mode() as usize].to_string(),
        split_point: key.split_point,
        lfo2_link: key.lfo2_link,
    })
}

/// Serialise a whole [`PluginState`] + metadata to a sparse TOML preset. Layer 2
/// and the keyboard record are written only when they deviate from the factory
/// default, so a single-layer patch produces the same file it did before 0221.
pub fn write_preset(meta: &Meta, state: &PluginState) -> Result<String, String> {
    let l2 = &state.layers[1];
    let file = PresetFile {
        schema: SCHEMA,
        meta: meta.clone(),
        params: params_table(&state.layers[0].params),
        matrix: matrix_rows(&state.layers[0].matrix),
        layer2: (!is_factory_layer(l2)).then(|| LayerFile {
            params: params_table(&l2.params),
            matrix: matrix_rows(&l2.matrix),
        }),
        keys: keys_file(&state.key),
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

/// Parse one layer's `params` table + `matrix` rows into a [`LayerState`].
/// Unspecified params fall back to their descriptor default; unspecified slots
/// are inert. `where_` prefixes the unknown-key warnings (`"params"` for Layer 1,
/// `"layer2.params"` for Layer 2) so a warning names the layer it came from.
///
/// The returned [`MatrixTable`] has depths seeded from the parsed params (the
/// param block is the depth authority), so the topology is render-ready.
fn parse_layer(
    params_table: &toml::Table,
    rows: &[MatrixRowFile],
    where_: &str,
    warnings: &mut Vec<String>,
) -> LayerState {
    // Params start at descriptor defaults; sparse keys override. `set` clamps.
    let mut params = Params::default();
    for (key, val) in params_table {
        match ParamId::from_name(key) {
            Some(id) => {
                if let Some(v) = parse_value(id.desc(), key, val, warnings) {
                    params.set(id, v);
                }
            }
            // Pre-0266 patches carry `assign_mode`, the four-way enum that
            // `stack_width` × `voice_mode` replaced (ADR 0003). Translate rather
            // than warn: the four old modes are exactly four points in the new
            // space, so nothing is lost and no patch loses its voicing.
            None if key == "assign_mode" => match legacy_assign_mode(val) {
                Some((width, mode)) => {
                    params.set(ParamId::StackWidth, width);
                    params.set(ParamId::VoiceMode, mode);
                }
                None => warnings.push(format!(
                    "{where_}: unrecognised legacy `assign_mode` value {val} (ignored)"
                )),
            },
            None => warnings.push(format!("{where_}: unknown parameter `{key}` (skipped)")),
        }
    }

    // Matrix starts all-inert; each routed row sets one slot's topology.
    let mut matrix = MatrixTable::default();
    for row in rows {
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
        let scale_polarity = name_to_u8(&POLARITY_NAMES, &row.scale_polarity).unwrap_or_else(|| {
            warnings.push(format!(
                "matrix slot {}: unknown scale polarity `{}` (using direct)",
                row.slot, row.scale_polarity
            ));
            0
        });
        let scale_shape = name_to_u8(&SHAPE_NAMES, &row.scale_shape).unwrap_or_else(|| {
            warnings.push(format!(
                "matrix slot {}: unknown scale shape `{}` (using lin)",
                row.slot, row.scale_shape
            ));
            0
        });
        let (polarity, shape) = curve_split(curve);
        matrix.slots[slot] = MatrixSlot {
            source: SourceId::from_u8(source),
            dest: DestId::from_u8(dest),
            polarity,
            shape,
            // A row exists in the file only because it was routed, and every
            // preset written before the toggle meant "on". Absent key → on.
            enabled: row.enabled,
            depth: 0.0, // seeded from params below
            scale_src: SourceId::from_u8(scale_src),
            scale_polarity: Polarity::from_u8(scale_polarity),
            scale_shape: Shape::from_u8(scale_shape),
        };
    }
    // Depth authority is the param block — seed every slot from it.
    for (i, slot) in matrix.slots.iter_mut().enumerate() {
        slot.depth = params.slot_depth(i);
    }
    // Every preset written before 0260 predates `Pan` as a destination, so it
    // says nothing about pan and would load with the spread knob inert. Seed
    // the default route when the patch has no opinion about pan at all; a patch
    // that routes Pan itself is left exactly as written. Seeded *after* the
    // depth pass so the route lands at its own 1.0, not at a stale slot depth.
    matrix.ensure_pan_route();

    LayerState { params, matrix }
}

/// Map a legacy `assign_mode` value — either its label or its enum index — onto
/// `(stack_width, voice_mode)` param values (ADR 0003):
///
/// | Old    | Width | Mode |
/// |--------|-------|------|
/// | Poly   | 1     | Poly |
/// | Unison | 16    | Solo |
/// | Solo   | 1     | Solo |
/// | Twin   | 2     | Poly |
///
/// Returns the two **param values** (enum indices), not the typed enums, since
/// that is what the sparse table stores.
fn legacy_assign_mode(val: &toml::Value) -> Option<(f32, f32)> {
    let name = match val {
        toml::Value::String(s) => s.to_ascii_lowercase(),
        toml::Value::Integer(i) => match i {
            0 => "poly".into(),
            1 => "unison".into(),
            2 => "solo".into(),
            3 => "twin".into(),
            _ => return None,
        },
        _ => return None,
    };
    // Width index: One=0, Two=1, Four=2, Eight=3, Sixteen=4. Mode: Poly=0, Solo=1.
    match name.as_str() {
        "poly" => Some((0.0, 0.0)),
        "unison" => Some((4.0, 1.0)),
        "solo" => Some((0.0, 1.0)),
        "twin" => Some((1.0, 0.0)),
        _ => None,
    }
}

/// Decode the `[keys]` section into a [`KeyState`]. An unknown mode label warns
/// and falls back to `Single`. Absent section → the default (Layer 2 off).
fn parse_keys(keys: &Option<KeysFile>, warnings: &mut Vec<String>) -> KeyState {
    let Some(k) = keys else {
        return KeyState::default();
    };
    let mode = KEY_MODE_NAMES
        .iter()
        .position(|n| n.eq_ignore_ascii_case(k.mode.trim()))
        .unwrap_or_else(|| {
            warnings.push(format!("keys.mode: unknown mode `{}` (using single)", k.mode));
            0
        });
    // Route the mode through `KeyState::apply` rather than reimplementing the
    // mode→toggles map, so the file and the UI opcode can never disagree about
    // what "dual" means.
    let mut key = KeyState {
        split_point: k.split_point,
        lfo2_link: k.lfo2_link,
        ..KeyState::default()
    };
    key.apply(KeyOp::SetKeyMode(mode as u8));
    key
}

/// Parse a TOML preset into `(meta, state, warnings)`. Unknown keys / bad enum
/// labels / type mismatches each fall back to a default and emit a non-fatal
/// warning. Only a malformed envelope is a hard [`PresetError`].
///
/// A file with no `[layer2]` / `[keys]` — every pre-0221 preset — yields Layer 2
/// at the factory patch with the keyboard in `Single`, so it plays exactly as it
/// did when it was written.
pub fn read_preset(s: &str) -> Result<(Meta, PluginState, Vec<String>), PresetError> {
    let header: Header = toml::from_str(s)?;
    if header.schema != SCHEMA {
        return Err(PresetError::UnsupportedSchema {
            found: header.schema,
            expected: SCHEMA,
        });
    }

    let file: PresetFile = toml::from_str(s)?;
    let mut warnings = Vec::new();

    let layer1 = parse_layer(&file.params, &file.matrix, "params", &mut warnings);
    let layer2 = match &file.layer2 {
        Some(l) => parse_layer(&l.params, &l.matrix, "layer2.params", &mut warnings),
        None => LayerState::factory_default(),
    };
    let key = parse_keys(&file.keys, &mut warnings);

    Ok((
        file.meta,
        PluginState { layers: [layer1, layer2], key },
        warnings,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::KeyMode;
    // Only the tests build slots with an explicit polarity; the codec reaches
    // the axis through `curve_code` / `curve_split` and never names a variant.
    use crate::params::{StackWidth, TOTAL_PARAMS, VoiceMode};

    fn meta(name: &str) -> Meta {
        Meta {
            name: name.to_string(),
            ..Meta::default()
        }
    }

    /// A layer that deviates from the factory patch in params and topology.
    fn sample_layer() -> LayerState {
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
            polarity: Polarity::Direct,
            shape: Shape::Lin,
            enabled: true,
            scale_polarity: Polarity::Direct,
            scale_shape: Shape::Lin,
            scale_src: SourceId::None,
        };
        matrix.slots[2] = MatrixSlot {
            source: SourceId::Lfo1,
            dest: DestId::Pitch,
            depth: -0.25,
            polarity: Polarity::Bipolar,
            shape: Shape::Lin,
            enabled: true,
            scale_polarity: Polarity::Direct,
            scale_shape: Shape::Lin,
            scale_src: SourceId::ModWheel,
        };
        LayerState { params, matrix }
    }

    /// Single-layer patch: Layer 1 edited, Layer 2 factory, keyboard default —
    /// the shape every pre-0221 preset had.
    fn sample_state() -> PluginState {
        PluginState {
            layers: [sample_layer(), LayerState::factory_default()],
            key: KeyState::default(),
        }
    }

    /// A split patch: two distinct layers and a non-default keyboard record.
    fn dual_state() -> PluginState {
        let mut l2 = LayerState::factory_default();
        l2.params.set(ParamId::Cutoff, 220.0);
        l2.params.set(ParamId::MatrixSlot5Depth, 0.75);
        l2.matrix.slots[5] = MatrixSlot {
            source: SourceId::Lfo2,
            dest: DestId::Cutoff,
            depth: 0.75,
            polarity: Polarity::Direct,
            shape: Shape::Lin,
            enabled: true,
            scale_polarity: Polarity::Direct,
            scale_shape: Shape::Lin,
            scale_src: SourceId::None,
        };
        PluginState {
            layers: [sample_layer(), l2],
            key: KeyState {
                layer2_on: true,
                split_enabled: true,
                split_point: 48,
                lfo2_link: true,
            },
        }
    }

    /// 0248: pan is an ordinary named param, so it rides the sparse table like
    /// any other — written only when it differs from centre, and negative
    /// (left) values survive the text round trip.
    #[test]
    fn layer_pan_round_trips_and_stays_sparse_at_centre() {
        let mut st = sample_state();
        st.layers[0].params.set(ParamId::LayerPan, -0.75);
        // Detune rides the same sparse table (0263) — a cents value, so the
        // taper is irrelevant here: the file carries plain units.
        st.layers[0].params.set(ParamId::LayerDetune, -7.5);
        let toml = write_preset(&meta("Pan"), &st).unwrap();
        assert!(toml.contains("layer_pan"), "a moved pan must be written:\n{toml}");
        let (_, back, warnings) = read_preset(&toml).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(back.layers[0].params.get(ParamId::LayerPan), -0.75);
        assert_eq!(back.layers[0].params.get(ParamId::LayerDetune), -7.5);

        // Centre is the descriptor default, so it stays out of the file and is
        // adopted on read — the sparse-format contract.
        let toml = write_preset(&meta("Centre"), &sample_state()).unwrap();
        assert!(!toml.contains("layer_pan"), "centre must not be written:\n{toml}");
        assert!(!toml.contains("layer_detune"), "zero detune must not be written:\n{toml}");
        let (_, back, _) = read_preset(&toml).unwrap();
        assert_eq!(back.layers[0].params.get(ParamId::LayerPan), 0.0);
        assert_eq!(back.layers[0].params.get(ParamId::LayerDetune), 0.0);
    }

    /// 0260: a preset written before pan was a destination says nothing about
    /// it, and would load with the Spread knob inert. The loader seeds the
    /// default route in that case — and only in that case.
    #[test]
    fn a_legacy_preset_gains_the_default_pan_route_on_load() {
        let s = r#"
schema = 1
[meta]
name = "Legacy"
[params]
cutoff = 900.0
"#;
        let (_m, st, warnings) = read_preset(s).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let routes: Vec<_> = st.layers[0]
            .matrix
            .slots
            .iter()
            .filter(|r| r.dest == DestId::Pan)
            .collect();
        assert_eq!(routes.len(), 1, "exactly one seeded pan route");
        assert_eq!(routes[0].source, SourceId::Spread);
        assert_eq!(routes[0].depth, 1.0, "seeded at unity, not at a stale slot depth");
        // Layer 2 defaults to the factory patch, which already carries it.
        assert!(st.layers[1].matrix.slots.iter().any(|r| r.dest == DestId::Pan));
    }

    /// A patch that routes Pan itself is not second-guessed.
    #[test]
    fn a_preset_with_its_own_pan_route_is_left_alone() {
        let s = r#"
schema = 1
[meta]
name = "Auto-pan"
[params]
matrix_slot0_depth = 0.5
[[matrix]]
slot = 0
source = "lfo1"
dest = "pan"
"#;
        let (_m, st, warnings) = read_preset(s).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let routes: Vec<_> = st.layers[0]
            .matrix
            .slots
            .iter()
            .filter(|r| r.dest == DestId::Pan)
            .collect();
        assert_eq!(routes.len(), 1, "no second route bolted on");
        assert_eq!(routes[0].source, SourceId::Lfo1);
        assert_eq!(routes[0].depth, 0.5, "depth still comes from the param block");
    }

    /// Pan/Spread survive the text format like any other topology.
    #[test]
    fn pan_route_round_trips_through_text() {
        let mut st = sample_state();
        st.layers[0].params.set(ParamId::MatrixSlot7Depth, 0.6);
        st.layers[0].matrix.slots[7] = MatrixSlot {
            source: SourceId::Spread,
            dest: DestId::Pan,
            depth: 0.6,
            polarity: Polarity::Direct,
            shape: Shape::Lin,
            enabled: true,
            scale_polarity: Polarity::Direct,
            scale_shape: Shape::Lin,
            scale_src: SourceId::None,
        };
        let toml = write_preset(&meta("PanRT"), &st).unwrap();
        assert!(toml.contains("\"spread\""), "source name on the wire:\n{toml}");
        assert!(toml.contains("\"pan\""), "dest name on the wire:\n{toml}");
        let (_, back, warnings) = read_preset(&toml).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(back.layers[0].matrix.slots[7].source, SourceId::Spread);
        assert_eq!(back.layers[0].matrix.slots[7].dest, DestId::Pan);
        assert_eq!(back.layers[0].matrix.slots[7].depth, 0.6);
    }

    /// 0266 / ADR 0003: the four legacy assign modes are four points in the
    /// (width, mode) space, so a pre-0266 preset keeps its voicing instead of
    /// warning and silently falling back to Poly.
    #[test]
    fn legacy_assign_mode_maps_onto_width_and_voice_mode() {
        for (label, width, mode) in [
            ("Poly", StackWidth::One, VoiceMode::Poly),
            ("Twin", StackWidth::Two, VoiceMode::Poly),
            ("Solo", StackWidth::One, VoiceMode::Solo),
            ("Unison", StackWidth::Sixteen, VoiceMode::Solo),
        ] {
            let src = format!(
                "schema = 1\n[meta]\nname = \"Legacy\"\n[params]\nassign_mode = \"{label}\"\n"
            );
            let (_m, st, warnings) = read_preset(&src).unwrap();
            assert!(warnings.is_empty(), "{label}: {warnings:?}");
            assert_eq!(st.layers[0].params.stack_width(), width, "{label} width");
            assert_eq!(st.layers[0].params.voice_mode(), mode, "{label} mode");
        }
    }

    /// An unrecognised legacy value warns rather than silently voicing wrong.
    #[test]
    fn unrecognised_legacy_assign_mode_warns() {
        let src = "schema = 1\n[meta]\nname = \"X\"\n[params]\nassign_mode = \"Chorded\"\n";
        let (_m, _st, warnings) = read_preset(src).unwrap();
        assert!(
            warnings.iter().any(|w| w.contains("assign_mode")),
            "{warnings:?}"
        );
    }

    #[test]
    fn topology_round_trips_through_text() {
        let st = sample_state();
        let toml = write_preset(&meta("RT"), &st).unwrap();
        let (m, back, warnings) = read_preset(&toml).unwrap();
        assert_eq!(m.name, "RT");
        assert!(warnings.is_empty(), "{warnings:?}");

        let l1 = &back.layers[0];
        assert_eq!(l1.params.get(ParamId::Cutoff), 1234.0);
        assert_eq!(l1.params.get(ParamId::Osc1Wave), 3.0);

        let s0 = l1.matrix.slots[0];
        assert_eq!(s0.source, SourceId::Env2);
        assert_eq!(s0.dest, DestId::Amp);
        assert_eq!((s0.polarity, s0.shape), (Polarity::Direct, Shape::Lin));
        assert_eq!(s0.scale_src, SourceId::None);
        assert_eq!(s0.depth, 1.0); // from the param

        let s2 = l1.matrix.slots[2];
        assert_eq!(s2.source, SourceId::Lfo1);
        assert_eq!(s2.dest, DestId::Pitch);
        // Legacy spelling `bipolar` still decodes to the bipolar polarity with
        // a linear bend — the property `curve_code` exists to preserve.
        assert_eq!((s2.polarity, s2.shape), (Polarity::Bipolar, Shape::Lin));
        assert_eq!(s2.scale_src, SourceId::ModWheel);
        assert_eq!(s2.depth, -0.25);
    }

    #[test]
    fn both_layers_and_keys_round_trip() {
        // 0221 acceptance: a dual/split preset survives save → reload with both
        // patches and the keyboard record intact.
        let st = dual_state();
        let toml = write_preset(&meta("Split"), &st).unwrap();
        let (_m, back, warnings) = read_preset(&toml).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");

        // Layer 1 kept its own patch...
        assert_eq!(back.layers[0].params.get(ParamId::Cutoff), 1234.0);
        assert_eq!(back.layers[0].matrix.slots[0].source, SourceId::Env2);
        // ...and Layer 2 its own, in a slot Layer 1 leaves inert.
        assert_eq!(back.layers[1].params.get(ParamId::Cutoff), 220.0);
        assert_eq!(back.layers[1].matrix.slots[5].source, SourceId::Lfo2);
        assert_eq!(back.layers[1].matrix.slots[5].dest, DestId::Cutoff);
        assert_eq!(back.layers[1].matrix.slots[5].depth, 0.75);
        assert!(!back.layers[0].matrix.slots[5].is_active());

        assert_eq!(back.key.key_mode(), KeyMode::Split);
        assert_eq!(back.key.split_point, 48);
        assert!(back.key.lfo2_link);
    }

    #[test]
    fn dual_mode_without_split_round_trips() {
        let mut st = dual_state();
        st.key.split_enabled = false;
        let toml = write_preset(&meta("Dual"), &st).unwrap();
        let (_m, back, _w) = read_preset(&toml).unwrap();
        assert_eq!(back.key.key_mode(), KeyMode::Dual);
        // The point rides along even with the split off, so arming it later
        // lands where the patch author left it.
        assert_eq!(back.key.split_point, 48);
    }

    #[test]
    fn single_layer_patch_writes_no_layer2_or_keys() {
        // The migration contract in the other direction: a single-layer patch
        // still saves as a single-layer file, so nothing in an existing bank
        // grows a `[layer2]` just by being re-saved.
        let toml = write_preset(&meta("Single"), &sample_state()).unwrap();
        assert!(!toml.contains("[layer2"), "{toml}");
        assert!(!toml.contains("[keys]"), "{toml}");
    }

    #[test]
    fn legacy_single_layer_preset_loads_as_layer1_plus_factory_layer2() {
        // A pre-0221 file: no `[layer2]`, no `[keys]`. It must load with Layer 1
        // exactly as written, Layer 2 at the factory patch, and the keyboard in
        // Single — i.e. sounding exactly as it did before the format changed.
        let legacy = r#"
schema = 1
[meta]
name = "Legacy"
[params]
cutoff = 800.0
[[matrix]]
slot = 1
source = "lfo1"
dest = "cutoff"
"#;
        let (m, st, warnings) = read_preset(legacy).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(m.name, "Legacy");
        assert_eq!(st.layers[0].params.get(ParamId::Cutoff), 800.0);
        assert_eq!(st.layers[0].matrix.slots[1].source, SourceId::Lfo1);

        assert_eq!(st.key, KeyState::default());
        assert_eq!(st.key.key_mode(), KeyMode::Single);
        // Layer 2 is the factory patch, param block and topology alike.
        assert!(is_factory_layer(&st.layers[1]));
        assert_eq!(st.layers[1].matrix, LayerState::factory_default().matrix);
    }

    #[test]
    fn keys_without_layer2_is_valid() {
        // Enabling Layer 2 on the factory patch writes no `[layer2]` (nothing
        // deviates), so `[keys]` must stand alone.
        let s = r#"
schema = 1
[meta]
name = "K"
[keys]
mode = "dual"
"#;
        let (_m, st, warnings) = read_preset(s).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(st.key.key_mode(), KeyMode::Dual);
        assert_eq!(st.key.split_point, DEFAULT_SPLIT_POINT);
        assert!(is_factory_layer(&st.layers[1]));
    }

    #[test]
    fn partial_keys_section_defaults_the_rest() {
        let s = r#"
schema = 1
[meta]
name = "K"
[keys]
split-point = 36
"#;
        let (_m, st, warnings) = read_preset(s).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(st.key.key_mode(), KeyMode::Single);
        assert_eq!(st.key.split_point, 36);
        assert!(!st.key.lfo2_link);
    }

    #[test]
    fn unknown_key_mode_warns_and_falls_back_to_single() {
        let s = r#"
schema = 1
[meta]
name = "K"
[keys]
mode = "quadruple"
"#;
        let (_m, st, warnings) = read_preset(s).unwrap();
        assert!(warnings.iter().any(|w| w.contains("unknown mode")), "{warnings:?}");
        assert_eq!(st.key.key_mode(), KeyMode::Single);
    }

    #[test]
    fn layer2_warnings_name_their_layer() {
        let s = r#"
schema = 1
[meta]
name = "W"
[layer2.params]
not_a_param = 5.0
"#;
        let (_m, _st, warnings) = read_preset(s).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].starts_with("layer2.params:"), "{warnings:?}");
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
    fn layer2_section_is_sparse_too() {
        let toml = write_preset(&meta("L2"), &dual_state()).unwrap();
        let doc: toml::Table = toml::from_str(&toml).unwrap();
        let l2 = doc.get("layer2").and_then(|v| v.as_table()).unwrap();
        let params = l2.get("params").and_then(|v| v.as_table()).unwrap();
        assert!(params.contains_key("cutoff"));
        assert!(params.len() < TOTAL_PARAMS, "expected sparse, got {}", params.len());
        // Layer 2 starts from the factory patch, so its rows are the factory Amp
        // route plus the one this patch adds.
        let rows = l2.get("matrix").and_then(|v| v.as_array()).unwrap();
        assert!(rows.iter().any(|r| r.get("slot").and_then(|v| v.as_integer()) == Some(5)));
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
        st.layers[0].matrix.slots[2].scale_src = SourceId::None;
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
        let (_m, st, warnings) = read_preset(s).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let matrix = st.layers[0].matrix;
        assert_eq!(matrix.slots[0].source, SourceId::Lfo1);
        assert_eq!(matrix.slots[0].dest, DestId::Pitch);
        assert_eq!(
            (matrix.slots[0].polarity, matrix.slots[0].shape),
            (Polarity::Direct, Shape::Lin)
        );
        assert_eq!(matrix.slots[0].scale_src, SourceId::None);
        // Every preset written before 0341 says nothing about the scale VCA's
        // polarity, and `direct` is the fold it always applied — so an absent
        // key must load as the arithmetic the file was voiced against.
        assert_eq!(matrix.slots[0].scale_polarity, Polarity::Direct);
    }

    /// The VCA's polarity survives the text format, and is written only when it
    /// is off `direct` — so a patch with a plain scale VCA still round-trips
    /// byte-identically.
    #[test]
    fn scale_polarity_round_trips_and_stays_sparse_at_direct() {
        let mut st = sample_state();
        st.layers[0].matrix.slots[0].scale_src = SourceId::StackPos;
        st.layers[0].matrix.slots[0].scale_polarity = Polarity::Abs;
        let text = write_preset(&meta("SP"), &st).unwrap();
        assert!(text.contains("scale-polarity = \"abs\""), "{text}");

        let (_m, back, warnings) = read_preset(&text).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(back.layers[0].matrix.slots[0].scale_polarity, Polarity::Abs);

        // …and a `direct` VCA writes no key at all.
        st.layers[0].matrix.slots[0].scale_polarity = Polarity::Direct;
        let plain = write_preset(&meta("SP"), &st).unwrap();
        assert!(!plain.contains("scale-polarity"), "{plain}");
    }

    #[test]
    fn unknown_scale_polarity_degrades_to_direct_with_warning() {
        let s = r#"
schema = 1
[meta]
name = "U"
[[matrix]]
slot = 0
source = "lfo1"
dest = "pitch"
scale-src = "mod-wheel"
scale-polarity = "bogus"
"#;
        let (_m, st, warnings) = read_preset(s).unwrap();
        assert_eq!(
            st.layers[0].matrix.slots[0].scale_polarity,
            Polarity::Direct
        );
        assert!(
            warnings.iter().any(|w| w.contains("unknown scale polarity")),
            "{warnings:?}"
        );
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
        let (_m, st, warnings) = read_preset(s).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("unknown source"), "{warnings:?}");
        assert!(!st.layers[0].matrix.slots[2].is_active());
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
        let (_m, st, warnings) = read_preset(s).unwrap();
        assert_eq!(st.layers[0].matrix.slots[0].scale_src, SourceId::None);
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
        let (_m, st, warnings) = read_preset(s).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("not_a_param"), "{warnings:?}");
        assert_eq!(st.layers[0].params.get(ParamId::Cutoff), 800.0);
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
        let (_m, st, _w) = read_preset(s).unwrap();
        assert_eq!(st.layers[0].params.get(ParamId::Resonance), 1.0);
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
        let (_m, st, warnings) = read_preset(s).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(st.layers[0].params.get(ParamId::Osc1Wave), 3.0);
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
        let (_m, st, warnings) = read_preset(s).unwrap();
        assert!(warnings.iter().any(|w| w.contains("out of range")), "{warnings:?}");
        // The bad row is dropped; the only live slot is the `Spread → Pan`
        // route seeded on load for a patch that says nothing about pan.
        let live: Vec<_> = st.layers[0].matrix.slots.iter().filter(|s| s.is_active()).collect();
        assert_eq!(live.len(), 1, "{live:?}");
        assert_eq!(live[0].dest, DestId::Pan);
    }
}
