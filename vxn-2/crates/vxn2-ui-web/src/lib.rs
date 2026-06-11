//! VXN2 HTML faceplate backend (ticket 0023 / epic E003).
//!
//! Thin wrapper over [`vxn_core_ui_web`]: splices VXN2's HTML / CSS / JS
//! into one string, builds a [`vxn_core_ui_web::WebEditorConfig`] with
//! VXN2-specific `parse_custom_ui` + `serialise_custom_view` hooks, then
//! calls `vxn_core_ui_web::open_editor`. The WebView lifecycle, IPC
//! bridge, batched view-event sink, corpus snapshot push, and macOS
//! text-input popup all live in core — this crate doesn't touch wry or
//! raw-window-handle directly.
//!
//! Asset bundle (`assets/`) is embedded via `include_str!` at build time
//! so the cdylib is self-contained — no fs dependency at run time.

use std::ffi::c_void;
use std::sync::Arc;

use serde_json::Value as JsonValue;
use vxn2_app::{MatrixRow, UiEvent, Vxn2UiCustom, Vxn2ViewCustom};
use vxn_core_app::{ControllerHandle, CorpusHandle};

pub use vxn_core_ui_web::{EditorHandle, prompt_text};

use vxn_core_ui_web::{
    DEFAULT_MAX_BATCH_BYTES, WebEditorConfig, descriptor_to_json, open_editor as core_open_editor,
};

/// Logical pixel dimensions of the VXN2 editor — matches
/// `ui-mockup/index.html`. Exposed so `vxn2-clap`'s `gui::get_size` can
/// hand the host the right surface dimensions.
pub const EDITOR_WIDTH: u32 = 1024;
pub const EDITOR_HEIGHT: u32 = 772;

const HTML_TEMPLATE: &str = include_str!("../assets/index.html");
const FACEPLATE_CSS: &str = include_str!("../assets/style.css");
const BOOTSTRAP_JS: &str = include_str!("../assets/bootstrap.js");
const PANEL_KNOB_JS: &str = include_str!("../assets/panels/knob.js");
const PANEL_FADER_JS: &str = include_str!("../assets/panels/fader.js");
const PANEL_BUTTON_GROUP_JS: &str = include_str!("../assets/panels/button-group.js");
const PANEL_GRAPH_JS: &str = include_str!("../assets/panels/graph.js");
const PANEL_ALGO_DIAGRAM_JS: &str = include_str!("../assets/panels/algo-diagram.js");
const PANEL_OP_ROW_JS: &str = include_str!("../assets/panels/op-row.js");
const PANEL_MOD_MATRIX_JS: &str = include_str!("../assets/panels/mod-matrix.js");
const PANEL_PRESET_BAR_JS: &str = include_str!("../assets/panels/preset-bar.js");
const PANEL_PRESET_BROWSER_JS: &str = include_str!("../assets/panels/preset-browser.js");
const MAIN_JS: &str = include_str!("../assets/main.js");

/// Open the VXN2 editor under `parent`. Wraps
/// [`vxn_core_ui_web::open_editor`] with VXN2's HTML and custom-event
/// hooks.
///
/// `parent` is the same raw pointer the clack shell extracts from the
/// host's `gui::set_parent` (NSView / HWND / xcb window id).
pub fn open_editor(
    parent: *mut c_void,
    ctrl: ControllerHandle,
    corpus: CorpusHandle,
) -> EditorHandle {
    let html = build_faceplate_html();
    let mut config = WebEditorConfig::new(html, EDITOR_WIDTH, EDITOR_HEIGHT);
    config.uncategorised_label = "Uncategorised";
    config.max_batch_bytes = DEFAULT_MAX_BATCH_BYTES;
    config.webview2_vendor = Some("Vulpus");
    config.webview2_product = Some("VXN2");
    config.parse_custom_ui = Some(Arc::new(parse_custom_ui));
    config.serialise_custom_view = Some(Arc::new(serialise_custom_view));
    core_open_editor(parent, ctrl, corpus, config)
}

