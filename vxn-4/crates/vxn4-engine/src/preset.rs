//! Portable VXN4 preset format (ticket 0383; E052, ADR 0005).
//!
//! A preset is a **sparse TOML** text file keyed by [`ParamDesc::name`] — never
//! by index, because the descriptor table will grow and a positional format
//! rots the first time it does. Only fields that deviate from their descriptor
//! default are written, enums store their variant **label** rather than their
//! discriminant, and numbers are in the descriptor's plain unit.
//!
//! ```text
//! schema = 1
//!
//! [meta]
//! name = "Glass Tine"
//!
//! [params]           every patch field that is off its default
//! [macros]           per-knob display labels, ranges and units
//! [[matrix]]         one row per wired slot — topology only
//! ```
//!
//! Pure main-thread mapping between a [`Patch`] and its file text: no IO (that
//! is 0385), no clap, no UI. The envelope — [`Meta`], [`Header`], [`SCHEMA`],
//! [`vxn_preset::value_for`], [`PresetError`] — comes from [`vxn_preset`]
//! rather than being spelled a third time.
//!
//! ## Sparse-vs-default has a sharp edge
//!
//! A field whose **default changes** silently changes every preset that did not
//! override it. That is the intended behaviour — it is how a bank adopts an
//! improved default, and it is why a preset written by a build with fewer
//! fields loads correctly into a build with more — but it makes changing a
//! default in [`crate::params`] a **user-visible act**, not an implementation
//! detail. Anything voiced against the old value has to be re-measured or
//! pinned by writing the old value into the file.
//!
//! ## What a preset is *not*
//!
//! Exactly the host region of the descriptor table ([`is_patch_field`] is the
//! predicate): the loaded-patch index, the oversampling quality, `master-gain`,
//! and the **eight macro positions**. A macro's position belongs to the song,
//! not to the sound, so loading a preset must not stamp on the player's knobs.
//! `gain` — the patch's own measured loudness trim — is a different param and
//! *is* patch state. A `[params]` key naming a host param is a warning, not an
//! error: the file is legible, it is just claiming something a patch cannot own.
//!
//! ## Topology and depth are split
//!
//! The [`Matrix`] is patch state a preset must carry **in full**: which routes
//! each macro drives, and how far, is the whole reason the CLAP surface can stay
//! at eleven params ([`crate::matrix`]). Its **topology** — source, dest, curve,
//! scale source, scale polarity, scale shape, the on/off switch — rides an
//! `[[matrix]]` array of tables keyed by kebab machine names. Its **depths do
//! not**: they are ordinary `matrix-NN-depth` descriptors and ride `[params]`,
//! deliberately not duplicated in the rows. This is vxn-1b's split
//! ([`vxn1b_engine::preset`], ADR 0001 §5) and the reason is the same — a depth
//! is a knob a player sweeps, and two authorities for one number is one too many.
//!
//! Only **wired** slots are written (both endpoints real, switch or no switch):
//! a route the player has switched off still has wiring worth saving, and
//! dropping it would make the toggle a destructive delete across a save/load.
//! An absent row, or one naming a source or dest this build has never heard of,
//! decodes **inert**.
//!
//! ## Macro labels are patch data, and live in `[macros]`
//!
//! The faceplate relabels and rescales all eight knobs when the preset changes,
//! so the labels have to travel with the sound. They are display strings rather
//! than values, so they get their own table rather than being smuggled into
//! `[params]` — keyed by the same `macro-1` … `macro-8` machine names the host
//! params carry, so the file names the same knob the same way twice over:
//!
//! ```text
//! [macros.macro-1]
//! label = "Detune"
//! max = 60.0
//! unit = " ct"
//! ```
//!
//! They are **not** a field of [`Patch`], and that is deliberate rather than
//! squeamish. A label is a `String`; a `Patch` that owned one could not cross to
//! the audio thread as a snapshot without the audio thread eventually *dropping*
//! it, which is an allocator call on the render path (0382). So the display
//! record rides beside the patch as [`Macros`] — same file, same load, different
//! side of the thread boundary. `[meta]` was the other candidate and is worse:
//! its shape is fixed by the shared crate, and a label set is per-knob data, not
//! the browser's name/author/category triple.
//!
//! ## Warnings, not failures
//!
//! Unknown keys, unknown enum labels, type mismatches and out-of-range slot
//! indices are **collected and returned** ([`Preset::warnings`]) — never silent,
//! never fatal. A patch that loads with three fields quietly at their defaults
//! is a support burden that looks like a synth bug; refusing the whole file over
//! one field a later build introduced is worse. Only a malformed envelope or an
//! unsupported `schema` is a [`PresetError`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use vxn_core_app::{ParamDesc, ParamKind};
use vxn_core_matrix::curve::{
    CURVE_NAMES, POLARITY_NAMES, Polarity, SHAPE_NAMES, Shape, curve_code, curve_split,
    polarity_from_name,
};
use vxn_core_matrix::slot::MatrixSlot;
use vxn_preset::ScalarKind;
pub use vxn_preset::{Header, Meta, PresetError, SCHEMA};

use vxn4_dsp::ops::{NOPS, OpConfig, Routing};

