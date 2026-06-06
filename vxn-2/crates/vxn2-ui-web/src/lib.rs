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
use vxn2_app::{Layer, MatrixRow, UiEvent, Vxn2UiCustom, Vxn2ViewCustom};
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

/// Splice CSS + bootstrap JS into the HTML template. 0025 will replace
/// the placeholder body markup; 0026 layers the panels JS in.
fn build_faceplate_html() -> String {
    HTML_TEMPLATE
        .replace("__CSS__", FACEPLATE_CSS)
        .replace("__BOOTSTRAP_JS__", BOOTSTRAP_JS)
}

/// Parse a VXN2-specific opcode that the shared vocabulary doesn't carry.
/// `v` is the already-parsed IPC JSON payload. Returns `None` on unknown
/// opcodes — `vxn_core_ui_web::parse_ui_event` falls through and drops.
fn parse_custom_ui(op: &str, v: &JsonValue) -> Option<UiEvent> {
    match op {
        "set_edit_layer" => {
            let layer = layer_from_json(v.get("layer")?)?;
            Some(UiEvent::Custom(Box::new(Vxn2UiCustom::SetEditLayer {
                layer,
            })))
        }
        "set_op_tab" => {
            let layer = layer_from_json(v.get("layer")?)?;
            let op = v.get("op")?.as_u64()? as u8;
            Some(UiEvent::Custom(Box::new(Vxn2UiCustom::SetOpTab {
                layer,
                op,
            })))
        }
        "set_matrix_row" => {
            let layer = layer_from_json(v.get("layer")?)?;
            let slot = v.get("slot")?.as_u64()? as u8;
            let row = matrix_row_from_json(v.get("row")?)?;
            Some(UiEvent::Custom(Box::new(Vxn2UiCustom::SetMatrixRow {
                layer,
                slot,
                row,
            })))
        }
        _ => None,
    }
}

/// Serialise a [`Vxn2ViewCustom`] payload to the JSON shape the page
/// dispatcher expects. `payload` is the raw `Box<dyn Any>` content from
/// `ViewEvent::Custom`.
fn serialise_custom_view(payload: &dyn std::any::Any) -> Option<JsonValue> {
    let custom = payload.downcast_ref::<Vxn2ViewCustom>()?;
    Some(match custom {
        Vxn2ViewCustom::EditLayerChanged { layer } => serde_json::json!({
            "kind": "edit_layer_changed",
            "layer": layer_to_str(*layer),
        }),
        Vxn2ViewCustom::OpTabChanged { layer, op } => serde_json::json!({
            "kind": "op_tab_changed",
            "layer": layer_to_str(*layer),
            "op": op,
        }),
        Vxn2ViewCustom::MatrixRowChanged { layer, slot, row } => serde_json::json!({
            "kind": "matrix_row_changed",
            "layer": layer_to_str(*layer),
            "slot": slot,
            "row": matrix_row_to_json(*row),
        }),
    })
}

fn layer_from_json(v: &JsonValue) -> Option<Layer> {
    match v.as_str()? {
        "upper" | "Upper" => Some(Layer::Upper),
        "lower" | "Lower" => Some(Layer::Lower),
        _ => None,
    }
}

fn layer_to_str(layer: Layer) -> &'static str {
    match layer {
        Layer::Upper => "upper",
        Layer::Lower => "lower",
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_custom_set_edit_layer() {
        let v = serde_json::json!({ "op": "set_edit_layer", "layer": "lower" });
        let ev = parse_custom_ui("set_edit_layer", &v).expect("parsed");
        match ev {
            UiEvent::Custom(p) => match p.downcast::<Vxn2UiCustom>().map(|b| *b) {
                Ok(Vxn2UiCustom::SetEditLayer { layer: Layer::Lower }) => {}
                other => panic!("unexpected payload: {other:?}"),
            },
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn parse_custom_set_op_tab() {
        let v = serde_json::json!({ "op": "set_op_tab", "layer": "upper", "op": 3 });
        let ev = parse_custom_ui("set_op_tab", &v).expect("parsed");
        match ev {
            UiEvent::Custom(p) => match p.downcast::<Vxn2UiCustom>().map(|b| *b) {
                Ok(Vxn2UiCustom::SetOpTab {
                    layer: Layer::Upper,
                    op: 3,
                }) => {}
                other => panic!("unexpected payload: {other:?}"),
            },
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn parse_custom_set_matrix_row() {
        let v = serde_json::json!({
            "op": "set_matrix_row",
            "layer": "lower",
            "slot": 7,
            "row": {
                "source": 2, "dest": 17, "curve": 1, "active": true, "depth": 0.5
            }
        });
        let ev = parse_custom_ui("set_matrix_row", &v).expect("parsed");
        match ev {
            UiEvent::Custom(p) => match p.downcast::<Vxn2UiCustom>().map(|b| *b) {
                Ok(Vxn2UiCustom::SetMatrixRow {
                    layer: Layer::Lower,
                    slot: 7,
                    row,
                }) => {
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
            layer: Layer::Upper,
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
        assert_eq!(v["layer"], "upper");
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
        // Spot-check: every entry has machine id (`name`), display label,
        // kind, and a CLAP id matching its index.
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
        // No unsubstituted placeholders left.
        assert!(!html.contains("__CSS__"));
        assert!(!html.contains("__BOOTSTRAP_JS__"));
    }

    /// `__vxn.params` field used by 0026's panel renderers is not yet
    /// populated by the bootstrap — but the surface exists. Sanity-check
    /// the bootstrap shape so a future edit doesn't drop the field.
    #[test]
    fn bootstrap_js_declares_required_surface() {
        assert!(BOOTSTRAP_JS.contains("window.__vxn"));
        assert!(BOOTSTRAP_JS.contains("applyViewEvents"));
        assert!(BOOTSTRAP_JS.contains("applyPresetCorpus"));
        assert!(BOOTSTRAP_JS.contains("\"ready\""));
    }
}
