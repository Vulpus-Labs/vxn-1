//! VXN2 HTML faceplate backend.
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

pub use vxn_core_ui_web::{EditorHandle, OpenEditorError, prompt_text};

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
const PANEL_DIAL_JS: &str = include_str!("../assets/panels/dial.js");
const PANEL_FADER_JS: &str = include_str!("../assets/panels/fader.js");
const PANEL_BUTTON_GROUP_JS: &str = include_str!("../assets/panels/button-group.js");
const PANEL_GRAPH_JS: &str = include_str!("../assets/panels/graph.js");
const PANEL_ALGO_DIAGRAM_JS: &str = include_str!("../assets/panels/algo-diagram.js");
// op-row sub-modules; spliced before it.
const PANEL_ALGO_DATA_JS: &str = include_str!("../assets/panels/algo-data.js");
const PANEL_KS_GRAPH_JS: &str = include_str!("../assets/panels/ks-graph.js");
const PANEL_EG_GRAPH_JS: &str = include_str!("../assets/panels/eg-graph.js");
const PANEL_OP_FADERS_JS: &str = include_str!("../assets/panels/op-faders.js");
const PANEL_OP_ROW_JS: &str = include_str!("../assets/panels/op-row.js");
const PANEL_MOD_MATRIX_JS: &str = include_str!("../assets/panels/mod-matrix.js");
const PANEL_PRESET_BAR_JS: &str = include_str!("../assets/panels/preset-bar.js");
const PANEL_PRESET_BROWSER_JS: &str = include_str!("../assets/panels/preset-browser.js");
const PANEL_FX_TABS_JS: &str = include_str!("../assets/panels/fx-tabs.js");
const MAIN_JS: &str = include_str!("../assets/main.js");

/// Open the VXN2 editor under `parent`. Wraps
/// [`vxn_core_ui_web::open_editor`] with VXN2's HTML and custom-event
/// hooks.
///
/// `parent` is the same raw pointer the clack shell extracts from the
/// host's `gui::set_parent` (NSView / HWND / xcb window id).
///
/// Errors (never panics) on a null parent handle or a wry build failure;
/// the clack shell maps it to `PluginError` in `set_parent`.
pub fn open_editor(
    parent: *mut c_void,
    ctrl: ControllerHandle,
    corpus: CorpusHandle,
) -> Result<EditorHandle, OpenEditorError> {
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
    let dev = dev_assets_dir();
    let js_bundle = faceplate_js_bundle(dev.as_deref());
    let html_tpl = asset(dev.as_deref(), "index.html", HTML_TEMPLATE);
    let css = format!(
        "{}\n{}\n{}",
        asset(dev.as_deref(), "style.css", FACEPLATE_CSS),
        vxn_core_ui_web::PRESET_BROWSER_CSS,
        vxn_core_ui_web::VALUE_POP_CSS,
    );
    html_tpl
        .replace("__CSS__", &css)
        .replace("__BOOTSTRAP_JS__", &js_bundle)
}