use crate::eg::EgParams;
use crate::matrix::{DEST_NAMES, DestId, Matrix, N_MACROS, N_MATRIX_SLOTS, SOURCE_NAMES, SourceId};
use crate::params::{
    desc, id_for_name, is_patch_field, macro_id, macro_index, patch_ids, set_value, value_of,
    variant_or_default,
};
use crate::patch::Patch;

// ── the macro display record ────────────────────────────────────────────────

/// One macro knob as the faceplate draws it: what it is called, what its travel
/// reads as, and in what unit.
///
/// Display only. The knob itself is always a `[0, 1]` host param and the matrix
/// is what turns it into an audible change; this record decides what the number
/// under it says. Nothing here reaches the audio thread, which is why it is not
/// part of [`Patch`] — see the module docs.
///
/// There is no taper column. The mockup's per-knob `exp` flag is a readout
/// nicety and the format is name-keyed and sparse, so adding one later is a new
/// optional key rather than a schema bump.
#[derive(Clone, Debug, PartialEq)]
pub struct MacroSpec {
    /// What the faceplate prints under the knob. **Empty means unassigned** —
    /// the patch does not use this macro, and the control greys.
    pub label: String,
    /// Value at the bottom of the knob's travel.
    pub min: f32,
    /// Value at the top.
    pub max: f32,
    /// Suffix on the readout, leading space and all (`" ct"`, `" Hz"`, `"%"`).
    pub unit: String,
}

impl Default for MacroSpec {
    /// An unassigned knob reading 0–100 %. The percentage is the mockup's own
    /// default readout, and an empty label is what tells the faceplate the patch
    /// has nothing wired to this knob.
    fn default() -> Self {
        Self {
            label: String::new(),
            min: 0.0,
            max: 100.0,
            unit: "%".to_string(),
        }
    }
}

/// The eight knobs' display records, in knob order.
pub type Macros = [MacroSpec; N_MACROS];

/// A decoded preset: everything a file carries, plus whatever the codec had to
/// substitute on the way in.
#[derive(Debug)]
pub struct Preset {
    pub meta: Meta,
    pub patch: Patch,
    /// The `[macros]` table. Knobs the file says nothing about are
    /// [`MacroSpec::default`] — unassigned.
    pub macros: Macros,
    /// Non-fatal substitutions, in file order. Empty for a clean load.
    pub warnings: Vec<String>,
}

// ── the file shape ──────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct PresetFile {
    schema: u32,
    meta: Meta,
    /// `name -> typed scalar`, resolved against the descriptor by hand below
    /// rather than by a derive, so an unknown key can warn instead of failing.
    #[serde(default, skip_serializing_if = "toml::Table::is_empty")]
    params: toml::Table,
    /// Keyed by `macro-1` … `macro-8`. Declared before `matrix` so the emitted
    /// TOML puts these tables above the array-of-tables rather than past it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    macros: BTreeMap<String, MacroRow>,
    /// Wired slots only; topology only. **No depth column** — see the module
    /// docs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    matrix: Vec<MatrixRow>,
}

/// One macro knob's display record in the file. Every column is optional and
/// omitted at its default, so a knob that only gets a name costs one line.
#[derive(Default, Serialize, Deserialize)]
struct MacroRow {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    min: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unit: Option<String>,
}

/// One wired matrix slot. `source`/`dest` are required kebab machine names;
/// everything else defaults to the inert setting and is omitted there, so a
/// plain unscaled linear route is three lines.
#[derive(Serialize, Deserialize)]
struct MatrixRow {
    slot: u8,
    source: String,
    dest: String,
    /// The flat `(polarity, shape)` code, spelled as a [`CURVE_NAMES`] name.
    #[serde(default = "default_lin", skip_serializing_if = "is_lin")]
    curve: String,
    #[serde(
        rename = "scale-src",
        default = "default_none",
        skip_serializing_if = "is_none"
    )]
    scale_src: String,
    #[serde(
        rename = "scale-polarity",
        default = "default_none",
        skip_serializing_if = "is_none"
    )]
    scale_polarity: String,
    #[serde(
        rename = "scale-shape",
        default = "default_lin",
        skip_serializing_if = "is_lin"
    )]
    scale_shape: String,
    /// The player's on/off switch. Absent → `true`: a row exists only because
    /// the slot was wired, and a wired slot means to sound unless it says
    /// otherwise. Omitted when on, so an all-on patch is unchanged on disk.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    enabled: bool,
}

fn default_lin() -> String {
    "lin".to_string()
}

fn default_none() -> String {
    "none".to_string()
}

fn default_true() -> bool {
    true
}

fn is_lin(s: &str) -> bool {
    s == "lin"
}

fn is_none(s: &str) -> bool {
    s == "none"
}

fn is_true(b: &bool) -> bool {
    *b
}

// ── write ───────────────────────────────────────────────────────────────────

/// One param value as a typed TOML scalar, mapping this engine's [`ParamKind`]
/// onto the shared [`ScalarKind`].
fn value_for(d: &ParamDesc, v: f32) -> toml::Value {
    let kind = match d.kind {
        ParamKind::Enum { variants } => ScalarKind::Enum { variants },
        ParamKind::Bool => ScalarKind::Bool,
        ParamKind::Int { .. } => ScalarKind::Int,
        ParamKind::Float { .. } => ScalarKind::Float,
    };
    vxn_preset::value_for(kind, v)
}

