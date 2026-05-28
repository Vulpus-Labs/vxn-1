//! Portable VXN1 preset format (E007 / ADR 0005 §1–§3).
//!
//! A preset is a **TOML** text file, keyed by parameter [`ParamDesc::name`]
//! (never by index or CLAP id — the param table reorders freely, so any
//! positional format would rot; [[vxn1-id-stability-dropped]]). Enums store
//! their **variant label** (`osc1_wave = "Saw"`), bools as `true`/`false`,
//! numbers in the descriptor's plain unit. Files are **sparse**: only params
//! that deviate from their descriptor default are written, so presets stay
//! small, diffable, and auto-adopt improved defaults.
//!
//! Two kinds (ADR 0005 terminology):
//!
//! - **Patch** — one layer's sound ([`PatchValues`]); loads into Upper *or*
//!   Lower, carries no global state.
//! - **Performance** — the whole instrument ([`PluginState`]): both layers, the
//!   global block, key mode and split point.
//!
//! This module is the **pure** mapping between those engine types and the file
//! format. No IO, no embedding, no clap, no UI, no host sync — that is 0025
//! (embedding), 0026 (load/save + host notify) and 0027 (browser). It is
//! main-thread only; the audio path never gains a serde/toml dependency.
//!
//! The binary [`crate::state`] blob is unchanged and kept for the CLAP
//! host-session channel — a different job (fast, opaque, positional). The two
//! formats are deliberately *not* unified (ADR 0005 §1).

use crate::params::{
    GlobalParam, GlobalValues, KeyMode, ParamDesc, ParamKind, ParamValues, PatchParam, PatchValues,
};
use crate::state::PluginState;
use serde::{Deserialize, Serialize};

/// Preset *file-format* version (independent of the binary state `VERSION`).
/// Because the format is name-keyed, most evolutions need no bump; reserve this
/// for structural changes (ADR 0005 §2).
pub const SCHEMA: u32 = 1;

const KIND_PATCH: &str = "patch";
const KIND_PERFORMANCE: &str = "performance";

/// Free-form preset metadata (the `[meta]` table). Only `name` is required.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Browser grouping (0027). Free-form string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// One layer's sound plus its metadata. Converts to/from [`PatchValues`].
#[derive(Clone, Debug)]
pub struct Patch {
    pub meta: Meta,
    pub values: PatchValues,
}

/// The whole instrument plus its metadata. Converts to/from [`PluginState`].
#[derive(Clone, Debug)]
pub struct Performance {
    pub meta: Meta,
    pub state: PluginState,
}

/// A parsed preset of either kind (the result of [`from_toml_str`]).
// A `Performance` is larger than a `Patch`, but this is a main-thread parse
// result handled once at load time — not boxing keeps the downstream load path
// (0026) a plain match, with no runtime cost worth the indirection.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum Preset {
    Patch(Patch),
    Performance(Performance),
}

/// Why a preset failed to parse. Unknown keys / bad enum labels do **not** land
/// here — those are non-fatal warnings (see [`from_toml_str`]).
#[derive(Debug)]
pub enum PresetError {
    /// The TOML did not parse, or required envelope fields (`schema`, `kind`,
    /// `meta`, the body table) were missing or the wrong type.
    Toml(toml::de::Error),
    /// `schema` is not a version this build understands.
    UnsupportedSchema { found: u32, expected: u32 },
    /// `kind` is neither `"patch"` nor `"performance"`.
    UnknownKind { found: String },
}

impl std::fmt::Display for PresetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PresetError::Toml(e) => write!(f, "invalid preset TOML: {e}"),
            PresetError::UnsupportedSchema { found, expected } => {
                write!(f, "unsupported preset schema {found} (this build expects {expected})")
            }
            PresetError::UnknownKind { found } => {
                write!(f, "unknown preset kind `{found}` (expected `patch` or `performance`)")
            }
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
//
// The param maps are kept as `toml::Table` (name → dynamically-typed value) and
// resolved against the descriptor by hand below — a fully-derived struct can't
// express "enum stored by label, value clamped to a runtime range". Scalars are
// declared before any table field so TOML serialization is well-formed.

