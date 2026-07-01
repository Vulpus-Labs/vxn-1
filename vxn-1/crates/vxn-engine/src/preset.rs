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
//! There is exactly one preset kind: a **Performance** — the whole instrument
//! ([`PluginState`]): both layers, the global block, key mode and split point.
//! The earlier patch/performance split was collapsed; every preset captures the
//! complete instrument state so loading is unambiguous.
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

// Shared sparse-TOML scaffold (ticket 0143). `Meta`, `PresetError`, `Header`
// and `SCHEMA` are byte-identical to vxn-2's codec, so they live in
// `vxn-preset`; re-exported here to keep the `crate::preset::Meta` path stable
// for `factory.rs`, `preset_io.rs` and the lib re-export.
pub use vxn_preset::{Header, Meta, PresetError, SCHEMA};
use vxn_preset::ScalarKind;

/// The whole instrument plus its metadata. Converts to/from [`PluginState`].
#[derive(Clone, Debug)]
pub struct Performance {
    pub meta: Meta,
    pub state: PluginState,
}

// ── On-disk file shape (serde) ──────────────────────────────────────────────
//
// The param maps are kept as `toml::Table` (name → dynamically-typed value) and
// resolved against the descriptor by hand below — a fully-derived struct can't
// express "enum stored by label, value clamped to a runtime range". Scalars are
// declared before any table field so TOML serialization is well-formed.

#[derive(Serialize, Deserialize)]
struct PerformanceFile {
    schema: u32,
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

// ── Sparse write (engine values → TOML value) ───────────────────────────────

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

/// A param namespace (the per-layer patch block, or the global block) viewed
/// generically so the sparse codec below is written once. The two blocks differ
/// only by which enum names/describes a param and which value container holds
/// it; this trait abstracts exactly that. Implemented for `PatchParam` over
/// `PatchValues` and `GlobalParam` over `GlobalValues`.
trait ParamNamespace: Copy {
    type Values;
    fn from_name(name: &str) -> Option<Self>;
    fn desc(self) -> &'static ParamDesc;
    fn all() -> Box<dyn Iterator<Item = Self>>;
    fn get(self, values: &Self::Values) -> f32;
    fn set(self, values: &mut Self::Values, v: f32);
}

impl ParamNamespace for PatchParam {
    type Values = PatchValues;
    fn from_name(name: &str) -> Option<Self> {
        PatchParam::from_name(name)
    }
    fn desc(self) -> &'static ParamDesc {
        PatchParam::desc(self)
    }
    fn all() -> Box<dyn Iterator<Item = Self>> {
        Box::new(PatchParam::all())
    }
    fn get(self, values: &PatchValues) -> f32 {
        values.get(self)
    }
    fn set(self, values: &mut PatchValues, v: f32) {
        values.set(self, v);
    }
}

impl ParamNamespace for GlobalParam {
    type Values = GlobalValues;
    fn from_name(name: &str) -> Option<Self> {
        GlobalParam::from_name(name)
    }
    fn desc(self) -> &'static ParamDesc {
        GlobalParam::desc(self)
    }
    fn all() -> Box<dyn Iterator<Item = Self>> {
        Box::new(GlobalParam::all())
    }
    fn get(self, values: &GlobalValues) -> f32 {
        values.get(self)
    }
    fn set(self, values: &mut GlobalValues, v: f32) {
        values.set(self, v);
    }
}