/// The sparse `[params]` table: every patch field that is off its default, slot
/// depths included — they are ordinary params.
fn params_table(patch: &Patch) -> toml::Table {
    let mut t = toml::Table::new();
    for id in patch_ids() {
        let (Some(d), Some(v)) = (desc(id), value_of(patch, id)) else {
            continue;
        };
        // An exact compare is the right one: a field nudged and nudged back
        // *is* the default again, and writing it would pin a value the patch
        // has no opinion about.
        if v != d.default {
            t.insert(d.name.to_string(), value_for(d, v));
        }
    }
    t
}

/// The `[macros]` tables: one per knob that says anything beyond the default.
fn macro_rows(macros: &Macros) -> BTreeMap<String, MacroRow> {
    let blank = MacroSpec::default();
    let mut out = BTreeMap::new();
    for (m, spec) in macros.iter().enumerate() {
        if *spec == blank {
            continue;
        }
        let name = desc(macro_id(m)).expect("a macro has a descriptor").name;
        out.insert(
            name.to_string(),
            MacroRow {
                label: spec.label.clone(),
                min: (spec.min != blank.min).then_some(spec.min),
                max: (spec.max != blank.max).then_some(spec.max),
                unit: (spec.unit != blank.unit).then(|| spec.unit.clone()),
            },
        );
    }
    out
}

/// The `[[matrix]]` rows: one per **wired** slot, topology only.
fn matrix_rows(matrix: &Matrix) -> Vec<MatrixRow> {
    matrix
        .slots
        .iter()
        .enumerate()
        .filter(|(_, s)| s.is_wired())
        .map(|(i, s)| MatrixRow {
            slot: i as u8,
            source: SOURCE_NAMES[s.source as usize].to_string(),
            dest: DEST_NAMES[s.dest as usize].to_string(),
            curve: CURVE_NAMES[curve_code(s.polarity, s.shape) as usize].to_string(),
            scale_src: SOURCE_NAMES[s.scale_src as usize].to_string(),
            scale_polarity: POLARITY_NAMES[s.scale_polarity as usize].to_string(),
            scale_shape: SHAPE_NAMES[s.scale_shape as usize].to_string(),
            enabled: s.enabled,
        })
        .collect()
}

/// Serialise a patch, its macro labels and its metadata to sparse TOML.
///
/// The output is deterministic — `[params]` and `[macros]` are ordered maps and
/// the rows follow slot order — so writing the same patch twice produces the
/// same bytes, which is what makes a "has this preset changed?" check a string
/// compare.
pub fn write_preset(meta: &Meta, patch: &Patch, macros: &Macros) -> Result<String, String> {
    let file = PresetFile {
        schema: SCHEMA,
        meta: meta.clone(),
        params: params_table(patch),
        macros: macro_rows(macros),
        matrix: matrix_rows(&patch.matrix),
    };
    // Values are clamped to finite ranges and every label comes from a static
    // table, so serialising this shape cannot fail.
    toml::to_string_pretty(&file).map_err(|e| e.to_string())
}

// ── read ────────────────────────────────────────────────────────────────────

/// A patch with every field at its descriptor default — what a preset that says
/// nothing at all decodes to, and the base every sparse file is laid over.
///
/// Built by driving [`set_value`] from the table rather than by hand, so
/// "absent means the descriptor default" is true by construction rather than by
/// two lists agreeing.
pub fn default_patch() -> Patch {
    let mut p = Patch {
        // The patch's display name lives in `[meta]`, which is the browser's
        // business and a `String`; `Patch::name` is the factory bank's
        // `&'static str` label and there is nothing honest to put in it here.
        name: "",
        ops: [OpConfig::default(); NOPS],
        routing: Routing::default(),
        matrix: Matrix::default(),
        eg: [EgParams::default(); NOPS],
        gain: 1.0,
    };
    for id in patch_ids() {
        if let Some(d) = desc(id) {
            set_value(&mut p, id, d.default);
        }
    }
    p
}