/// Splice CSS + the concatenated JS bundle into the HTML template. JS
/// load order matches the dependency chain: bootstrap (declares
/// `window.__vxn`) -> panel primitives (attach to `__vxn.panels`) ->
/// main (binds DOM, replaces the stub event handlers). `__PARAMS_JSON__`
/// in bootstrap is substituted with the descriptor table so the page
/// hydrates without an extra IPC round-trip.
///
/// Asset source: by default every part of the page comes from the
/// crate's `include_str!` embed (production behaviour, no fs
/// dependency). With `VXN2_DEV_ASSETS=1` set in the host's
/// environment and a `Contents/Resources/` directory present next
/// to the loaded cdylib (the xtask `bundle` step stages it there),
/// the editor instead reads each asset from disk so a CSS / JS edit
/// is visible after a re-open of the editor without recompiling the
/// cdylib. Per-file fallback: a missing file silently falls back to
/// the embed, so a partial Resources/ tree doesn't break the page.
fn build_faceplate_html() -> String {
    let params_json = build_params_json();
    let matrix_lists_json = build_matrix_lists_json();
    let dev = dev_assets_dir();
    let default_patch_json = build_default_patch_json();
    let bootstrap = asset(dev.as_deref(), "bootstrap.js", BOOTSTRAP_JS)
        .replace("__PARAMS_JSON__", &params_json)
        .replace("__MATRIX_LISTS_JSON__", &matrix_lists_json)
        .replace("__DEFAULT_PATCH_JSON__", &default_patch_json)
        .replace("__SUBDIVISIONS_JSON__", &build_subdivisions_json());
    let js_bundle = [
        bootstrap,
        asset(dev.as_deref(), "panels/knob.js", PANEL_KNOB_JS),
        asset(dev.as_deref(), "panels/fader.js", PANEL_FADER_JS),
        asset(dev.as_deref(), "panels/button-group.js", PANEL_BUTTON_GROUP_JS),
        asset(dev.as_deref(), "panels/graph.js", PANEL_GRAPH_JS),
        asset(dev.as_deref(), "panels/algo-diagram.js", PANEL_ALGO_DIAGRAM_JS),
        asset(dev.as_deref(), "panels/op-row.js", PANEL_OP_ROW_JS),
        asset(dev.as_deref(), "panels/mod-matrix.js", PANEL_MOD_MATRIX_JS),
        asset(dev.as_deref(), "panels/preset-bar.js", PANEL_PRESET_BAR_JS),
        // Shared two-pane preset browser (vxn-core-ui-web), ESM markers
        // stripped, spliced immediately before its VXN2 glue (which calls
        // the `createPresetBrowser` it defines).
        vxn_core_ui_web::strip_esm_exports(vxn_core_ui_web::PRESET_BROWSER_JS),
        asset(dev.as_deref(), "panels/preset-browser.js", PANEL_PRESET_BROWSER_JS),
        asset(dev.as_deref(), "main.js", MAIN_JS),
    ]
    .join("\n;\n");
    let html_tpl = asset(dev.as_deref(), "index.html", HTML_TEMPLATE);
    let css = format!(
        "{}\n{}",
        asset(dev.as_deref(), "style.css", FACEPLATE_CSS),
        vxn_core_ui_web::PRESET_BROWSER_CSS,
    );
    html_tpl
        .replace("__CSS__", &css)
        .replace("__BOOTSTRAP_JS__", &js_bundle)
}

/// Read `relative` from `dev_dir` (if any) and fall back to `embedded`
/// on any error. Pulled out so each asset slot in
/// [`build_faceplate_html`] is a one-liner.
fn asset(dev_dir: Option<&std::path::Path>, relative: &str, embedded: &'static str) -> String {
    if let Some(dir) = dev_dir {
        if let Ok(s) = std::fs::read_to_string(dir.join(relative)) {
            return s;
        }
    }
    embedded.to_string()
}

/// `Contents/Resources/` next to the loaded cdylib, IF the host set
/// `VXN2_DEV_ASSETS=1` AND we can locate it AND the directory exists.
/// Returns `None` otherwise — the embed is the safe fallback.
fn dev_assets_dir() -> Option<std::path::PathBuf> {
    if std::env::var("VXN2_DEV_ASSETS").ok().as_deref() != Some("1") {
        return None;
    }
    let resources = bundle_resources_dir()?;
    if resources.is_dir() {
        Some(resources)
    } else {
        None
    }
}

/// Resolve `Contents/Resources/` relative to the cdylib on disk. Uses
/// `dladdr(this fn's address)` to read the loaded module's path —
/// `Path::current_exe` is the host (Bitwig, Reaper), not the plugin.
#[cfg(target_os = "macos")]
fn bundle_resources_dir() -> Option<std::path::PathBuf> {
    use std::ffi::{CStr, c_void};
    #[repr(C)]
    struct DlInfo {
        dli_fname: *const i8,
        dli_fbase: *mut c_void,
        dli_sname: *const i8,
        dli_saddr: *mut c_void,
    }
    unsafe extern "C" {
        fn dladdr(addr: *const c_void, info: *mut DlInfo) -> i32;
    }
    let mut info = DlInfo {
        dli_fname: std::ptr::null(),
        dli_fbase: std::ptr::null_mut(),
        dli_sname: std::ptr::null(),
        dli_saddr: std::ptr::null_mut(),
    };
    let here = bundle_resources_dir as *const c_void;
    let ok = unsafe { dladdr(here, &mut info) };
    if ok == 0 || info.dli_fname.is_null() {
        return None;
    }
    let path = unsafe { CStr::from_ptr(info.dli_fname) }.to_str().ok()?;
    let p = std::path::PathBuf::from(path);
    // .../Contents/MacOS/<plugin>  ->  .../Contents/Resources/
    Some(p.parent()?.parent()?.join("Resources"))
}