#[derive(Serialize, Deserialize)]
struct PatchFile {
    schema: u32,
    kind: String,
    meta: Meta,
    #[serde(default)]
    patch: toml::Table,
}

#[derive(Serialize, Deserialize)]
struct PerformanceFile {
    schema: u32,
    kind: String,
    meta: Meta,
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

/// Just enough of the envelope to dispatch on before committing to a body shape.
#[derive(Deserialize)]
struct Header {
    schema: u32,
    kind: String,
}

// ── Sparse write (engine values → TOML value) ───────────────────────────────

/// One param's value as a typed TOML scalar, matching its descriptor kind.
fn value_for(desc: &ParamDesc, v: f32) -> toml::Value {
    match desc.kind {
        ParamKind::Enum { variants } => {
            let i = (v.round() as usize).min(variants.len().saturating_sub(1));
            toml::Value::String(variants[i].to_string())
        }
        ParamKind::Bool => toml::Value::Boolean(v >= 0.5),
        ParamKind::Int { .. } => toml::Value::Integer(v.round() as i64),
        // f32 → f64 widening is exact, and the f64 narrows back to the same f32
        // on read, so the stored value round-trips precisely.
        ParamKind::Float { .. } => toml::Value::Float(v as f64),
    }
}

fn patch_to_table(pv: &PatchValues) -> toml::Table {
    let mut t = toml::Table::new();
    for p in PatchParam::all() {
        let d = p.desc();
        let v = pv.get(p);
        if v != d.default {
            t.insert(d.name.to_string(), value_for(d, v));
        }
    }
    t
}

fn global_to_table(gv: &GlobalValues) -> toml::Table {
    let mut t = toml::Table::new();
    for g in GlobalParam::all() {
        let d = g.desc();
        let v = gv.get(g);
        if v != d.default {
            t.insert(d.name.to_string(), value_for(d, v));
        }
    }
    t
}

// ── Default-fill read (TOML value → engine values, collecting warnings) ──────

/// Resolve one TOML scalar to a plain-unit `f32` for `desc`. On any type or
/// label mismatch, push a warning and return `None` (the caller leaves the
/// descriptor default in place).
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
                    warnings.push(format!(
                        "{ctx}.{key}: unknown enum label `{s}` (using default)"
                    ));
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
        // Accept either TOML number form for numeric params.
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

fn apply_patch_table(table: &toml::Table, ctx: &str, pv: &mut PatchValues, warnings: &mut Vec<String>) {
    for (key, val) in table {
        match PatchParam::from_name(key) {
            Some(p) => {
                if let Some(v) = parse_value(p.desc(), ctx, key, val, warnings) {
                    pv.set(p, v); // clamps to range
                }
            }
            None => warnings.push(format!("{ctx}: unknown parameter `{key}` (skipped)")),
        }
    }
}

fn apply_global_table(
    table: &toml::Table,
    ctx: &str,
    gv: &mut GlobalValues,
    warnings: &mut Vec<String>,
) {
    for (key, val) in table {
        match GlobalParam::from_name(key) {
            Some(g) => {
                if let Some(v) = parse_value(g.desc(), ctx, key, val, warnings) {
                    gv.set(g, v); // clamps to range
                }
            }
            None => warnings.push(format!("{ctx}: unknown parameter `{key}` (skipped)")),
        }
    }
}

fn table_to_patch(table: &toml::Table, ctx: &str, warnings: &mut Vec<String>) -> PatchValues {
    let mut pv = PatchValues::default();
    apply_patch_table(table, ctx, &mut pv, warnings);
    pv
}

// ── Public API ──────────────────────────────────────────────────────────────

impl Patch {
    /// Serialize to a sparse TOML preset (`kind = "patch"`).
    pub fn to_toml_string(&self) -> String {
        let file = PatchFile {
            schema: SCHEMA,
            kind: KIND_PATCH.to_string(),
            meta: self.meta.clone(),
            patch: patch_to_table(&self.values),
        };
        // Values are clamped to finite descriptor ranges, so serialization of
        // this shape cannot fail.
        toml::to_string_pretty(&file).expect("patch preset serialization is infallible")
    }
}

impl Performance {
    /// Serialize to a sparse TOML preset (`kind = "performance"`).
    pub fn to_toml_string(&self) -> String {
        let p = &self.state.params;
        let file = PerformanceFile {
            schema: SCHEMA,
            kind: KIND_PERFORMANCE.to_string(),
            meta: self.meta.clone(),
            performance: PerformanceBody {
                key_mode: self.state.key_mode.label().to_string(),
                split_point: self.state.split_point,
                global: global_to_table(&p.global),
                upper: patch_to_table(&p.layers[0]),
                lower: patch_to_table(&p.layers[1]),
            },
        };
        toml::to_string_pretty(&file).expect("performance preset serialization is infallible")
    }
}

impl Preset {
    /// Serialize whichever kind this is.
    pub fn to_toml_string(&self) -> String {
        match self {
            Preset::Patch(p) => p.to_toml_string(),
            Preset::Performance(p) => p.to_toml_string(),
        }
    }