/// Resolve one TOML scalar to a plain-unit `f32` for `d`. On a type mismatch or
/// an unknown enum label, warn and fall back — `None` leaves the default in
/// place, `Some` carries a substituted value.
fn parse_value(
    d: &ParamDesc,
    key: &str,
    val: &toml::Value,
    warnings: &mut Vec<String>,
) -> Option<f32> {
    match d.kind {
        ParamKind::Enum { .. } => match val.as_str() {
            Some(s) => {
                if d.variant_index(s).is_none() {
                    warnings.push(format!(
                        "params.{key}: unknown enum label `{s}` (using default)"
                    ));
                }
                // The fallback itself is the table's, so a later build's
                // waveform name lands on this build's default rather than
                // refusing the file.
                Some(variant_or_default(d, s))
            }
            None => {
                warnings.push(format!(
                    "params.{key}: expected a string label (using default)"
                ));
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
            if let Some(f) = val.as_float() {
                Some(f as f32)
            } else if let Some(i) = val.as_integer() {
                Some(i as f32)
            } else {
                warnings.push(format!("params.{key}: expected a number (using default)"));
                None
            }
        }
    }
}

/// Look a kebab machine name up in a matrix label table, returning its `u8`
/// discriminant. Case-insensitive, since a preset is a text file people edit.
fn name_to_u8(table: &[&str], name: &str) -> Option<u8> {
    let lc = name.trim();
    table
        .iter()
        .position(|n| n.eq_ignore_ascii_case(lc))
        .map(|i| i as u8)
}

fn parse_params(table: &toml::Table, patch: &mut Patch, warnings: &mut Vec<String>) {
    for (key, val) in table {
        match id_for_name(key) {
            Some(id) if is_patch_field(id) => {
                let d = desc(id).expect("a resolved id has a descriptor");
                if let Some(v) = parse_value(d, key, val, warnings) {
                    set_value(patch, id, v);
                }
            }
            // A real name, in the wrong half of the table. The file is legible;
            // it is claiming performance state as patch state, which a preset
            // must not do — loading it would move the player's knobs.
            Some(_) => warnings.push(format!(
                "params.{key}: `{key}` is host state, not patch state (skipped)"
            )),
            None => warnings.push(format!("params: unknown parameter `{key}` (skipped)")),
        }
    }
}

fn parse_macros(
    rows: &BTreeMap<String, MacroRow>,
    macros: &mut Macros,
    warnings: &mut Vec<String>,
) {
    let blank = MacroSpec::default();
    for (key, row) in rows {
        // The `[macros]` keys are exactly the host params' machine names, so
        // the lookup is the descriptor table's rather than a second one.
        let Some(m) = id_for_name(key).and_then(macro_index) else {
            warnings.push(format!("macros: unknown macro knob `{key}` (skipped)"));
            continue;
        };
        macros[m] = MacroSpec {
            label: row.label.clone(),
            min: row.min.unwrap_or(blank.min),
            max: row.max.unwrap_or(blank.max),
            unit: row.unit.clone().unwrap_or_else(|| blank.unit.clone()),
        };
    }
}

fn parse_matrix(rows: &[MatrixRow], matrix: &mut Matrix, warnings: &mut Vec<String>) {
    for row in rows {
        let slot = row.slot as usize;
        if slot >= N_MATRIX_SLOTS {
            warnings.push(format!("matrix: slot {} out of range (skipped)", row.slot));
            continue;
        }
        // A source or dest this build does not know leaves the slot inert
        // rather than guessing at a neighbour: a route landing on the wrong
        // destination is worse than a route that does nothing.
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
        // The shaping columns are different: an unknown bend is a wrong-sounding
        // route, not a wrong-wired one, so it degrades to the identity and warns.
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
        // `polarity_from_name`, not `name_to_u8`: the resting map was spelled
        // `direct` before 0340 and both spellings name the same range map.
        let scale_polarity = polarity_from_name(&row.scale_polarity).unwrap_or_else(|| {
            warnings.push(format!(
                "matrix slot {}: unknown scale polarity `{}` (using none)",
                row.slot, row.scale_polarity
            ));
            Polarity::None
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
            enabled: row.enabled,
            // Depth is the param block's; the row carries none. Seeded already
            // by `parse_params`, so re-read it rather than zeroing it here.
            depth: matrix.slots[slot].depth,
            scale_src: SourceId::from_u8(scale_src),
            scale_polarity,
            scale_shape: Shape::from_u8(scale_shape),
        };
    }
}

/// Parse a TOML preset into a [`Preset`].
///
/// Unspecified fields take their descriptor defaults and unspecified slots are
/// inert, so a file written by a build with fewer fields loads into one with
/// more. Unknown keys, unknown labels and type mismatches each fall back and
/// emit a warning; only a malformed envelope or an unsupported `schema` is an
/// error.
pub fn read_preset(s: &str) -> Result<Preset, PresetError> {
    // The schema probe runs first and on its own: the body's shape is only
    // meaningful once the version says this build understands it.
    let header: Header = toml::from_str(s)?;
    if header.schema != SCHEMA {
        return Err(PresetError::UnsupportedSchema {
            found: header.schema,
            expected: SCHEMA,
        });
    }

    let file: PresetFile = toml::from_str(s)?;
    let mut warnings = Vec::new();
    let mut patch = default_patch();
    let mut macros = Macros::default();

    // Params first, so the topology pass finds each slot's depth already in
    // place and the two halves of a slot never disagree about which is
    // authoritative.
    parse_params(&file.params, &mut patch, &mut warnings);
    parse_macros(&file.macros, &mut macros, &mut warnings);
    parse_matrix(&file.matrix, &mut patch.matrix, &mut warnings);

    Ok(Preset {
        meta: file.meta,
        patch,
        macros,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::engine::Engine;
    use crate::params::{N_MACROS as PARAM_MACROS, PATCH_PARAMS};
    use crate::patch::{N_PATCHES, patch};

    fn meta(name: &str) -> Meta {
        Meta {
            name: name.to_string(),
            ..Meta::default()
        }
    }

    fn write(p: &Patch) -> String {
        write_preset(&meta(p.name), p, &Macros::default()).expect("a patch serialises")
    }

    /// Every patch field, compared **bitwise**. `assert_eq!` on `f32` would let
    /// a `-0.0` for a `0.0` through, and the render is not indifferent to that.
    fn assert_same_params(a: &Patch, b: &Patch, what: &str) {
        for id in patch_ids() {
            let (x, y) = (value_of(a, id).unwrap(), value_of(b, id).unwrap());
            assert_eq!(
                x.to_bits(),
                y.to_bits(),
                "{what}: {} was {x}, came back {y}",
                desc(id).unwrap().name
            );
        }
    }

    fn assert_same_topology(a: &Matrix, b: &Matrix, what: &str) {
        for (i, (x, y)) in a.slots.iter().zip(b.slots.iter()).enumerate() {
            assert_eq!(x, y, "{what}: slot {i} did not round-trip");
        }
    }

    // ── the format ──────────────────────────────────────────────────────────

    #[test]
    fn a_preset_writes_only_what_deviates_from_the_default() {
        let text = write(&patch(1)); // epiano
        let doc: toml::Table = toml::from_str(&text).unwrap();
        let params = doc["params"].as_table().unwrap();

        assert!(
            params.len() < PATCH_PARAMS / 2,
            "expected sparse, got {} of {PATCH_PARAMS}",
            params.len()
        );
        // The fields epiano actually authors...
        assert!(params.contains_key("op-3-ratio"), "{text}");
        assert!(params.contains_key("pm-0-1"), "{text}");
        assert!(params.contains_key("out-0"), "{text}");
        assert!(params.contains_key("gain"), "{text}");
        // ...and nothing about the silent operators it leaves alone.
        assert!(!params.contains_key("op-7-ratio"), "{text}");
        assert!(!params.contains_key("op-0-phase"), "{text}");
    }

    /// The host region is performance state. A preset that wrote it would move
    /// the player's knobs on load, which is the one thing loading a sound must
    /// not do.
    #[test]
    fn a_preset_never_writes_the_host_params() {
        for i in 0..N_PATCHES {
            let text = write(&patch(i));
            let doc: toml::Table = toml::from_str(&text).unwrap();
            let params = doc["params"].as_table().unwrap();
            for key in ["patch", "quality", "master-gain", "macro-1", "macro-8"] {
                assert!(
                    !params.contains_key(key),
                    "{key} is not patch state:\n{text}"
                );
            }
            // `gain` is the patch's own trim and is a different param.
            assert!(params.contains_key("gain"), "{text}");
        }
    }

    #[test]
    fn enum_fields_store_their_label_not_their_discriminant() {
        let text = write(&patch(3)); // saws — saw, square and triangle operators
        assert!(text.contains(r#"op-0-wave = "saw""#), "{text}");
        assert!(text.contains(r#"op-2-wave = "square""#), "{text}");
        assert!(text.contains(r#"op-3-wave = "triangle""#), "{text}");
    }

    #[test]
    fn depth_is_a_param_and_is_not_duplicated_in_the_topology_rows() {
        let text = write(&patch(0)); // sine — two routed slots, both at depth
        let doc: toml::Table = toml::from_str(&text).unwrap();
        assert!(
            doc["params"]
                .as_table()
                .unwrap()
                .contains_key("matrix-00-depth")
        );
        for row in doc["matrix"].as_array().unwrap() {
            assert!(
                row.get("depth").is_none(),
                "a row must not carry depth:\n{text}"
            );
        }
    }

    #[test]
    fn only_wired_slots_are_written() {
        let p = patch(0); // sine wires two of the 48 slots
        let text = write(&p);
        let doc: toml::Table = toml::from_str(&text).unwrap();
        let rows = doc["matrix"].as_array().unwrap();
        assert_eq!(
            rows.len(),
            p.matrix.slots.iter().filter(|s| s.is_wired()).count()
        );
        assert_eq!(rows.len(), 2, "{text}");
    }

    /// A route the player has switched off keeps its wiring: the toggle is not
    /// a delete.
    #[test]
    fn a_switched_off_route_is_still_written() {
        let mut p = patch(0);
        p.matrix.slots[0].enabled = false;
        let text = write(&p);
        assert!(text.contains("enabled = false"), "{text}");
        let back = read_preset(&text).unwrap();
        assert!(back.patch.matrix.slots[0].is_wired());
        assert!(!back.patch.matrix.slots[0].enabled);
        // ...and an on route writes no key at all.
        assert!(!write(&patch(0)).contains("enabled"));
    }

    #[test]
    fn writing_the_same_patch_twice_produces_the_same_bytes() {
        for i in 0..N_PATCHES {
            assert_eq!(write(&patch(i)), write(&patch(i)));
        }
    }

    // ── macros ──────────────────────────────────────────────────────────────

    #[test]
    fn macro_labels_ride_their_own_table_and_round_trip() {
        let mut macros = Macros::default();
        macros[0] = MacroSpec {
            label: "Detune".to_string(),
            min: 0.0,
            max: 60.0,
            unit: " ct".to_string(),
        };
        macros[5] = MacroSpec {
            label: "Damp".to_string(),
            ..MacroSpec::default()
        };

        let text = write_preset(&meta("Super"), &patch(6), &macros).unwrap();
        assert!(text.contains("[macros.macro-1]"), "{text}");
        assert!(text.contains(r#"label = "Detune""#), "{text}");
        assert!(text.contains(r#"unit = " ct""#), "{text}");
        // Macro 6 is only renamed, so its range and unit stay out of the file.
        assert!(text.contains("[macros.macro-6]"), "{text}");
        assert_eq!(text.matches("max =").count(), 1, "{text}");
        // Unassigned knobs write nothing at all.
        for m in [2, 3, 4, 7, 8] {
            assert!(!text.contains(&format!("[macros.macro-{m}]")), "{text}");
        }

        let back = read_preset(&text).unwrap();
        assert!(back.warnings.is_empty(), "{:?}", back.warnings);
        assert_eq!(back.macros, macros);
    }

    /// The `[macros]` table names the same eight knobs the host params do — but
    /// it carries their *labels*, never their positions.
    #[test]
    fn the_macro_table_is_keyed_by_the_host_param_names() {
        assert_eq!(PARAM_MACROS, N_MACROS);
        for m in 0..N_MACROS {
            let name = desc(macro_id(m)).unwrap().name;
            assert_eq!(id_for_name(name).and_then(macro_index), Some(m));
        }
        let mut macros = Macros::default();
        macros[7] = MacroSpec {
            label: "Air".to_string(),
            ..MacroSpec::default()
        };
        let text = write_preset(&meta("M"), &patch(0), &macros).unwrap();
        // The knob's *position* is nowhere in the file.
        let doc: toml::Table = toml::from_str(&text).unwrap();
        assert!(!doc["params"].as_table().unwrap().contains_key("macro-8"));
        assert!(doc["macros"].as_table().unwrap().contains_key("macro-8"));
    }

    #[test]
    fn an_unknown_macro_key_warns_and_is_skipped() {
        let s = r#"
schema = 1
[meta]
name = "X"
[macros.macro-9]
label = "Nope"
"#;
        let back = read_preset(s).unwrap();
        assert_eq!(back.warnings.len(), 1);
        assert!(back.warnings[0].contains("macro-9"), "{:?}", back.warnings);
        assert_eq!(back.macros, Macros::default());
    }

    // ── round trips ─────────────────────────────────────────────────────────

    #[test]
    fn every_factory_patch_round_trips_field_by_field() {
        for i in 0..N_PATCHES {
            let p = patch(i);
            let text = write(&p);
            let back = read_preset(&text).expect("a factory preset parses");
            assert!(back.warnings.is_empty(), "{}: {:?}", p.name, back.warnings);
            assert_eq!(back.meta.name, p.name);
            assert_same_params(&p, &back.patch, p.name);
            assert_same_topology(&p.matrix, &back.patch.matrix, p.name);
        }
    }

    /// Six notes, held then released, with the macros parked off zero so the
    /// matrix topology is load-bearing for the samples — a route that decoded
    /// onto the wrong destination would be inaudible with every knob at rest.
    fn render(p: &Patch) -> Vec<u32> {
        let mut e = Engine::new(48_000.0);
        e.install_patch(p.clone());
        for (m, v) in [(0, 0.42_f32), (1, 0.31), (2, 0.77), (3, 0.55), (5, 0.63)] {
            e.set_macro(m, v);
        }
        for n in [48, 55, 60, 64, 67, 72] {
            e.note_on(n, 100);
        }
        let mut out = Vec::new();
        let (mut l, mut r) = ([0.0_f32; 256], [0.0_f32; 256]);
        for block in 0..24 {
            if block == 16 {
                for n in [48, 55, 60, 64, 67, 72] {
                    e.note_off(n);
                }
            }
            e.process(&mut l, &mut r);
            out.extend(l.iter().chain(r.iter()).map(|s| s.to_bits()));
        }
        out
    }

    /// E052's acceptance, at its sharpest: not "sounds the same" but the same
    /// bits. Every field compared individually could still miss one the codec
    /// never looks at, and the render is what notices.
    #[test]
    fn every_factory_patch_renders_bit_identically_after_a_round_trip() {
        for i in 0..N_PATCHES {
            let p = patch(i);
            let back = read_preset(&write(&p)).unwrap().patch;
            let (before, after) = (render(&p), render(&back));
            assert_eq!(before.len(), after.len());
            let diff = before.iter().zip(after.iter()).position(|(a, b)| a != b);
            assert_eq!(diff, None, "{}: samples diverge at {diff:?}", p.name);
            // ...and the render is not trivially silent, or the assertion above
            // would hold for a codec that dropped the whole patch.
            assert!(
                before.iter().any(|s| f32::from_bits(*s).abs() > 1.0e-4),
                "{} rendered silence",
                p.name
            );
        }
    }

    /// splitmix64. A generated patch has to be *reproducible* — a property test
    /// that cannot be re-run on its failing input is a flake report, not a bug
    /// report — and that rules out anything seeded from the clock.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u32 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            ((z ^ (z >> 31)) >> 32) as u32
        }

        fn below(&mut self, n: usize) -> usize {
            self.next() as usize % n
        }

        fn unit(&mut self) -> f32 {
            self.next() as f32 / u32::MAX as f32
        }

        fn chance(&mut self, n: usize) -> bool {
            self.below(n) == 0
        }
    }

    fn random_value(d: &ParamDesc, r: &mut Rng) -> f32 {
        match d.kind {
            ParamKind::Enum { variants } => r.below(variants.len()) as f32,
            ParamKind::Bool => r.below(2) as f32,
            ParamKind::Int { .. } => d.min + r.below((d.max - d.min) as usize + 1) as f32,
            ParamKind::Float { .. } => d.min + r.unit() * (d.max - d.min),
        }
    }

    /// A patch with every field somewhere random in its own range, and a matrix
    /// of slots that are either fully wired or entirely blank.
    ///
    /// Blank-or-wired rather than "each field independently random": a slot with
    /// no source is not written at all, so a *half*-filled slot's curve and
    /// switch are lost by design and comparing them would be asserting against
    /// the format's own contract. `an_unwired_slot_decodes_inert` covers that
    /// case on purpose instead.
    fn random_patch(r: &mut Rng) -> Patch {
        let mut p = default_patch();
        for id in patch_ids() {
            let d = desc(id).unwrap();
            // Leave some fields alone, so the sparse path is exercised too.
            if r.chance(3) {
                continue;
            }
            set_value(&mut p, id, random_value(d, r));
        }
        for slot in p.matrix.slots.iter_mut() {
            if r.chance(2) {
                continue;
            }
            slot.source = SourceId::from_u8(1 + r.below(N_MACROS) as u8);
            slot.dest = DestId::from_u8(1 + r.below(crate::matrix::N_DESTS) as u8);
            slot.polarity = Polarity::from_u8(r.below(3) as u8);
            slot.shape = Shape::from_u8(r.below(3) as u8);
            slot.enabled = r.below(2) == 0;
            slot.scale_src = SourceId::from_u8(r.below(N_MACROS + 1) as u8);
            slot.scale_polarity = Polarity::from_u8(r.below(3) as u8);
            slot.scale_shape = Shape::from_u8(r.below(3) as u8);
        }
        p
    }

    fn random_macros(r: &mut Rng) -> Macros {
        std::array::from_fn(|m| {
            if r.chance(2) {
                return MacroSpec::default();
            }
            MacroSpec {
                label: format!("Knob {m}"),
                min: r.unit() * 10.0,
                max: 10.0 + r.unit() * 1000.0,
                unit: [" ct", " Hz", "%", "", " dB"][r.below(5)].to_string(),
            }
        })
    }

    /// The table is 252 entries wide and mostly generated, so an off-by-one in
    /// the name scheme would land a field in its neighbour's slot silently.
    /// Only a property test over whole random patches sees that; example tests
    /// all pass, because they only ever look at the fields they name.
    #[test]
    fn random_patches_round_trip_field_by_field() {
        let mut r = Rng(0x5EED_0383);
        for case in 0..64 {
            let p = random_patch(&mut r);
            let macros = random_macros(&mut r);
            let text = write_preset(&meta("Random"), &p, &macros).unwrap();
            let back = read_preset(&text).unwrap_or_else(|e| panic!("case {case}: {e}"));
            assert!(back.warnings.is_empty(), "case {case}: {:?}", back.warnings);
            assert_same_params(&p, &back.patch, &format!("case {case}"));
            assert_same_topology(&p.matrix, &back.patch.matrix, &format!("case {case}"));
            assert_eq!(back.macros, macros, "case {case}");
        }
    }

    #[test]
    fn an_empty_body_decodes_to_the_descriptor_defaults() {
        let back = read_preset("schema = 1\n[meta]\nname = \"Blank\"\n").unwrap();
        assert!(back.warnings.is_empty());
        for id in patch_ids() {
            let d = desc(id).unwrap();
            assert_eq!(
                value_of(&back.patch, id).unwrap().to_bits(),
                d.default.to_bits(),
                "{} did not default",
                d.name
            );
        }
        assert_eq!(back.macros, Macros::default());
        // ...and the patch's display name is the envelope's, not the body's.
        assert_eq!(back.meta.name, "Blank");
        assert_eq!(back.patch.name, "");
    }

    // ── warnings ────────────────────────────────────────────────────────────

    #[test]
    fn an_unknown_param_key_warns_and_keeps_the_rest_of_the_file() {
        let s = r#"
schema = 1
[meta]
name = "X"
[params]
op-0-ratio = 3.5
filter-cutoff = 800.0
"#;
        let back = read_preset(s).unwrap();
        assert_eq!(back.warnings.len(), 1);
        assert!(
            back.warnings[0].contains("filter-cutoff"),
            "{:?}",
            back.warnings
        );
        assert_eq!(back.patch.ops[0].ratio, 3.5);
    }

    #[test]
    fn a_host_param_in_the_body_warns_rather_than_moving_the_players_knobs() {
        let s = r#"
schema = 1
[meta]
name = "X"
[params]
master-gain = 2.0
macro-3 = 0.9
"#;
        let back = read_preset(s).unwrap();
        assert_eq!(back.warnings.len(), 2);
        assert!(
            back.warnings.iter().all(|w| w.contains("host state")),
            "{:?}",
            back.warnings
        );
    }

    #[test]
    fn an_unknown_enum_label_warns_and_takes_the_default() {
        let s = r#"
schema = 1
[meta]
name = "X"
[params]
op-0-wave = "abs-sine"
op-1-wave = "SQUARE"
"#;
        let back = read_preset(s).unwrap();
        assert_eq!(back.warnings.len(), 1, "{:?}", back.warnings);
        assert!(back.warnings[0].contains("abs-sine"), "{:?}", back.warnings);
        assert_eq!(back.patch.ops[0].wave, vxn4_dsp::wavetable::Waveform::Sine);
        // Case-insensitive, because a preset is a text file people edit.
        assert_eq!(
            back.patch.ops[1].wave,
            vxn4_dsp::wavetable::Waveform::Square
        );
    }

    #[test]
    fn a_wrong_typed_value_warns_and_takes_the_default() {
        let s = r#"
schema = 1
[meta]
name = "X"
[params]
op-0-ratio = "loud"
"#;
        let back = read_preset(s).unwrap();
        assert!(
            back.warnings[0].contains("expected a number"),
            "{:?}",
            back.warnings
        );
        assert_eq!(
            back.patch.ops[0].ratio,
            desc(id_for_name("op-0-ratio").unwrap()).unwrap().default
        );
    }

    /// An integer where a float belongs is what a hand-written preset produces,
    /// and TOML types the two apart. Accepting it is not laxity — refusing
    /// `out-0 = 1` would be a papercut with no upside.
    #[test]
    fn an_integer_reads_as_a_float() {
        let s = "schema = 1\n[meta]\nname = \"X\"\n[params]\nout-0 = 1\n";
        let back = read_preset(s).unwrap();
        assert!(back.warnings.is_empty(), "{:?}", back.warnings);
        assert_eq!(back.patch.routing.out[0], 1.0);
    }

    #[test]
    fn an_unknown_route_endpoint_warns_and_leaves_the_slot_inert() {
        let s = r#"
schema = 1
[meta]
name = "X"
[params]
matrix-00-depth = 0.5
[[matrix]]
slot = 0
source = "lfo1"
dest = "pm-0-1"
[[matrix]]
slot = 1
source = "macro1"
dest = "filter-cutoff"
"#;
        let back = read_preset(s).unwrap();
        assert_eq!(back.warnings.len(), 2);
        assert!(
            back.warnings[0].contains("unknown source"),
            "{:?}",
            back.warnings
        );
        assert!(
            back.warnings[1].contains("unknown dest"),
            "{:?}",
            back.warnings
        );
        assert!(!back.patch.matrix.slots[0].is_wired());
        assert!(!back.patch.matrix.slots[1].is_wired());
        // The depth is a param and survives the row being dropped, so re-wiring
        // the slot by hand gets the depth its author meant.
        assert_eq!(back.patch.matrix.slots[0].depth, 0.5);
    }

    #[test]
    fn an_unknown_shaping_column_degrades_to_the_identity_and_warns() {
        let s = r#"
schema = 1
[meta]
name = "X"
[[matrix]]
slot = 0
source = "macro1"
dest = "pm-0-1"
curve = "sproing"
scale-src = "lfo9"
scale-polarity = "sideways"
scale-shape = "wobble"
"#;
        let back = read_preset(s).unwrap();
        assert_eq!(back.warnings.len(), 4, "{:?}", back.warnings);
        let s0 = back.patch.matrix.slots[0];
        // Wired, because the endpoints were good — only the shaping degraded.
        assert!(s0.is_wired());
        assert_eq!((s0.polarity, s0.shape), (Polarity::None, Shape::Lin));
        assert_eq!(s0.scale_src, SourceId::None);
        assert_eq!(s0.scale_polarity, Polarity::None);
        assert_eq!(s0.scale_shape, Shape::Lin);
    }

    /// `direct` is what the resting polarity was spelled before 0340. It names
    /// the same range map, so it is accepted silently rather than warned about.
    #[test]
    fn the_pre_0340_polarity_spelling_still_reads() {
        let s = r#"
schema = 1
[meta]
name = "X"
[[matrix]]
slot = 0
source = "macro1"
dest = "pm-0-1"
scale-src = "macro2"
scale-polarity = "direct"
"#;
        let back = read_preset(s).unwrap();
        assert!(back.warnings.is_empty(), "{:?}", back.warnings);
        assert_eq!(back.patch.matrix.slots[0].scale_polarity, Polarity::None);
    }

    #[test]
    fn a_slot_index_past_the_table_warns_and_is_skipped() {
        let s = r#"
schema = 1
[meta]
name = "X"
[[matrix]]
slot = 200
source = "macro1"
dest = "pm-0-1"
"#;
        let back = read_preset(s).unwrap();
        assert!(
            back.warnings[0].contains("out of range"),
            "{:?}",
            back.warnings
        );
        assert!(back.patch.matrix.slots.iter().all(|s| !s.is_wired()));
    }

    #[test]
    fn an_unwired_slot_decodes_inert() {
        // Nothing in the file mentions slot 4, so it comes back blank whatever
        // its neighbours do.
        let text = write(&patch(0));
        let back = read_preset(&text).unwrap();
        assert_eq!(back.patch.matrix.slots[4], MatrixSlot::default());
    }

    // ── the envelope ────────────────────────────────────────────────────────

    #[test]
    fn an_unsupported_schema_is_a_typed_error() {
        let s = "schema = 99\n[meta]\nname = \"X\"\n";
        match read_preset(s) {
            Err(PresetError::UnsupportedSchema {
                found: 99,
                expected: 1,
            }) => {}
            other => panic!("expected UnsupportedSchema, got {other:?}"),
        }
    }

    #[test]
    fn malformed_toml_is_an_error_rather_than_a_warning() {
        assert!(matches!(
            read_preset("not a preset ===="),
            Err(PresetError::Toml(_))
        ));
        // A missing envelope is malformed too — `name` is the one required key.
        assert!(matches!(
            read_preset("schema = 1\n"),
            Err(PresetError::Toml(_))
        ));
    }

    #[test]
    fn meta_survives_the_round_trip() {
        let m = Meta {
            name: "Glass Tine".to_string(),
            author: Some("df".to_string()),
            category: Some("keys".to_string()),
            comment: Some("measured at -6 dBFS".to_string()),
        };
        let text = write_preset(&m, &patch(1), &Macros::default()).unwrap();
        let back = read_preset(&text).unwrap();
        assert_eq!(back.meta, m);
    }
}