#[cfg(not(target_os = "macos"))]
fn bundle_resources_dir() -> Option<std::path::PathBuf> {
    // Windows / Linux dev-asset hot-reload is a follow-up; the embed
    // path covers everyone today.
    None
}

/// Build a `ParseCustomUi` closure pointing at this crate's
/// VXN2-specific opcode parser. Exposed for the editor smoke test
/// (ticket 0032) so an in-process test can drive the same IPC parse
/// path the live WebView uses without spinning up a wry instance.
pub fn parse_custom_ui_for_test() -> vxn_core_ui_web::ParseCustomUi {
    Arc::new(parse_custom_ui)
}

/// Parse a VXN2-specific opcode that the shared vocabulary doesn't carry.
/// `v` is the already-parsed IPC JSON payload. Returns `None` on unknown
/// opcodes — `vxn_core_ui_web::parse_ui_event` falls through and drops.
fn parse_custom_ui(op: &str, v: &JsonValue) -> Option<UiEvent> {
    match op {
        "set_op_tab" => {
            let op = v.get("op")?.as_u64()? as u8;
            Some(UiEvent::Custom(Box::new(Vxn2UiCustom::SetOpTab { op })))
        }
        "set_matrix_row" => {
            let slot = v.get("slot")?.as_u64()? as u8;
            let row = matrix_row_from_json(v.get("row")?)?;
            Some(UiEvent::Custom(Box::new(Vxn2UiCustom::SetMatrixRow {
                slot,
                row,
            })))
        }
        "request_matrix_snapshot" => Some(UiEvent::Custom(Box::new(
            Vxn2UiCustom::RequestMatrixSnapshot,
        ))),
        "request_full_rebroadcast" => Some(UiEvent::Custom(Box::new(
            Vxn2UiCustom::RequestFullRebroadcast,
        ))),
        _ => None,
    }
}

/// Serialise a [`Vxn2ViewCustom`] payload to the JSON shape the page
/// dispatcher expects. `payload` is the raw `Box<dyn Any>` content from
/// `ViewEvent::Custom`.
fn serialise_custom_view(payload: &dyn std::any::Any) -> Option<JsonValue> {
    let custom = payload.downcast_ref::<Vxn2ViewCustom>()?;
    Some(match custom {
        Vxn2ViewCustom::OpTabChanged { op } => serde_json::json!({
            "kind": "op_tab_changed",
            "op": op,
        }),
        Vxn2ViewCustom::MatrixRowChanged { slot, row } => serde_json::json!({
            "kind": "matrix_row_changed",
            "slot": slot,
            "row": matrix_row_to_json(*row),
        }),
        Vxn2ViewCustom::MatrixSnapshot { rows } => serde_json::json!({
            "kind": "matrix_snapshot",
            "rows": rows.iter().map(|r| matrix_row_to_json(*r)).collect::<Vec<_>>(),
        }),
    })
}

fn matrix_row_from_json(v: &JsonValue) -> Option<MatrixRow> {
    Some(MatrixRow {
        source: v.get("source")?.as_u64()? as u8,
        dest: v.get("dest")?.as_u64()? as u8,
        curve: v.get("curve")?.as_u64()? as u8,
        active: v.get("active")?.as_bool()?,
        depth: v.get("depth")?.as_f64()? as f32,
    })
}

fn matrix_row_to_json(row: MatrixRow) -> JsonValue {
    serde_json::json!({
        "source": row.source,
        "dest": row.dest,
        "curve": row.curve,
        "active": row.active,
        "depth": row.depth,
    })
}

// ── Params JSON helper ──────────────────────────────────────────────────────

/// Walk the engine's CLAP-id-indexed parameter table and emit one JSON
/// array entry per id, in the shape `vxn_core_ui_web::descriptor_to_json`
/// produces. The page hydrates this into `__vxn.params` on first batch
/// (0026); each entry carries `name` (machine id), `label`, `min`, `max`,
/// `default`, `kind`, and either `unit` or `variants`.
///
/// Returns a JSON string ready to splice into the HTML or hand to JS via
/// `evaluate_script`. Production builds embed this at module load; a
/// `VXN2_DEV_ASSETS=1` mode (ticket 0032) reads from disk instead.
/// Walk the engine's mod-matrix enum tables and emit the source / dest
/// / curve pick-lists the page populates the `<select>`s from. Shape:
///
/// ```json
/// {"sources": [{"id": N, "name": "...", "label": "..."}, ...],
///  "dests":   [...],
///  "curves":  [...]}
/// ```
///
/// `id` is the engine's `SourceId` / `DestId` / `CurveKind` u8
/// discriminant — what `matrix_row_from_json` decodes back. The page
/// never invents indices; it picks from this table.
pub fn build_matrix_lists_json() -> String {
    use vxn2_engine::matrix::{
        CURVE_LABELS, CURVE_NAMES, DEST_LABELS, DEST_NAMES, SOURCE_LABELS, SOURCE_NAMES,
    };
    let sources: Vec<JsonValue> = SOURCE_NAMES
        .iter()
        .zip(SOURCE_LABELS.iter())
        .enumerate()
        .map(|(i, (n, l))| serde_json::json!({ "id": i, "name": n, "label": l }))
        .collect();
    let dests: Vec<JsonValue> = DEST_NAMES
        .iter()
        .zip(DEST_LABELS.iter())
        .enumerate()
        .map(|(i, (n, l))| serde_json::json!({ "id": i, "name": n, "label": l }))
        .collect();
    let curves: Vec<JsonValue> = CURVE_NAMES
        .iter()
        .zip(CURVE_LABELS.iter())
        .enumerate()
        .map(|(i, (n, l))| serde_json::json!({ "id": i, "name": n, "label": l }))
        .collect();
    serde_json::json!({ "sources": sources, "dests": dests, "curves": curves }).to_string()
}