/// The concatenated classic-`<script>` JS bundle (shared widgets + bootstrap
/// with the descriptor JSON spliced in + every panel + main). Shared by the
/// native [`build_faceplate_html`] and the web [`build_web_faceplate_html`] so
/// the two pages run byte-identical faceplate logic — only the transport under
/// it differs.
fn faceplate_js_bundle(dev: Option<&std::path::Path>) -> String {
    let params_json = build_params_json();
    let matrix_lists_json = build_matrix_lists_json();
    let default_patch_json = build_default_patch_json();
    let bootstrap = asset(dev, "bootstrap.js", BOOTSTRAP_JS)
        .replace("__PARAMS_JSON__", &params_json)
        .replace("__MATRIX_LISTS_JSON__", &matrix_lists_json)
        .replace("__DEFAULT_PATCH_JSON__", &default_patch_json)
        .replace("__SUBDIVISIONS_JSON__", &build_subdivisions_json());
    [
        // Shared widget primitives: valuePop / wireDrag / cutoff-tuned math.
        // Spliced FIRST so their stripped top-level bindings precede
        // bootstrap.js and every panel that references them (`const` bindings
        // don't hoist).
        vxn_core_ui_web::shared_widgets_js(),
        bootstrap,
        asset(dev, "panels/knob.js", PANEL_KNOB_JS),
        // dial.js depends on fader.js's taper/format helpers, so it's spliced
        // immediately after fader.js below.
        asset(dev, "panels/fader.js", PANEL_FADER_JS),
        asset(dev, "panels/dial.js", PANEL_DIAL_JS),
        asset(dev, "panels/button-group.js", PANEL_BUTTON_GROUP_JS),
        asset(dev, "panels/graph.js", PANEL_GRAPH_JS),
        asset(dev, "panels/algo-diagram.js", PANEL_ALGO_DIAGRAM_JS),
        // op-row's data tables + sub-widgets, spliced before the coordinator.
        // algo-data must precede op-row (referenced in bind); ks-graph /
        // eg-graph are referenced at render time but kept here for clarity.
        asset(dev, "panels/algo-data.js", PANEL_ALGO_DATA_JS),
        asset(dev, "panels/ks-graph.js", PANEL_KS_GRAPH_JS),
        asset(dev, "panels/eg-graph.js", PANEL_EG_GRAPH_JS),
        asset(dev, "panels/op-faders.js", PANEL_OP_FADERS_JS),
        asset(dev, "panels/op-row.js", PANEL_OP_ROW_JS),
        asset(dev, "panels/mod-matrix.js", PANEL_MOD_MATRIX_JS),
        asset(dev, "panels/preset-bar.js", PANEL_PRESET_BAR_JS),
        // Shared two-pane preset browser (vxn-core-ui-web), ESM markers
        // stripped, spliced immediately before its VXN2 glue (which calls
        // the `createPresetBrowser` it defines).
        vxn_core_ui_web::strip_esm_exports(vxn_core_ui_web::PRESET_BROWSER_JS),
        asset(dev, "panels/preset-browser.js", PANEL_PRESET_BROWSER_JS),
        // FX tab strip: attaches `window.__vxn.wireFxTabs`, which main.js calls
        // in `boot`.
        asset(dev, "panels/fx-tabs.js", PANEL_FX_TABS_JS),
        asset(dev, "main.js", MAIN_JS),
    ]
    .join("\n;\n")
}