/// Sparse-write a namespace: emit only params that deviate from their default.
fn to_table<P: ParamNamespace>(values: &P::Values) -> toml::Table {
    let mut t = toml::Table::new();
    for p in P::all() {
        let d = p.desc();
        let v = p.get(values);
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

/// Default-fill read: resolve each table entry against the namespace and set it
/// (clamped to range), warning on unknown keys and bad values.
fn apply_table<P: ParamNamespace>(
    table: &toml::Table,
    ctx: &str,
    values: &mut P::Values,
    warnings: &mut Vec<String>,
) {
    for (key, val) in table {
        match P::from_name(key) {
            Some(p) => {
                if let Some(v) = parse_value(p.desc(), ctx, key, val, warnings) {
                    p.set(values, v); // clamps to range
                }
            }
            None => warnings.push(format!("{ctx}: unknown parameter `{key}` (skipped)")),
        }
    }
}

fn table_to_patch(table: &toml::Table, ctx: &str, warnings: &mut Vec<String>) -> PatchValues {
    let mut pv = PatchValues::default();
    apply_table::<PatchParam>(table, ctx, &mut pv, warnings);
    pv
}

// ── Public API ──────────────────────────────────────────────────────────────

impl Performance {
    /// Serialize to a sparse TOML preset.
    pub fn to_toml_string(&self) -> String {
        let p = &self.state.params;
        let file = PerformanceFile {
            schema: SCHEMA,
            meta: self.meta.clone(),
            performance: PerformanceBody {
                key_mode: self.state.key_mode.label().to_string(),
                split_point: self.state.split_point,
                global: to_table::<GlobalParam>(&p.global),
                upper: to_table::<PatchParam>(&p.layers[0]),
                lower: to_table::<PatchParam>(&p.layers[1]),
            },
        };
        // Values are clamped to finite descriptor ranges, so serialization of
        // this shape cannot fail.
        toml::to_string_pretty(&file).expect("performance preset serialization is infallible")
    }
}

/// Parse a TOML preset. Returns the [`Performance`] plus any non-fatal
/// **warnings** (unknown keys, bad enum labels, type mismatches — each fell back
/// to the descriptor default rather than failing the load). Only a malformed
/// envelope (`schema`/structure) is a hard [`PresetError`].
pub fn from_toml_str(s: &str) -> Result<(Performance, Vec<String>), PresetError> {
    let header: Header = toml::from_str(s)?;
    if header.schema != SCHEMA {
        return Err(PresetError::UnsupportedSchema {
            found: header.schema,
            expected: SCHEMA,
        });
    }

    let mut warnings = Vec::new();
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
    apply_table::<GlobalParam>(&body.global, "performance.global", &mut global, &mut warnings);
    let upper = table_to_patch(&body.upper, "performance.upper", &mut warnings);
    let lower = table_to_patch(&body.lower, "performance.lower", &mut warnings);

    Ok((
        Performance {
            meta: file.meta,
            state: PluginState {
                params: ParamValues {
                    layers: [upper, lower],
                    global,
                },
                key_mode,
                split_point: body.split_point,
            },
        },
        warnings,
    ))
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
        // Set every per-layer param in Upper to a distinct non-default value,
        // then serialize/parse through the performance format.
        let mut pv = PatchValues::default();
        for p in PatchParam::all() {
            let want = non_default(p.desc());
            assert_ne!(want, p.desc().default, "{} test value is default", p.desc().name);
            pv.set(p, want);
        }
        let mut params = ParamValues::default();
        *params.layer_mut(Layer::Upper) = pv.clone();
        let perf = Performance {
            meta: meta("RT"),
            state: PluginState {
                params,
                key_mode: KeyMode::Whole,
                split_point: 60,
            },
        };
        let (back, warnings) = from_toml_str(&perf.to_toml_string()).unwrap();
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        for p in PatchParam::all() {
            assert_eq!(
                back.state.params.layer(Layer::Upper).get(p),
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
        let (back, warnings) = from_toml_str(&perf.to_toml_string()).unwrap();
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
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
    fn default_performance_is_sparse() {
        let perf = Performance {
            meta: meta("Empty"),
            state: PluginState {
                params: ParamValues::default(),
                key_mode: KeyMode::Whole,
                split_point: 60,
            },
        };
        let s = perf.to_toml_string();
        // The body tables carry no entries when nothing deviates from default.
        let doc: toml::Table = toml::from_str(&s).unwrap();
        let body = doc.get("performance").and_then(|v| v.as_table()).unwrap();
        for tbl in ["global", "upper", "lower"] {
            let t = body.get(tbl).and_then(|v| v.as_table());
            assert!(
                t.map(|t| t.is_empty()).unwrap_or(true),
                "default {tbl} should serialize empty, got: {s}"
            );
        }
        // And parsing an empty body yields exactly the defaults.
        let (back, warnings) = from_toml_str(&s).unwrap();
        assert!(warnings.is_empty());
        let def = PatchValues::default();
        for p in PatchParam::all() {
            assert_eq!(back.state.params.layer(Layer::Upper).get(p), def.get(p));
        }
    }

    #[test]
    fn unknown_key_warns_and_skips() {
        let s = r#"
schema = 1
[meta]
name = "X"
[performance]
key_mode = "Whole"
split_point = 60
[performance.upper]
cutoff = 1234.0
not_a_param = 5.0
"#;
        let (back, warnings) = from_toml_str(s).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("not_a_param"), "{warnings:?}");
        assert_eq!(back.state.params.layer(Layer::Upper).get(PatchParam::Cutoff), 1234.0);
    }

    #[test]
    fn bad_enum_label_warns_and_defaults() {
        let s = r#"
schema = 1
[meta]
name = "X"
[performance]
key_mode = "Whole"
split_point = 60
[performance.upper]
osc1_wave = "Sawww"
"#;
        let (back, warnings) = from_toml_str(s).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("Sawww"), "{warnings:?}");
        assert_eq!(
            back.state.params.layer(Layer::Upper).get(PatchParam::Osc1Wave),
            PatchParam::Osc1Wave.desc().default
        );
    }

    #[test]
    fn enum_label_is_case_insensitive() {
        let s = r#"
schema = 1
[meta]
name = "X"
[performance]
key_mode = "Whole"
split_point = 60
[performance.upper]
osc1_wave = "pulse"
"#;
        let (back, warnings) = from_toml_str(s).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        // "Pulse" is index 3 in WAVE_LABELS.
        assert_eq!(back.state.params.layer(Layer::Upper).get(PatchParam::Osc1Wave), 3.0);
    }

    #[test]
    fn value_clamps_on_read() {
        let s = r#"
schema = 1
[meta]
name = "X"
[performance]
key_mode = "Whole"
split_point = 60
[performance.upper]
resonance = 9.0
"#;
        let (back, _) = from_toml_str(s).unwrap();
        assert_eq!(back.state.params.layer(Layer::Upper).get(PatchParam::Resonance), 1.0);
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
        let (back, warnings) = from_toml_str(&perf.to_toml_string()).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
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
[meta]
name = "X"
"#;
        match from_toml_str(s) {
            Err(PresetError::UnsupportedSchema { found: 2, expected: 1 }) => {}
            other => panic!("expected UnsupportedSchema, got {other:?}"),
        }
    }

    #[test]
    fn malformed_toml_is_error() {
        assert!(matches!(from_toml_str("nonsense ===="), Err(PresetError::Toml(_))));
    }

    // ── vxn-app TOML codec parity (E019 / 0066) ──────────────────────────────
    //
    // The web export/import + URL share-link reuse a wasm-clean reimplementation
    // of THIS format in `vxn-app::preset_toml` (model-trait based, no engine
    // types), so an exported web patch imports on desktop and vice versa. These
    // guard against the two drifting — the same discipline `state.rs` gets via
    // `codec_matches_legacy_plugin_state`.

    use crate::shared::SharedParams;

    /// A `PluginState` with a distinct non-default value set on every per-layer
    /// param (Upper and Lower) and every global, plus non-default key mode +
    /// split — maximal coverage for the parity asserts.
    fn dense_state() -> PluginState {
        let mut params = ParamValues::default();
        for p in PatchParam::all() {
            params.layer_mut(Layer::Upper).set(p, non_default(p.desc()));
            params.layer_mut(Layer::Lower).set(p, non_default(p.desc()));
        }
        for g in GlobalParam::all() {
            params.global.set(g, non_default(g.desc()));
        }
        PluginState {
            params,
            key_mode: KeyMode::Split,
            split_point: 48,
        }
    }

    fn app_meta(m: &Meta) -> vxn_app::PresetMeta {
        vxn_app::PresetMeta {
            name: m.name.clone(),
            author: m.author.clone(),
            category: m.category.clone(),
            comment: m.comment.clone(),
        }
    }

    // The vxn-app writer is byte-identical to the engine's `to_toml_string` for
    // the same state + meta (so the file format cannot drift between backends).
    #[test]
    fn app_writer_matches_engine_byte_for_byte() {
        let state = dense_state();
        let meta = Meta {
            name: "Drift Guard".into(),
            author: Some("Vulpus Labs".into()),
            category: Some("Bass".into()),
            comment: Some("parity".into()),
        };
        let engine_toml = Performance {
            meta: meta.clone(),
            state: state.clone(),
        }
        .to_toml_string();

        let shared = SharedParams::new();
        shared.load_performance(&state);
        let app_toml = vxn_app::write_toml(&shared, &app_meta(&meta));

        assert_eq!(engine_toml, app_toml, "vxn-app TOML drifted from engine");
    }

    // A vxn-app-exported patch parses on the engine (desktop) loader, reproducing
    // every param — the "imports on the desktop build" acceptance, in reverse.
    #[test]
    fn app_write_parses_on_engine() {
        let state = dense_state();
        let shared = SharedParams::new();
        shared.load_performance(&state);
        let meta = vxn_app::PresetMeta {
            name: "Web Patch".into(),
            ..Default::default()
        };
        let app_toml = vxn_app::write_toml(&shared, &meta);

        let (back, warnings) = from_toml_str(&app_toml).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(back.state.key_mode, state.key_mode);
        assert_eq!(back.state.split_point, state.split_point);
        for p in PatchParam::all() {
            assert_eq!(
                back.state.params.layer(Layer::Upper).get(p),
                state.params.layer(Layer::Upper).get(p),
                "upper {} drift",
                p.desc().name
            );
            assert_eq!(
                back.state.params.layer(Layer::Lower).get(p),
                state.params.layer(Layer::Lower).get(p),
                "lower {} drift",
                p.desc().name
            );
        }
        for g in GlobalParam::all() {
            assert_eq!(
                back.state.params.global.get(g),
                state.params.global.get(g),
                "global {} drift",
                g.desc().name
            );
        }
    }

    // An engine-written (desktop) preset applies through the vxn-app reader,
    // reproducing every param — the forward "imports on web" direction.
    #[test]
    fn engine_write_applies_through_app_reader() {
        let state = dense_state();
        let engine_toml = Performance {
            meta: Meta {
                name: "Desktop".into(),
                ..Meta::default()
            },
            state: state.clone(),
        }
        .to_toml_string();

        let shared = SharedParams::new();
        let (meta, warnings) = vxn_app::read_toml_into(&shared, &engine_toml).unwrap();
        assert_eq!(meta.name, "Desktop");
        assert!(warnings.is_empty(), "{warnings:?}");

        let back = shared.to_state();
        assert_eq!(back.key_mode, state.key_mode);
        assert_eq!(back.split_point, state.split_point);
        for p in PatchParam::all() {
            assert_eq!(
                back.params.layer(Layer::Upper).get(p),
                state.params.layer(Layer::Upper).get(p),
            );
            assert_eq!(
                back.params.layer(Layer::Lower).get(p),
                state.params.layer(Layer::Lower).get(p),
            );
        }
        for g in GlobalParam::all() {
            assert_eq!(back.params.global.get(g), state.params.global.get(g));
        }
    }

    // A sparse default preset round-trips to defaults through the app reader, and
    // the app reader resets a dirty model first (omitted params land on default).
    #[test]
    fn app_reader_resets_omitted_params_to_default() {
        // A near-default state: only one param deviates.
        let mut params = ParamValues::default();
        params.layer_mut(Layer::Upper).set(PatchParam::Cutoff, 1234.0);
        let state = PluginState {
            params,
            key_mode: KeyMode::Whole,
            split_point: 60,
        };
        let toml = Performance {
            meta: Meta { name: "Sparse".into(), ..Meta::default() },
            state,
        }
        .to_toml_string();

        // A model pre-loaded with a DIFFERENT dense patch.
        let shared = SharedParams::new();
        shared.load_performance(&dense_state());
        vxn_app::read_toml_into(&shared, &toml).unwrap();

        let back = shared.to_state();
        // The one set param took; everything else reset to default.
        assert_eq!(back.params.layer(Layer::Upper).get(PatchParam::Cutoff), 1234.0);
        let def = PatchValues::default();
        assert_eq!(
            back.params.layer(Layer::Lower).get(PatchParam::Cutoff),
            def.get(PatchParam::Cutoff),
            "lower cutoff should have reset to default"
        );
        assert_eq!(back.key_mode, KeyMode::Whole);
    }

    // A malformed / wrong-schema blob is a hard error and does NOT mutate the
    // model (parse fails before the commit) — the graceful-rejection acceptance.
    #[test]
    fn app_reader_rejects_garbage_without_mutating() {
        let shared = SharedParams::new();
        shared.load_performance(&dense_state());
        let before = shared.to_state();

        assert!(vxn_app::read_toml_into(&shared, "not = valid = toml").is_err());
        assert!(
            vxn_app::read_toml_into(&shared, "schema = 999\n[meta]\nname='x'").is_err(),
            "wrong schema rejected"
        );

        // Model untouched by the failed reads.
        let after = shared.to_state();
        assert_eq!(before.split_point, after.split_point);
        assert_eq!(
            before.params.layer(Layer::Upper).get(PatchParam::Cutoff),
            after.params.layer(Layer::Upper).get(PatchParam::Cutoff),
        );
    }
}