pub fn build_params_json() -> String {
    let entries: Vec<JsonValue> = (0..vxn2_engine::TOTAL_PARAMS)
        .filter_map(|i| {
            let desc = vxn2_engine::params::core_desc_for_clap_id(i)?;
            let mut v = descriptor_to_json(desc);
            if let Some(obj) = v.as_object_mut() {
                obj.insert("id".into(), serde_json::json!(i));
            }
            Some(v)
        })
        .collect();
    serde_json::Value::Array(entries).to_string()
}

/// Plain-value-per-CLAP-id array from `default_patch::default_param_values`.
/// Spliced into bootstrap as `__DEFAULT_PATCH_JSON__` so the UI can seed
/// its live-value cache. In the running plugin this is overwritten by the
/// engine's NaN-diff snapshot at boot; in the offline HTML dump it's the
/// only source of values that vary per-op.
pub fn build_default_patch_json() -> String {
    let values = vxn2_engine::default_patch::default_param_values();
    serde_json::Value::Array(values.iter().map(|v| serde_json::json!(*v)).collect())
        .to_string()
}

/// Tempo-sync subdivision labels (`vxn2_dsp::lfo::SUBDIVISIONS`, coarse→fine)
/// spliced into bootstrap as `__SUBDIVISIONS_JSON__`. A synced rate/time
/// fader reads this list while dragging to show the division its position
/// selects — the same mapping `sync_aware_display` uses on the Rust side, so
/// the live drag label and the engine echo agree. See `panels/fader.js`.
pub fn build_subdivisions_json() -> String {
    let labels: Vec<JsonValue> = vxn2_engine::SUBDIVISIONS
        .iter()
        .map(|s| serde_json::json!(s.label))
        .collect();
    serde_json::Value::Array(labels).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_custom_set_op_tab() {
        let v = serde_json::json!({ "op": "set_op_tab", "op": 3 });
        let ev = parse_custom_ui("set_op_tab", &v).expect("parsed");
        match ev {
            UiEvent::Custom(p) => match p.downcast::<Vxn2UiCustom>().map(|b| *b) {
                Ok(Vxn2UiCustom::SetOpTab { op: 3 }) => {}
                other => panic!("unexpected payload: {other:?}"),
            },
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn parse_custom_set_matrix_row() {
        let v = serde_json::json!({
            "op": "set_matrix_row",
            "slot": 7,
            "row": {
                "source": 2, "dest": 17, "curve": 1, "active": true, "depth": 0.5
            }
        });
        let ev = parse_custom_ui("set_matrix_row", &v).expect("parsed");
        match ev {
            UiEvent::Custom(p) => match p.downcast::<Vxn2UiCustom>().map(|b| *b) {
                Ok(Vxn2UiCustom::SetMatrixRow { slot: 7, row }) => {
                    assert_eq!(row.source, 2);
                    assert_eq!(row.dest, 17);
                    assert_eq!(row.curve, 1);
                    assert!(row.active);
                    assert!((row.depth - 0.5).abs() < 1e-5);
                }
                other => panic!("unexpected payload: {other:?}"),
            },
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn parse_custom_unknown_opcode_returns_none() {
        let v = serde_json::json!({ "op": "explode" });
        assert!(parse_custom_ui("explode", &v).is_none());
    }

    #[test]
    fn serialise_view_matrix_row_changed_round_trip_shape() {
        let payload = Vxn2ViewCustom::MatrixRowChanged {
            slot: 4,
            row: MatrixRow {
                source: 1,
                dest: 2,
                curve: 0,
                active: true,
                depth: -0.3,
            },
        };
        let v = serialise_custom_view(&payload).expect("serialised");
        assert_eq!(v["kind"], "matrix_row_changed");
        assert_eq!(v["slot"], 4);
        assert_eq!(v["row"]["dest"], 2);
        assert!((v["row"]["depth"].as_f64().unwrap() - (-0.3)).abs() < 1e-5);
    }

    #[test]
    fn build_params_json_covers_full_table() {
        let s = build_params_json();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let arr = v.as_array().expect("array");
        assert_eq!(arr.len(), vxn2_engine::TOTAL_PARAMS);
        for (i, entry) in arr.iter().enumerate() {
            assert!(entry.get("name").is_some(), "missing name at {i}");
            assert!(entry.get("label").is_some(), "missing label at {i}");
            assert!(entry.get("kind").is_some(), "missing kind at {i}");
            assert_eq!(entry["id"].as_u64().unwrap() as usize, i);
        }
    }

    #[test]
    fn build_faceplate_html_splices_css_and_bootstrap() {
        let html = build_faceplate_html();
        assert!(html.contains("VXN2"));
        assert!(html.contains("color-scheme: dark"));
        assert!(html.contains("window.__vxn"));
        assert!(!html.contains("__CSS__"));
        assert!(!html.contains("__BOOTSTRAP_JS__"));
    }

    /// 0025 AC: shell carries the static faceplate markup, no inline
    /// handlers, no placeholder values inlined into style attributes, all
    /// sections + a representative cross-section of params reachable via
    /// `querySelector` by 0026.
    #[test]
    fn faceplate_html_shell_meets_0025_acceptance() {
        let html = HTML_TEMPLATE;
        for bad in ["onclick=", "oninput=", "onchange=", "onmousedown="] {
            assert!(!html.contains(bad), "stale inline handler: {bad}");
        }
        assert!(!html.contains("style=\"height:"), "placeholder fader height inlined");
        assert!(!html.contains("style=\"bottom:"), "placeholder fader thumb inlined");
        assert!(html.contains("class=\"vxn-faceplate\""));
        assert!(html.contains("data-vxn-section=\"faceplate\""));
        for section in [
            "preset-bar", "algo-block", "algo-diagram", "algo-svg",
            "op-tabs", "op-detail",
            "lfo1", "lfo2", "pitch-eg", "peg-svg", "mod-env",
            "voice", "stack", "delay", "reverb", "master",
            "algo-overlay", "algo-grid", "mod-matrix", "mm-overlay-list",
        ] {
            let needle = format!("data-vxn-section=\"{section}\"");
            assert!(html.contains(&needle), "missing section: {section}");
        }
        for id in [
            "algo", "lfo1-rate", "lfo1-shape", "lfo1-sync",
            "lfo2-rate", "lfo2-sync",
            "mod-env-a", "mod-env-r", "mod-env-shape",
            "assign-mode", "glide-time", "legato",
            "stack-density", "stack-distrib",
            "delay-on", "delay-time", "delay-feedback", "delay-mix",
            "delay-sync",
            "reverb-on", "reverb-size", "reverb-mix",
            "master-tune", "master-volume",
        ] {
            let needle = format!("data-vxn-param=\"{id}\"");
            assert!(html.contains(&needle), "missing param: {id}");
        }
        for custom in [
            "preset_prev", "preset_next", "preset_browse",
            "preset_save", "preset_save_as",
            "open_algo_picker", "close_algo_picker",
            "open_mod_matrix", "close_mod_matrix",
        ] {
            let needle = format!("data-vxn-custom=\"{custom}\"");
            assert!(html.contains(&needle), "missing custom dispatch: {custom}");
        }
        // Overlays start hidden — 0027 / 0028 toggle via the [hidden]
        // attribute. Tolerate intervening attrs (data-vxn-role etc.).
        assert!(
            html.contains("data-vxn-section=\"algo-overlay\"")
                && html.contains("\"algo-overlay\"") && html.contains(" hidden>"),
            "algo-overlay must start hidden",
        );
        assert!(
            html.contains("data-vxn-section=\"mod-matrix\""),
            "mod-matrix section missing",
        );
        // mod-matrix carries `hidden` (with optional intervening
        // data-vxn-role from the panel binder).
        let mm_idx = html.find("data-vxn-section=\"mod-matrix\"").unwrap();
        let tag_close = html[mm_idx..].find('>').unwrap();
        assert!(
            html[mm_idx..mm_idx + tag_close].contains("hidden"),
            "mod-matrix must start hidden",
        );
    }

    /// CSS file carries the viewport dims + structural rules ported from
    /// the mockup.
    #[test]
    fn faceplate_css_carries_mockup_rules() {
        assert!(FACEPLATE_CSS.contains("--editor-w: 1024px"));
        assert!(FACEPLATE_CSS.contains("--editor-h: 772px"));
        assert!(FACEPLATE_CSS.contains(".vxn-faceplate"));
        assert!(FACEPLATE_CSS.contains(".op-row"));
        assert!(FACEPLATE_CSS.contains(".gmod-row"));
        assert!(FACEPLATE_CSS.contains(".perf-row"));
        assert!(FACEPLATE_CSS.contains(".overlay-backdrop"));
        assert!(FACEPLATE_CSS.contains(".algo-grid"));
        assert!(FACEPLATE_CSS.contains(".mm-overlay"));
    }

    /// Bootstrap declares the surface main.js + panels.js attach to;
    /// main.js (loaded later in the same script) is what fires `ready`.
    #[test]
    fn bootstrap_js_declares_required_surface() {
        assert!(BOOTSTRAP_JS.contains("window.__vxn"));
        assert!(BOOTSTRAP_JS.contains("applyViewEvents"));
        assert!(BOOTSTRAP_JS.contains("applyPresetCorpus"));
        assert!(BOOTSTRAP_JS.contains("paramsByName"));
        assert!(BOOTSTRAP_JS.contains("__PARAMS_JSON__"));
        assert!(BOOTSTRAP_JS.contains("panels"));
    }

    /// Panel primitives and main bootstrap carry the contracts main.js +
    /// the section renderers depend on.
    #[test]
    fn panel_js_files_carry_expected_exports() {
        assert!(PANEL_KNOB_JS.contains("__vxn.panels.knob"));
        assert!(PANEL_FADER_JS.contains("__vxn.panels.fader"));
        assert!(PANEL_FADER_JS.contains("paramToNorm"));
        assert!(PANEL_BUTTON_GROUP_JS.contains("__vxn.panels.buttonGroup"));
        assert!(PANEL_BUTTON_GROUP_JS.contains("createToggleHeader"));
        assert!(PANEL_GRAPH_JS.contains("__vxn.panels.graph"));
        assert!(PANEL_ALGO_DIAGRAM_JS.contains("__vxn.panels.algoDiagram"));
        assert!(PANEL_OP_ROW_JS.contains("__vxn.panels.opRow"));
        assert!(PANEL_OP_ROW_JS.contains("ALGO_CARRIERS"));
        assert!(MAIN_JS.contains("dispatch(\"ready\")"));
        assert!(MAIN_JS.contains("begin_gesture"));
        assert!(MAIN_JS.contains("set_param_norm"));
        assert!(MAIN_JS.contains("end_gesture"));
        assert!(MAIN_JS.contains("request_text_input"));
        assert!(MAIN_JS.contains("text_input_result"));
        assert!(MAIN_JS.contains("panels.opRow"));
        // 0030: text-input Promise + Linux fallback are part of the
        // bundled surface, not external infrastructure.
        assert!(MAIN_JS.contains("dispatchTextInput"));
        assert!(MAIN_JS.contains("resolveTextInput"));
        assert!(MAIN_JS.contains("showFallbackDialog"));
        assert!(MAIN_JS.contains("vxn-text-input-fallback"));
        // The numeric-entry path brackets the commit with begin_gesture /
        // end_gesture so host automation sees a clean atomic write.
        assert!(
            MAIN_JS.contains("begin_gesture")
                && MAIN_JS.contains("set_param")
                && MAIN_JS.contains("end_gesture"),
            "numeric-entry gesture bracketing missing",
        );
    }

    /// Dumps the spliced HTML to `/tmp/vxn2-faceplate.html` for manual
    /// inspection. Ignored by default.
    #[test]
    #[ignore]
    fn dump_spliced_html() {
        let html = build_faceplate_html();
        std::fs::write("/tmp/vxn2-faceplate.html", &html).expect("write dump");
        let start = html.find("<script>").expect("script start");
        let end = html[start..].find("</script>").expect("script end");
        let js = &html[start + "<script>".len()..start + end];
        std::fs::write("/tmp/vxn2-bundle.js", js).expect("write js");
    }

    /// Bundle composition: params JSON spliced into bootstrap, all panel
    /// files concatenated, no placeholder tokens left in the served HTML.
    #[test]
    fn build_faceplate_html_bundles_full_js_stack() {
        let html = build_faceplate_html();
        assert!(!html.contains("__PARAMS_JSON__"));
        assert!(!html.contains("__BOOTSTRAP_JS__"));
        assert!(html.contains("__vxn.panels.knob"));
        assert!(html.contains("__vxn.panels.fader"));
        assert!(html.contains("__vxn.panels.buttonGroup"));
        assert!(html.contains("__vxn.panels.graph"));
        assert!(html.contains("__vxn.panels.algoDiagram"));
        assert!(html.contains("__vxn.panels.opRow"));
        assert!(html.contains("dispatch(\"ready\")"));
        // Params JSON spliced — first descriptor's machine id is `op1-num`
        // post-flatten. Cheap structural canary.
        assert!(html.contains("\"op1-num\""));
        assert!(html.contains("\"lfo1-rate\""));
        assert!(html.contains("\"master-volume\""));
    }

    /// The 32 x 6 JS carrier table embedded in `panels/op-row.js`
    /// MUST match the engine's algorithm routing table — drift would
    /// silently colour the op tabs wrong. Parses the JS literal back
    /// out (assumes a stable formatting; the parser is intentionally
    /// strict so editing the table without keeping the comment markers
    /// breaks the test loudly).
    #[test]
    fn op_row_carriers_match_engine_table() {
        let js = PANEL_OP_ROW_JS;
        // Extract the ALGO_CARRIERS literal — between
        // `const ALGO_CARRIERS = [` and the matching `];`.
        let start = js.find("const ALGO_CARRIERS = [")
            .expect("ALGO_CARRIERS declaration");
        let body_start = start + "const ALGO_CARRIERS = [".len();
        let body_rest = &js[body_start..];
        let body_end = body_rest.find("];").expect("close of array");
        let body = &body_rest[..body_end];

        // Each row looks like `[true, false, true, false, false, false],`
        // — extract one row per `[…]` group.
        let mut rows: Vec<[bool; 6]> = Vec::new();
        for raw in body.split(']') {
            let trimmed = raw.trim_start_matches(|c: char| c != '[');
            if trimmed.is_empty() {
                continue;
            }
            let inner = trimmed.trim_start_matches('[');
            let mut row = [false; 6];
            let mut col = 0usize;
            for tok in inner.split(',') {
                let t = tok.trim();
                if t.is_empty() {
                    continue;
                }
                if t == "true" {
                    assert!(col < 6, "row overflow in ALGO_CARRIERS");
                    row[col] = true;
                    col += 1;
                } else if t == "false" {
                    assert!(col < 6, "row overflow in ALGO_CARRIERS");
                    row[col] = false;
                    col += 1;
                } else {
                    // Stray tokens (comments / whitespace) — ignore.
                }
            }
            if col == 6 {
                rows.push(row);
            }
        }

        assert_eq!(
            rows.len(),
            vxn2_dsp::algo::N_ALGOS,
            "ALGO_CARRIERS row count must match engine N_ALGOS",
        );
        for (i, row) in rows.iter().enumerate() {
            let spec = vxn2_dsp::algo::ALGOS[i];
            for op in 0..6 {
                let engine = (spec.carriers & (1u8 << op)) != 0;
                assert_eq!(
                    row[op], engine,
                    "ALGO_CARRIERS mismatch at algo {} (1-indexed), op {} (1-indexed)",
                    i + 1,
                    op + 1,
                );
            }
        }
    }

    /// Parity check for `ALGO_FB_OPS` in `op-row.js`. Mirrors
    /// `AlgoSpec::structural_fb_op` across the 32 algorithms — drift would
    /// silently disable the wrong op's feedback fader.
    #[test]
    fn op_row_fb_ops_match_engine_table() {
        let js = PANEL_OP_ROW_JS;
        let start = js.find("const ALGO_FB_OPS = [")
            .expect("ALGO_FB_OPS declaration");
        let body_start = start + "const ALGO_FB_OPS = [".len();
        let body_rest = &js[body_start..];
        let body_end = body_rest.find("];").expect("close of array");
        let body = &body_rest[..body_end];

        let mut vals: Vec<u8> = Vec::new();
        for tok in body.split(',') {
            let t = tok.trim();
            if t.is_empty() {
                continue;
            }
            if let Ok(n) = t.parse::<u8>() {
                vals.push(n);
            }
        }
        assert_eq!(
            vals.len(),
            vxn2_dsp::algo::N_ALGOS,
            "ALGO_FB_OPS count must match engine N_ALGOS",
        );
        for (i, v) in vals.iter().enumerate() {
            assert_eq!(
                *v,
                vxn2_dsp::algo::ALGOS[i].structural_fb_op,
                "ALGO_FB_OPS mismatch at algo {} (1-indexed)",
                i + 1,
            );
        }
    }

    #[test]
    fn parse_custom_request_matrix_snapshot() {
        let v = serde_json::json!({ "op": "request_matrix_snapshot" });
        let ev = parse_custom_ui("request_matrix_snapshot", &v).expect("parsed");
        match ev {
            UiEvent::Custom(p) => match p.downcast::<Vxn2UiCustom>().map(|b| *b) {
                Ok(Vxn2UiCustom::RequestMatrixSnapshot) => {}
                other => panic!("unexpected payload: {other:?}"),
            },
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn serialise_view_matrix_snapshot_shape() {
        let row = MatrixRow {
            source: 3,
            dest: 5,
            curve: 2,
            active: true,
            depth: 0.25,
        };
        let payload = Vxn2ViewCustom::MatrixSnapshot { rows: [row; 16] };
        let v = serialise_custom_view(&payload).expect("serialised");
        assert_eq!(v["kind"], "matrix_snapshot");
        let rows = v["rows"].as_array().expect("rows array");
        assert_eq!(rows.len(), 16);
        assert_eq!(rows[0]["dest"], 5);
        assert_eq!(rows[15]["source"], 3);
    }

    #[test]
    fn build_matrix_lists_json_includes_all_enum_widths() {
        let s = build_matrix_lists_json();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["sources"].as_array().unwrap().len(), 12);
        assert_eq!(v["dests"].as_array().unwrap().len(), 28);
        assert_eq!(v["curves"].as_array().unwrap().len(), 4);
        assert_eq!(v["sources"][0]["name"], "none");
        assert_eq!(v["sources"][1]["name"], "lfo1");
        assert_eq!(v["dests"][26]["name"], "reverb-mix");
        assert_eq!(v["dests"][27]["name"], "feedback");
        assert_eq!(v["curves"][3]["name"], "bipolar");
    }

    #[test]
    fn build_faceplate_html_includes_matrix_panel_and_lists() {
        let html = build_faceplate_html();
        assert!(html.contains("__vxn.panels.modMatrix"), "modMatrix panel missing");
        assert!(html.contains("\"reverb-mix\""), "matrix dest list missing");
        assert!(!html.contains("__MATRIX_LISTS_JSON__"));
    }

    /// 0029 AC: preset-bar panel + browse-dialog markup must be in the
    /// served HTML; main.js must route preset_loaded / status through
    /// the panel.
    #[test]
    fn build_faceplate_html_includes_preset_bar_panel_and_dialog() {
        let html = build_faceplate_html();
        assert!(
            html.contains("__vxn.panels.presetBar"),
            "presetBar panel missing from bundle",
        );
        assert!(
            html.contains("data-vxn-section=\"preset-name\""),
            "preset-name display missing",
        );
        assert!(
            html.contains("data-vxn-section=\"toast\""),
            "toast element missing",
        );
        assert!(
            html.contains("data-vxn-section=\"browse-dialog\""),
            "browse-dialog missing",
        );
        // main.js routes the preset_loaded + status view events to the
        // panel rather than dropping them.
        assert!(
            html.contains("panels.presetBar.onView"),
            "main.js missing preset-bar onView route",
        );
        // Save As awaits `dispatchTextInput` directly; the old
        // correlation-token + router-callback path is gone.
        assert!(
            html.contains("dispatchTextInput(\"Save Preset As\""),
            "preset-bar Save As not wired through dispatchTextInput",
        );
        assert!(
            !html.contains("onTextInputResult"),
            "preset-bar still exposes onTextInputResult shim",
        );
        // dispatchTextInput is the page-side Promise primitive added in
        // 0030; it MUST be present for any text-input round trip.
        assert!(
            html.contains("vxn.dispatchTextInput = dispatchTextInput"),
            "dispatchTextInput not installed on vxn",
        );
    }

    /// Preset browser (two-pane, ported from VXN1): panel JS + floating
    /// folders/presets markup must be in the served HTML, and main.js must
    /// forward the corpus, highlight the loaded preset, and follow moves.
    #[test]
    fn build_faceplate_html_includes_preset_browser() {
        let html = build_faceplate_html();
        assert!(
            html.contains("__vxn.panels.presetBrowser"),
            "presetBrowser panel missing from bundle",
        );
        // Shared module spliced (the glue calls the factory it defines) and
        // its ESM markers stripped so the inline <script> stays valid.
        assert!(
            html.contains("function createPresetBrowser"),
            "shared preset-browser module not spliced into the bundle",
        );
        assert!(
            html.contains("createPresetBrowser({"),
            "glue does not instantiate the shared browser",
        );
        assert!(
            !html.contains("export function createPresetBrowser"),
            "ESM export marker leaked into the inline script",
        );
        // Floating two-pane markup (VXN1 ids).
        for id in [
            "id=\"browser-panel\"",
            "id=\"browser-backdrop\"",
            "id=\"browser-folders\"",
            "id=\"browser-presets\"",
            "id=\"browser-search-input\"",
            "id=\"browser-search-clear\"",
            "id=\"browser-close\"",
        ] {
            assert!(html.contains(id), "browser markup missing: {id}");
        }
        // main.js installs the real corpus handler + load-highlight +
        // follow-path routes.
        assert!(
            html.contains("vxn.applyPresetCorpus = function"),
            "main.js does not install applyPresetCorpus handler",
        );
        assert!(
            html.contains("presetBrowser.setCurrentSource"),
            "main.js does not route preset_loaded source to the browser",
        );
        assert!(
            html.contains("presetBrowser.followPath"),
            "main.js does not route preset_corpus_changed follow to the browser",
        );
        // The browser dispatches the full opcode set the shared backend parses.
        for op in [
            "load_factory", "load_user", "rename_preset", "delete_preset",
            "move_preset", "rename_folder", "delete_folder", "new_folder",
            "save_preset",
        ] {
            assert!(html.contains(op), "browser missing {op} dispatch");
        }
        // Two-pane / DnD / context-menu CSS ported.
        assert!(html.contains(".browser-panes"), "two-pane CSS missing");
        assert!(html.contains(".browser-submenu"), "move-to submenu CSS missing");
        assert!(html.contains(".browser-row.dragging"), "drag-and-drop CSS missing");
    }
}