/// Build the STANDALONE web faceplate page. Same markup / CSS /
/// faceplate JS + spliced descriptor JSON as the native page, but:
///
/// - a tiny classic pre-script installs a `window.ipc` that *queues* posted
///   opcodes, so no intent the faceplate dispatches during boot (notably
///   `ready`) is lost before the ES-module bridge takes over; and
/// - a trailing `<script type="module">` boots `faceplate-bridge.mjs`, which
///   replaces `window.ipc` with the real SAB/controller transport and drains the
///   boot queue.
///
/// The transport ES modules (`faceplate-bridge.mjs` + its imports) and the two
/// `.wasm` files are served alongside this page by the xtask bundler; the
/// page references them by relative URL.
pub fn build_web_faceplate_html() -> String {
    // No dev-asset hot-reload for the generated page — always the embedded bundle.
    let bundle = faceplate_js_bundle(None);
    // The boot-queue stub MUST run before the faceplate bundle so `window.ipc`
    // exists when main.js dispatches. Prepended into the same classic script.
    let ipc_stub = "window.ipc={postMessage:function(m){\
(window.__vxnBootQueue=window.__vxnBootQueue||[]).push(m);}};";
    let classic = format!("{ipc_stub}\n;\n{bundle}");
    let css = format!(
        "{}\n{}\n{}",
        FACEPLATE_CSS,
        vxn_core_ui_web::PRESET_BROWSER_CSS,
        vxn_core_ui_web::VALUE_POP_CSS,
    );
    // The module boot; appended before </body> so the classic faceplate has
    // already parsed + set window.__vxn.applyViewEvents.
    let module_boot = "\n<script type=\"module\">\n\
import { bootFaceplate } from \"./faceplate-bridge.mjs\";\n\
bootFaceplate().catch(function (e) { console.error(\"vxn2 boot failed\", e); });\n\
</script>\n";
    let html = HTML_TEMPLATE
        .replace("__CSS__", &css)
        .replace("__BOOTSTRAP_JS__", &classic);
    if let Some(idx) = html.rfind("</body>") {
        format!("{}{}{}", &html[..idx], module_boot, &html[idx..])
    } else {
        format!("{html}{module_boot}")
    }
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
/// so an in-process test can drive the same IPC parse
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
        "set_ks_curve" => {
            let op = v.get("op")?.as_u64()? as u8;
            let side = v.get("side")?.as_u64()? as u8;
            let curve = v.get("curve")?.as_u64()? as u8;
            Some(UiEvent::Custom(Box::new(Vxn2UiCustom::SetKsCurve {
                op,
                side,
                curve,
            })))
        }
        "request_ks_curve_snapshot" => Some(UiEvent::Custom(Box::new(
            Vxn2UiCustom::RequestKsCurveSnapshot,
        ))),
        "set_eg_curve" => {
            let op = v.get("op")?.as_u64()? as u8;
            let curve = v.get("curve")?.as_u64()? as u8;
            Some(UiEvent::Custom(Box::new(Vxn2UiCustom::SetEgCurve {
                op,
                curve,
            })))
        }
        "request_eg_curve_snapshot" => Some(UiEvent::Custom(Box::new(
            Vxn2UiCustom::RequestEgCurveSnapshot,
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
        Vxn2ViewCustom::MatrixSnapshot { rows } => serde_json::json!({
            "kind": "matrix_snapshot",
            "rows": rows.iter().map(|r| matrix_row_to_json(*r)).collect::<Vec<_>>(),
        }),
        Vxn2ViewCustom::KsCurveSnapshot { curves } => serde_json::json!({
            "kind": "ks_curve_snapshot",
            // [[l, r]; 6] — outer index is op (0-based), inner is side.
            "curves": curves.iter().map(|s| s.to_vec()).collect::<Vec<_>>(),
        }),
        Vxn2ViewCustom::EgCurveSnapshot { curves } => serde_json::json!({
            "kind": "eg_curve_snapshot",
            // [u8; 6] — index is op (0-based); 0 = Exp, 1 = Lin.
            "curves": curves.to_vec(),
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
        // E033 scale source; absent (older page / unscaled) → 0 = None.
        scale_src: v.get("scale").and_then(|s| s.as_u64()).unwrap_or(0) as u8,
    })
}

fn matrix_row_to_json(row: MatrixRow) -> JsonValue {
    serde_json::json!({
        "source": row.source,
        "dest": row.dest,
        "curve": row.curve,
        "active": row.active,
        "depth": row.depth,
        "scale": row.scale_src,
    })
}

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
        coherence, DestId, SourceId, CURVE_LABELS, CURVE_NAMES, DEST_LABELS, DEST_NAMES,
        SOURCE_LABELS, SOURCE_NAMES,
    };
    // `id` is the wire discriminant; `tier` is the granularity tier:
    // 0 = patch-global, 1 = per-stack, 2 = per-lane. The UI reads `tier` and
    // the `coherence` table below rather than re-deriving the rule.
    let sources: Vec<JsonValue> = SOURCE_NAMES
        .iter()
        .zip(SOURCE_LABELS.iter())
        .enumerate()
        .map(|(i, (n, l))| {
            let tier = SourceId::from_u8(i as u8).tier() as u8;
            serde_json::json!({ "id": i, "name": n, "label": l, "tier": tier })
        })
        .collect();
    let dests: Vec<JsonValue> = DEST_NAMES
        .iter()
        .zip(DEST_LABELS.iter())
        .enumerate()
        .map(|(i, (n, l))| {
            let tier = DestId::from_u8(i as u8).tier() as u8;
            serde_json::json!({ "id": i, "name": n, "label": l, "tier": tier })
        })
        .collect();
    let curves: Vec<JsonValue> = CURVE_NAMES
        .iter()
        .zip(CURVE_LABELS.iter())
        .enumerate()
        .map(|(i, (n, l))| serde_json::json!({ "id": i, "name": n, "label": l }))
        .collect();
    // Flat `coherence[srcId][dstId]` verdict table — the canonical engine
    // predicate baked in so the validator never drifts from the rule.
    // Values are the machine-name strings ("ok", "tier-collapse", …).
    let coherence_table: Vec<Vec<&str>> = (0..SOURCE_NAMES.len())
        .map(|si| {
            let src = SourceId::from_u8(si as u8);
            (0..DEST_NAMES.len())
                .map(|di| coherence(src, DestId::from_u8(di as u8)).name())
                .collect()
        })
        .collect();
    serde_json::json!({
        "sources": sources,
        "dests": dests,
        "curves": curves,
        "coherence": coherence_table,
    })
    .to_string()
}

/// One JSON entry per CLAP id (shape from `descriptor_to_json`): `name`,
/// `label`, `min`, `max`, `default`, `kind`, and either `unit` or `variants`.
/// The page hydrates it into `__vxn.params` on first batch.
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
        // The opcode is the first arg ("set_op_tab"); inside the payload `"op"`
        // is the operator-tab index.
        let v = serde_json::json!({ "op": 3 });
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
        // Spot-check: verify "algo" descriptor field values match the engine's
        // descriptor exactly (catches mismatches between build_params_json
        // serialisation and the actual param table).
        let algo_id = vxn2_engine::params::id_of("algo").expect("algo param not found");
        let algo_desc = vxn2_engine::params::core_desc_for_clap_id(algo_id)
            .expect("algo descriptor missing");
        let entry = &arr[algo_id];
        assert_eq!(
            entry["name"].as_str().unwrap(),
            algo_desc.name,
            "algo name mismatch in params JSON"
        );
        assert_eq!(
            entry["label"].as_str().unwrap(),
            algo_desc.label,
            "algo label mismatch in params JSON"
        );
        assert_eq!(
            entry["min"].as_f64().unwrap() as f32,
            algo_desc.min,
            "algo min mismatch in params JSON"
        );
        assert_eq!(
            entry["max"].as_f64().unwrap() as f32,
            algo_desc.max,
            "algo max mismatch in params JSON"
        );
        assert_eq!(
            entry["default"].as_f64().unwrap() as f32,
            algo_desc.default,
            "algo default mismatch in params JSON"
        );
    }

    /// Shell carries the static faceplate markup: no inline handlers, no
    /// placeholder values inlined into style attributes, all sections + a
    /// representative cross-section of params reachable via `querySelector`.
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
            "voice", "stack", "filter", "fx", "master",
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
            // Phaser: every param reachable in its pane.
            "phaser-on", "phaser-rate", "phaser-depth", "phaser-feedback",
            "phaser-mix",
            "master-tune", "master-volume",
            // Filter section: every param reachable.
            "filter-enable", "filter-cutoff", "filter-resonance",
            "filter-mode", "filter-slope", "filter-drive",
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
        // Overlays start hidden — toggled via the [hidden] attribute. Tolerate
        // intervening attrs (data-vxn-role etc.).
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

    // Asset-present guards — one per embedded asset.
    // These confirm the asset didn't vanish and that no __PLACEHOLDER__ tokens
    // remain in the rendered page. The real behavioural net for JS wiring is
    // the Vitest suite under `assets/__tests__/`; do not regrow per-token
    // substring assertions here (they prove nothing about live behaviour).
    #[test]
    fn asset_present_css() {
        assert!(!FACEPLATE_CSS.is_empty(), "faceplate CSS asset is empty");
        // Viewport dims are load-bearing structure (not wiring); one guard is enough.
        assert!(FACEPLATE_CSS.contains("--editor-w: 1024px"), "viewport width CSS var missing");
        assert!(FACEPLATE_CSS.contains("--editor-h: 772px"), "viewport height CSS var missing");
    }

    #[test]
    fn asset_present_bootstrap_js() {
        assert!(!BOOTSTRAP_JS.is_empty(), "bootstrap JS asset is empty");
        // The __PARAMS_JSON__ placeholder must exist in the source so the splice
        // has something to replace; the rendered page must not contain it.
        assert!(
            BOOTSTRAP_JS.contains("__PARAMS_JSON__"),
            "bootstrap.js missing __PARAMS_JSON__ placeholder",
        );
        let html = build_faceplate_html();
        assert!(
            !html.contains("__PARAMS_JSON__"),
            "__PARAMS_JSON__ placeholder not spliced in rendered page",
        );
    }

    #[test]
    fn asset_present_panel_js() {
        // Each panel file is non-empty — catches an accidental `include_str!`
        // path typo that would embed an empty string silently.
        for (name, src) in [
            ("knob.js",          PANEL_KNOB_JS),
            ("dial.js",          PANEL_DIAL_JS),
            ("fader.js",         PANEL_FADER_JS),
            ("button-group.js",  PANEL_BUTTON_GROUP_JS),
            ("graph.js",         PANEL_GRAPH_JS),
            ("algo-diagram.js",  PANEL_ALGO_DIAGRAM_JS),
            ("algo-data.js",     PANEL_ALGO_DATA_JS),
            ("ks-graph.js",      PANEL_KS_GRAPH_JS),
            ("eg-graph.js",      PANEL_EG_GRAPH_JS),
            ("op-faders.js",     PANEL_OP_FADERS_JS),
            ("op-row.js",        PANEL_OP_ROW_JS),
            ("mod-matrix.js",    PANEL_MOD_MATRIX_JS),
            ("preset-bar.js",    PANEL_PRESET_BAR_JS),
            ("preset-browser.js", PANEL_PRESET_BROWSER_JS),
            ("fx-tabs.js",       PANEL_FX_TABS_JS),
            ("main.js",          MAIN_JS),
        ] {
            assert!(!src.is_empty(), "panel asset {name} is empty");
        }
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

    /// Bundle composition: CSS + params JSON spliced in, all panel files
    /// concatenated, no placeholder tokens left in the served HTML.
    #[test]
    fn build_faceplate_html_bundles_full_js_stack() {
        let html = build_faceplate_html();
        // No unreplaced placeholder tokens.
        assert!(!html.contains("__CSS__"), "CSS placeholder not spliced");
        assert!(!html.contains("__PARAMS_JSON__"), "params JSON placeholder not spliced");
        assert!(!html.contains("__BOOTSTRAP_JS__"), "bootstrap JS placeholder not spliced");
        assert!(!html.contains("__MATRIX_LISTS_JSON__"), "matrix lists placeholder not spliced");
        // Structural canary.
        assert!(html.contains("VXN2"), "VXN2 identifier missing from rendered HTML");
        assert!(html.contains("color-scheme: dark"), "color-scheme: dark missing from CSS");
        assert!(html.contains("window.__vxn"), "window.__vxn surface missing");
        // All panel panel modules present in the bundle (non-empty asset guard
        // is in asset_present_panel_js; here we confirm splice into the page).
        assert!(html.contains("__vxn.panels.knob"));
        assert!(html.contains("__vxn.panels.fader"));
        assert!(html.contains("__vxn.panels.buttonGroup"));
        assert!(html.contains("__vxn.panels.graph"));
        assert!(html.contains("__vxn.panels.algoDiagram"));
        assert!(html.contains("__vxn.panels.algoData"));
        assert!(html.contains("__vxn.panels.ksGraph"));
        assert!(html.contains("__vxn.panels.egGraph"));
        assert!(html.contains("__vxn.panels.opRow"));
        assert!(html.contains("dispatch(\"ready\")"));
        // Params JSON spliced — first descriptor's machine id is `op1-num`
        // post-flatten. Cheap structural canary.
        assert!(html.contains("\"op1-num\""));
        assert!(html.contains("\"lfo1-rate\""));
        assert!(html.contains("\"master-volume\""));
    }

    /// Extract the body of a top-level JS `const DECL_NAME = [ ... ];`
    /// declaration from `js`. Returns the slice between `[` and `]`.
    /// Panics with a clear message if either marker is absent — the algo
    /// drift guards rely on stable formatting, so a silent miss would
    /// make those tests vacuous instead of failing loudly.
    fn extract_js_array_body<'a>(js: &'a str, decl_name: &str) -> &'a str {
        let needle = format!("const {decl_name} = [");
        let start = js.find(needle.as_str())
            .unwrap_or_else(|| panic!("{decl_name} declaration not found in JS"));
        let body_start = start + needle.len();
        let body_rest = &js[body_start..];
        let body_end = body_rest.find("];")
            .unwrap_or_else(|| panic!("closing ]; for {decl_name} not found in JS"));
        &body_rest[..body_end]
    }

    /// Parse `matrix_lists_json` once into a `serde_json::Value`; shared by
    /// the two matrix-lists tests so the JSON string is only parsed once.
    fn matrix_lists_value() -> serde_json::Value {
        serde_json::from_str(&build_matrix_lists_json())
            .expect("build_matrix_lists_json must produce valid JSON")
    }

    /// The 32 x 6 JS carrier table embedded in `panels/algo-data.js`
    /// MUST match the engine's algorithm routing table — drift would
    /// silently colour the op tabs wrong. Parses the JS literal back
    /// out (assumes a stable formatting; the parser is intentionally
    /// strict so editing the table without keeping the comment markers
    /// breaks the test loudly).
    #[test]
    fn algo_data_carriers_match_engine_table() {
        let js = PANEL_ALGO_DATA_JS;
        // Extract the ALGO_CARRIERS literal — between
        // `const ALGO_CARRIERS = [` and the matching `];`.
        let body = extract_js_array_body(js, "ALGO_CARRIERS");

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

    /// Parity check for `ALGO_FB_OPS` in `algo-data.js`. Mirrors
    /// `AlgoSpec::fb_src` (the feedback source op, where `fb_scale` lives)
    /// across the 32 algorithms — drift would silently disable the wrong op's
    /// feedback fader.
    #[test]
    fn algo_data_fb_ops_match_engine_table() {
        let body = extract_js_array_body(PANEL_ALGO_DATA_JS, "ALGO_FB_OPS");
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
                vxn2_dsp::algo::ALGOS[i].fb_src,
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
    fn parse_custom_set_ks_curve() {
        let v = serde_json::json!({ "op": 2, "side": 1, "curve": 3 });
        let ev = parse_custom_ui("set_ks_curve", &v).expect("parsed");
        match ev {
            UiEvent::Custom(p) => match p.downcast::<Vxn2UiCustom>().map(|b| *b) {
                Ok(Vxn2UiCustom::SetKsCurve { op, side, curve }) => {
                    assert_eq!((op, side, curve), (2, 1, 3));
                }
                other => panic!("unexpected payload: {other:?}"),
            },
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn parse_custom_request_ks_curve_snapshot() {
        let v = serde_json::json!({ "op": "request_ks_curve_snapshot" });
        let ev = parse_custom_ui("request_ks_curve_snapshot", &v).expect("parsed");
        match ev {
            UiEvent::Custom(p) => match p.downcast::<Vxn2UiCustom>().map(|b| *b) {
                Ok(Vxn2UiCustom::RequestKsCurveSnapshot) => {}
                other => panic!("unexpected payload: {other:?}"),
            },
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn serialise_view_ks_curve_snapshot_shape() {
        let curves = [[0u8, 2u8], [1, 3], [0, 2], [0, 2], [0, 2], [3, 0]];
        let payload = Vxn2ViewCustom::KsCurveSnapshot { curves };
        let v = serialise_custom_view(&payload).expect("serialised");
        assert_eq!(v["kind"], "ks_curve_snapshot");
        let rows = v["curves"].as_array().expect("curves array");
        assert_eq!(rows.len(), 6);
        assert_eq!(rows[1][0], 1);
        assert_eq!(rows[1][1], 3);
        assert_eq!(rows[5][0], 3);
    }

    #[test]
    fn parse_custom_set_eg_curve() {
        let v = serde_json::json!({ "op": 3, "curve": 1 });
        let ev = parse_custom_ui("set_eg_curve", &v).expect("parsed");
        match ev {
            UiEvent::Custom(p) => match p.downcast::<Vxn2UiCustom>().map(|b| *b) {
                Ok(Vxn2UiCustom::SetEgCurve { op, curve }) => {
                    assert_eq!((op, curve), (3, 1));
                }
                other => panic!("unexpected payload: {other:?}"),
            },
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn parse_custom_request_eg_curve_snapshot() {
        let v = serde_json::json!({ "op": "request_eg_curve_snapshot" });
        let ev = parse_custom_ui("request_eg_curve_snapshot", &v).expect("parsed");
        match ev {
            UiEvent::Custom(p) => match p.downcast::<Vxn2UiCustom>().map(|b| *b) {
                Ok(Vxn2UiCustom::RequestEgCurveSnapshot) => {}
                other => panic!("unexpected payload: {other:?}"),
            },
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn serialise_view_eg_curve_snapshot_shape() {
        let curves = [0u8, 1, 0, 1, 0, 0];
        let payload = Vxn2ViewCustom::EgCurveSnapshot { curves };
        let v = serialise_custom_view(&payload).expect("serialised");
        assert_eq!(v["kind"], "eg_curve_snapshot");
        let arr = v["curves"].as_array().expect("curves array");
        assert_eq!(arr.len(), 6);
        assert_eq!(arr[1], 1);
        assert_eq!(arr[3], 1);
        assert_eq!(arr[0], 0);
    }

    #[test]
    fn serialise_view_matrix_snapshot_shape() {
        let row = MatrixRow {
            source: 3,
            dest: 5,
            curve: 2,
            active: true,
            depth: 0.25,
            scale_src: 0,
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
        let v = matrix_lists_value();
        assert_eq!(v["sources"].as_array().unwrap().len(), 12);
        // Expected dest count = 52 (None + 51 routable dests).
        assert_eq!(v["dests"].as_array().unwrap().len(), 52);
        assert_eq!(v["curves"].as_array().unwrap().len(), 4);
        assert_eq!(v["sources"][0]["name"], "none");
        assert_eq!(v["sources"][1]["name"], "lfo1");
        assert_eq!(v["dests"][26]["name"], "reverb-mix");
        assert_eq!(v["dests"][27]["name"], "feedback");
        assert_eq!(v["dests"][28]["name"], "cutoff");
        assert_eq!(v["dests"][29]["name"], "resonance");
        assert_eq!(v["dests"][30]["name"], "op1-stack-pitch");
        assert_eq!(v["dests"][35]["name"], "op6-stack-pitch");
        assert_eq!(v["dests"][36]["name"], "op1-phase");
        assert_eq!(v["dests"][41]["name"], "op6-phase");
        assert_eq!(v["dests"][42]["name"], "filter-drive");
        assert_eq!(v["dests"][43]["name"], "global-eg-rate");
        assert_eq!(v["dests"][44]["name"], "op1-eg-rate");
        assert_eq!(v["dests"][49]["name"], "op6-eg-rate");
        assert_eq!(v["dests"][50]["name"], "pitch-eg-rate");
        assert_eq!(v["dests"][51]["name"], "mod-env-rate");
        assert_eq!(v["curves"][3]["name"], "bipolar");
    }

    #[test]
    fn build_matrix_lists_json_carries_tiers_and_coherence() {
        let v = matrix_lists_value();
        // Tiers: lfo1 = patch-global (0), velocity = per-stack (1),
        // voice-rand = per-lane (2); lfo2-phase dest = per-lane (2).
        assert_eq!(v["sources"][1]["name"], "lfo1");
        assert_eq!(v["sources"][1]["tier"], 0);
        assert_eq!(v["sources"][7]["name"], "velocity");
        assert_eq!(v["sources"][7]["tier"], 1);
        assert_eq!(v["sources"][11]["name"], "voice-rand");
        assert_eq!(v["sources"][11]["tier"], 2);
        assert_eq!(v["dests"][22]["name"], "lfo2-phase");
        assert_eq!(v["dests"][22]["tier"], 2);
        // Coherence table: read the exported verdict, not a re-derivation.
        // voice-rand(11) → lfo2-rate(21) collapses; → lfo2-phase(22) is ok.
        let coh = &v["coherence"];
        assert_eq!(coh[11][21], "tier-collapse");
        assert_eq!(coh[11][22], "ok");
        // lfo2(2) → lfo2-rate(21) is self-rate; voice-idx(9) → cutoff(28)
        // is degenerate (a verdict only the engine table knows).
        assert_eq!(coh[2][21], "self-rate");
        assert_eq!(coh[9][28], "degenerate");
        // empty slots never flag.
        assert_eq!(coh[0][5], "ok");
    }

    // Contract guard: the mod-matrix panel consumes the engine's coherence
    // verdict table across the Rust↔JS boundary. The reason tokens are emitted
    // verbatim by `matrix::Coherence::reason()` (vxn2-engine/src/matrix.rs);
    // renaming a verdict there silently kills the JS tooltip — so these guard a
    // real cross-language contract, not JS internals. Live panel behaviour (row
    // repaint, edit-time validation) is JS wiring and is not asserted here.
    #[test]
    fn mod_matrix_panel_wires_coherence_validation() {
        // Panel reads the exported table (`window.__vxn.matrix.coherence`)
        // rather than re-deriving verdicts.
        assert!(
            PANEL_MOD_MATRIX_JS.contains("matrix.coherence"),
            "panel must consume the exported coherence table"
        );
        // Reason strings are the wire contract with `Coherence::reason()` —
        // keep in lockstep with that enum.
        for reason in ["self-rate", "tier-collapse", "degenerate"] {
            assert!(
                PANEL_MOD_MATRIX_JS.contains(reason),
                "missing reason mapping for {reason} (see Coherence::reason)"
            );
        }
        // Invalid-row feedback is a JS→CSS class contract: the panel toggles
        // `vxn-mm-invalid`, the stylesheet must style it or the flag is dead.
        assert!(
            PANEL_MOD_MATRIX_JS.contains("vxn-mm-invalid"),
            "panel must toggle the invalid-row class"
        );
        assert!(
            FACEPLATE_CSS.contains(".vxn-mm-invalid"),
            "stylesheet missing invalid-row rule for the toggled class"
        );
    }

    // Contract guard: mod-matrix depth reuses the shared bipolar fader
    // primitive (fader.js `createBipolar`) instead of forking its own control.
    // That cross-module reuse is the contract; the exact markup, center-tick
    // CSS, and dispatch-call formatting are JS internals (a token in a comment
    // would satisfy them) — exercised by the Vitest suite / manual DAW checks,
    // not guarded here.
    #[test]
    fn mod_matrix_depth_is_bipolar_fader() {
        assert!(
            PANEL_FADER_JS.contains("createBipolar"),
            "fader primitive must export the bipolar variant"
        );
        assert!(
            PANEL_MOD_MATRIX_JS.contains("createBipolar"),
            "matrix depth must reuse the shared bipolar fader"
        );
    }

    #[test]
    fn build_faceplate_html_includes_matrix_panel_and_lists() {
        let html = build_faceplate_html();
        assert!(html.contains("__vxn.panels.modMatrix"), "modMatrix panel missing");
        assert!(html.contains("\"reverb-mix\""), "matrix dest list missing");
        assert!(!html.contains("__MATRIX_LISTS_JSON__"));
    }

    /// Preset-bar panel + browse-dialog markup must be in the
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
        // Save As awaits `dispatchTextInput` directly.
        assert!(
            html.contains("dispatchTextInput(\"Save Preset As\""),
            "preset-bar Save As not wired through dispatchTextInput",
        );
        assert!(
            !html.contains("onTextInputResult"),
            "preset-bar still exposes onTextInputResult shim",
        );
        // dispatchTextInput is the page-side Promise primitive; it MUST be
        // present for any text-input round trip.
        assert!(
            html.contains("vxn.dispatchTextInput = dispatchTextInput"),
            "dispatchTextInput not installed on vxn",
        );
    }

    /// Contract guards for the preset-browser bundle: the shared module is
    /// spliced with its ESM export marker stripped (a leaked `export` breaks
    /// the inline `<script>`), main.js wires the corpus/highlight/follow
    /// routes, and the browser dispatches the full opcode set the backend
    /// parses. Internal markup ids and two-pane/DnD CSS are JS/style internals
    /// (not guarded here); live browser behaviour is a manual DAW check.
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
        // Opcode-dispatch contract: the browser must dispatch every op the
        // backend parses. Canonical consumer list is faceplate-bridge.mjs
        // (DEFERRED_OPS + the routeOpcode switch); keep in lockstep with it.
        for op in [
            "load_factory", "load_user", "rename_preset", "delete_preset",
            "move_preset", "rename_folder", "delete_folder", "new_folder",
            "save_preset",
        ] {
            assert!(html.contains(op), "browser missing {op} dispatch");
        }
    }
}