    /// The preset's metadata, regardless of kind (browser display).
    pub fn meta(&self) -> &Meta {
        match self {
            Preset::Patch(p) => &p.meta,
            Preset::Performance(p) => &p.meta,
        }
    }

    /// `"patch"` or `"performance"` — the on-disk `kind`.
    pub fn kind_str(&self) -> &'static str {
        match self {
            Preset::Patch(_) => KIND_PATCH,
            Preset::Performance(_) => KIND_PERFORMANCE,
        }
    }
}

/// Parse a TOML preset of either kind. Returns the preset plus any non-fatal
/// **warnings** (unknown keys, bad enum labels, type mismatches — each fell back
/// to the descriptor default rather than failing the load). Only a malformed
/// envelope (`schema`/`kind`/structure) is a hard [`PresetError`].
pub fn from_toml_str(s: &str) -> Result<(Preset, Vec<String>), PresetError> {
    let header: Header = toml::from_str(s)?;
    if header.schema != SCHEMA {
        return Err(PresetError::UnsupportedSchema {
            found: header.schema,
            expected: SCHEMA,
        });
    }

    let mut warnings = Vec::new();
    match header.kind.as_str() {
        KIND_PATCH => {
            let file: PatchFile = toml::from_str(s)?;
            let values = table_to_patch(&file.patch, "patch", &mut warnings);
            Ok((
                Preset::Patch(Patch {
                    meta: file.meta,
                    values,
                }),
                warnings,
            ))
        }
        KIND_PERFORMANCE => {
            let file: PerformanceFile = toml::from_str(s)?;
            let body = file.performance;

            let key_mode = KeyMode::from_label(&body.key_mode).unwrap_or_else(|| {
                warnings.push(format!(
                    "performance.key_mode: unknown `{}` (using Whole)",
                    body.key_mode
                ));
                KeyMode::Whole
            });

            let mut global = GlobalValues::default();
            apply_global_table(&body.global, "performance.global", &mut global, &mut warnings);
            let upper = table_to_patch(&body.upper, "performance.upper", &mut warnings);
            let lower = table_to_patch(&body.lower, "performance.lower", &mut warnings);

            Ok((
                Preset::Performance(Performance {
                    meta: file.meta,
                    state: PluginState {
                        params: ParamValues {
                            layers: [upper, lower],
                            global,
                        },
                        key_mode,
                        split_point: body.split_point,
                    },
                }),
                warnings,
            ))
        }
        other => Err(PresetError::UnknownKind {
            found: other.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::Layer;

    /// A value guaranteed to differ from `desc.default`, within range, that
    /// round-trips exactly through the format.
    fn non_default(desc: &ParamDesc) -> f32 {
        match desc.kind {
            ParamKind::Enum { .. } => {
                // Every enum here has >= 2 variants, so 0 and 1 are both valid.
                if desc.default.round() as usize == 0 {
                    1.0
                } else {
                    0.0
                }
            }
            ParamKind::Bool => 1.0 - desc.default,
            ParamKind::Int { .. } => {
                if desc.default < desc.max {
                    desc.default + 1.0
                } else {
                    desc.default - 1.0
                }
            }
            ParamKind::Float { .. } => {
                let mid = (desc.min + desc.max) * 0.5;
                if mid != desc.default {
                    mid
                } else if desc.min != desc.default {
                    desc.min
                } else {
                    desc.max
                }
            }
        }
    }

    fn meta(name: &str) -> Meta {
        Meta {
            name: name.to_string(),
            ..Meta::default()
        }
    }

    #[test]
    fn every_patch_param_round_trips() {
        // Set every param to a distinct non-default value, then serialize/parse.
        let mut pv = PatchValues::default();
        for p in PatchParam::all() {
            let want = non_default(p.desc());
            assert_ne!(want, p.desc().default, "{} test value is default", p.desc().name);
            pv.set(p, want);
        }
        let patch = Patch {
            meta: meta("RT"),
            values: pv.clone(),
        };
        let (parsed, warnings) = from_toml_str(&patch.to_toml_string()).unwrap();
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        let Preset::Patch(back) = parsed else {
            panic!("expected a patch");
        };
        for p in PatchParam::all() {
            assert_eq!(
                back.values.get(p),
                pv.get(p),
                "{} did not round-trip",
                p.desc().name
            );
        }
        assert_eq!(back.meta, meta("RT"));
    }

    #[test]
    fn every_global_param_round_trips() {
        let mut gv = GlobalValues::default();
        for g in GlobalParam::all() {
            gv.set(g, non_default(g.desc()));
        }
        let mut state = PluginState {
            params: ParamValues::default(),
            key_mode: KeyMode::Whole,
            split_point: 60,
        };
        state.params.global = gv.clone();
        let perf = Performance {
            meta: meta("G"),
            state,
        };
        let (parsed, warnings) = from_toml_str(&perf.to_toml_string()).unwrap();
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        let Preset::Performance(back) = parsed else {
            panic!("expected a performance");
        };
        for g in GlobalParam::all() {
            assert_eq!(
                back.state.params.global.get(g),
                gv.get(g),
                "{} did not round-trip",
                g.desc().name
            );
        }
    }

    #[test]
    fn default_patch_is_sparse() {
        let patch = Patch {
            meta: meta("Empty"),
            values: PatchValues::default(),
        };
        let s = patch.to_toml_string();
        // The [patch] table carries no entries when nothing deviates from default.
        let doc: toml::Table = toml::from_str(&s).unwrap();
        let body = doc.get("patch").and_then(|v| v.as_table());
        assert!(
            body.map(|t| t.is_empty()).unwrap_or(true),
            "default patch should serialize an empty body, got: {s}"
        );
        // And parsing an empty body yields exactly the defaults.
        let (parsed, warnings) = from_toml_str(&s).unwrap();
        assert!(warnings.is_empty());
        let Preset::Patch(back) = parsed else {
            panic!("expected a patch");
        };
        let def = PatchValues::default();
        for p in PatchParam::all() {
            assert_eq!(back.values.get(p), def.get(p));
        }
    }

    #[test]
    fn unknown_key_warns_and_skips() {
        let s = r#"
schema = 1
kind = "patch"
[meta]
name = "X"
[patch]
cutoff = 1234.0
not_a_param = 5.0
"#;
        let (parsed, warnings) = from_toml_str(s).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("not_a_param"), "{warnings:?}");
        let Preset::Patch(back) = parsed else {
            panic!("expected a patch");
        };
        assert_eq!(back.values.get(PatchParam::Cutoff), 1234.0);
    }

    #[test]
    fn bad_enum_label_warns_and_defaults() {
        let s = r#"
schema = 1
kind = "patch"
[meta]
name = "X"
[patch]
osc1_wave = "Sawww"
"#;
        let (parsed, warnings) = from_toml_str(s).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("Sawww"), "{warnings:?}");
        let Preset::Patch(back) = parsed else {
            panic!("expected a patch");
        };
        assert_eq!(
            back.values.get(PatchParam::Osc1Wave),
            PatchParam::Osc1Wave.desc().default
        );
    }

    #[test]
    fn enum_label_is_case_insensitive() {
        let s = r#"
schema = 1
kind = "patch"
[meta]
name = "X"
[patch]
osc1_wave = "pulse"
"#;
        let (parsed, warnings) = from_toml_str(s).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let Preset::Patch(back) = parsed else {
            panic!("expected a patch");
        };
        // "Pulse" is index 3 in WAVE_LABELS.
        assert_eq!(back.values.get(PatchParam::Osc1Wave), 3.0);
    }

    #[test]
    fn value_clamps_on_read() {
        let s = r#"
schema = 1
kind = "patch"
[meta]
name = "X"
[patch]
resonance = 9.0
"#;
        let (parsed, _) = from_toml_str(s).unwrap();
        let Preset::Patch(back) = parsed else {
            panic!("expected a patch");
        };
        assert_eq!(back.values.get(PatchParam::Resonance), 1.0);
    }

    #[test]
    fn performance_round_trips_full_state() {
        let mut params = ParamValues::default();
        params.layer_mut(Layer::Upper).set(PatchParam::Cutoff, 1111.0);
        params.layer_mut(Layer::Lower).set(PatchParam::Osc1Wave, 0.0);
        params.global.set(GlobalParam::MasterVolume, 0.33);
        params.global.set(GlobalParam::ChorusOn, 0.0);
        let state = PluginState {
            params,
            key_mode: KeyMode::Split,
            split_point: 48,
        };
        let perf = Performance {
            meta: meta("Perf"),
            state: state.clone(),
        };
        let (parsed, warnings) = from_toml_str(&perf.to_toml_string()).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let Preset::Performance(back) = parsed else {
            panic!("expected a performance");
        };
        assert_eq!(back.state.key_mode, KeyMode::Split);
        assert_eq!(back.state.split_point, 48);
        assert_eq!(
            back.state.params.layer(Layer::Upper).get(PatchParam::Cutoff),
            1111.0
        );
        assert_eq!(
            back.state.params.layer(Layer::Lower).get(PatchParam::Osc1Wave),
            0.0
        );
        assert_eq!(back.state.params.global.get(GlobalParam::MasterVolume), 0.33);
        assert_eq!(back.state.params.global.get(GlobalParam::ChorusOn), 0.0);
    }

    #[test]
    fn key_mode_serializes_by_label() {
        let perf = Performance {
            meta: meta("KM"),
            state: PluginState {
                params: ParamValues::default(),
                key_mode: KeyMode::Dual,
                split_point: 60,
            },
        };
        let s = perf.to_toml_string();
        assert!(s.contains(r#"key_mode = "Dual""#), "{s}");
    }

    #[test]
    fn schema_mismatch_is_typed_error() {
        let s = r#"
schema = 2
kind = "patch"
[meta]
name = "X"
"#;
        match from_toml_str(s) {
            Err(PresetError::UnsupportedSchema { found: 2, expected: 1 }) => {}
            other => panic!("expected UnsupportedSchema, got {other:?}"),
        }
    }

    #[test]
    fn unknown_kind_is_typed_error() {
        let s = r#"
schema = 1
kind = "banana"
[meta]
name = "X"
"#;
        match from_toml_str(s) {
            Err(PresetError::UnknownKind { found }) if found == "banana" => {}
            other => panic!("expected UnknownKind, got {other:?}"),
        }
    }

    #[test]
    fn malformed_toml_is_error() {
        assert!(matches!(from_toml_str("nonsense ===="), Err(PresetError::Toml(_))));
    }
}
