//! VXN1b web editor backend (thin shim).
//!
//! Forked from `vxn-ui-web` (E038 / 0209). Thin wrapper over
//! [`vxn_core_ui_web`]: [`open_editor`] assembles VXN1b's faceplate HTML
//! (markup + CSS + the bridge/browser/panels/dispatch JS modules + the
//! param-descriptor JSON) and builds a [`vxn_core_ui_web::WebEditorConfig`]
//! carrying that HTML, then calls `vxn_core_ui_web::open_editor`. The WebView
//! lifecycle, the JS↔Rust IPC bridge, the batched `evaluate_script` view-event
//! sink, the corpus-snapshot push, the parent-window adapter, and the native
//! text-input popup all live in the shared crate — this crate touches neither
//! wry nor raw-window-handle directly.
//!
//! What stays here is the faceplate asset splice and the param-descriptor JSON
//! builder. VXN1b is single-patch (no Upper/Lower layer, no key-mode / split),
//! so — unlike VXN1 — this crate carries NO custom opcode-parse / view-serialise
//! hooks: `parse_custom_ui` / `serialise_custom_view` are left `None` in the
//! config. (Matrix-topology custom opcodes land in 0210.) The faceplate is the
//! compact three-row layout (Osc/Mixer/Filter · LFO/Env · Voice/FX/Master); the
//! mod-matrix overlay is a follow-up step (0210).

use std::ffi::c_void;

use vxn1b_engine::matrix::MatrixTable;
use vxn1b_engine::params::{TOTAL_PARAMS, desc_for_clap_id};
use vxn_core_app::{ControllerHandle, CorpusHandle, ParamDesc, ParamKind, Taper, UiEvent};
use vxn_core_ui_web::{DEFAULT_MAX_BATCH_BYTES, WebEditorConfig};

// The WebView lifecycle, IPC bridge, batched view-event sink, corpus
// snapshot push, and native text-input popup all live in the shared
// crate. This crate re-exports its handle + error and supplies a
// `WebEditorConfig` carrying VXN1b's faceplate HTML.
pub use vxn_core_ui_web::{EditorHandle, OpenEditorError, prompt_text};

/// Logical pixel dimensions of the editor (ADR 0001 §7 compact layout). Height
/// tracks the CSS geometry in `faceplate.css`, sized to the **tallest tab pane**
/// — the Layer pane's three rows (0219): 20 pad + 26 banner + 30 preset-bar +
/// 26 tab-strip + 3×8 chrome gaps + (124 + 124 + 164 rows + 2×8 pane gaps) = 554.
/// The FX/Global pane is two rows (mixer + split, then Dynamics/FX/Master since
/// 0220), so it stays shorter than the Layer pane and does not drive the height.
/// Width was widened from
/// the initial 760 for top-row breathing room + the standalone Dynamics panel.
/// Keep in sync with `--editor-w` / the row heights.
pub const EDITOR_WIDTH: u32 = 940;
pub const EDITOR_HEIGHT: u32 = 554;

/// Display label for the virtual root group of the user preset corpus.
/// VXN1b has no per-synth override, so this matches the shared default.
const UNCATEGORISED: &str = "Uncategorised";

/// Open the VXN1b editor under `parent`. Thin wrapper over
/// [`vxn_core_ui_web::open_editor`]: builds VXN1b's faceplate HTML and a
/// [`WebEditorConfig`], then hands off to the shared crate. The WebView
/// lifecycle, IPC bridge, batched view-event sink, corpus-snapshot push, the
/// batch chunking, the unsafe parent-handle plumbing, and the WebView2
/// user-data folder override all live in core — this crate touches neither
/// wry nor raw-window-handle directly.
///
/// `parent` is the same raw pointer the host hands the clack shell in
/// `gui::set_parent` (NSView / HWND / xcb-window-id).
///
/// Errors (never panics) on a null parent handle or a wry build failure; the
/// clack shell maps it to `PluginError` in `set_parent` so no unwind crosses
/// the C ABI and the plugin keeps rendering.
pub fn open_editor(
    parent: *mut c_void,
    ctrl: ControllerHandle,
    corpus: CorpusHandle,
    matrices: &[MatrixTable; 2],
) -> Result<EditorHandle, OpenEditorError> {
    let html = build_faceplate_html(matrices);
    let mut config = WebEditorConfig::new(html, EDITOR_WIDTH, EDITOR_HEIGHT);
    config.uncategorised_label = UNCATEGORISED;
    config.max_batch_bytes = DEFAULT_MAX_BATCH_BYTES;
    // WebView2 user-data folder: `%LOCALAPPDATA%\VulpusLabs\VXN1b\WebView2`
    // (the shared crate joins vendor/product/"WebView2"). Avoids the
    // admin-only `C:\Program Files\<host>\<exe>.WebView2` default.
    config.webview2_vendor = Some("VulpusLabs");
    config.webview2_product = Some("VXN1b");
    // Two-layer key-mode opcodes (0219). The faceplate posts `set_key_mode`
    // (Layer 2 toggle) / `set_split_point` as non-automatable custom ops; parse
    // them into a `KeyOp` payload the clap controller applies to the shared
    // KeyState. (Matrix topology opcodes join this hook with the overlay, 0210.)
    config.parse_custom_ui = Some(std::sync::Arc::new(|op, v| {
        use vxn1b_engine::{KeyOp, Layer, MatrixEdit, MatrixField};
        match op {
            "set_key_mode" => Some(UiEvent::Custom(Box::new(KeyOp::SetKeyMode(
                v.get("mode")?.as_u64()? as u8,
            )))),
            "set_split_point" => Some(UiEvent::Custom(Box::new(KeyOp::SetSplitPoint(
                v.get("note")?.as_u64()? as u8,
            )))),
            // Cross-layer LFO 2 link (0217, ADR 0002 §5) — Layer 2's LFO 2 slaves
            // to Layer 1's. Non-automatable, so it rides KeyState like the key
            // mode; distinct from the `lfo2_sync` (tempo-sync) CLAP param.
            "set_lfo2_link" => {
                Some(UiEvent::Custom(Box::new(KeyOp::SetLfo2Link(v.get("on")?.as_bool()?))))
            }
            // Matrix topology edit (0219/0210): source/dest/curve/scale on one
            // slot of one layer. Depth is a normal CLAP param, not this op.
            "set_matrix" => {
                let layer = if v.get("layer")?.as_str()? == "lower" {
                    Layer::L2
                } else {
                    Layer::L1
                };
                let field = match v.get("field")?.as_str()? {
                    "source" => MatrixField::Source,
                    "dest" => MatrixField::Dest,
                    "curve" => MatrixField::Curve,
                    "scale" => MatrixField::ScaleSrc,
                    _ => return None,
                };
                Some(UiEvent::Custom(Box::new(MatrixEdit {
                    layer,
                    slot: v.get("slot")?.as_u64()? as u8,
                    field,
                    value: v.get("value")?.as_u64()? as u8,
                })))
            }
            _ => None,
        }
    }));
    // Meter frames (0240) are the one view-bound custom payload. They ride the
    // normal per-tick ViewEvent batch — one `evaluate_script`, no separate
    // bridge channel — and carry raw peaks; the dB mapping and ballistics live
    // in `panels/meter.js`.
    //
    // Arrays rather than named l/r keys: the page indexes `[0]`/`[1]`, and the
    // frame ships up to 60×/s, so the terser shape is worth it on the wire.
    config.serialise_custom_view = Some(std::sync::Arc::new(|payload| {
        // Matrix topology echo (0247): the patch changed under an open editor
        // (preset load, host state load, undo). Same slot shape as the
        // open-time `__MATRIX_JSON__` seed, so the page can swap one for the
        // other; depths stay out — they are params and ride `ParamChanged`.
        if let Some(m) = payload.downcast_ref::<vxn1b_engine::MatrixSnapshot>() {
            return Some(serde_json::json!({
                "kind": "matrix",
                "slots": [slots_json(&m.layers[0]), slots_json(&m.layers[1])],
            }));
        }
        // Keyboard echo (0221). Same reason as the topology echo: KeyState is
        // not a CLAP param, so a preset / host-state load moves it with nothing
        // in the param machinery to tell the page. `mode` is the derived 0/1/2
        // the faceplate already speaks (`set_key_mode`), so the echo and the
        // opcode share one encoding.
        if let Some(k) = payload.downcast_ref::<vxn1b_engine::KeyState>() {
            return Some(serde_json::json!({
                "kind": "keys",
                "mode": k.key_mode() as u8,
                "split": k.split_point,
                "link": k.lfo2_link,
            }));
        }
        let f = payload.downcast_ref::<vxn1b_engine::MeterFrame>()?;
        Some(serde_json::json!({
            "kind": "meters",
            "l1": [f.layer1.0, f.layer1.1],
            "l2": [f.layer2.0, f.layer2.1],
            "dynIn": [f.dynamics_in.0, f.dynamics_in.1],
            "dynOut": [f.dynamics_out.0, f.dynamics_out.1],
            // One value, not a pair — the compressor's detector is stereo-linked.
            "dynGr": f.dynamics_gr,
            "master": [f.master.0, f.master.1],
        }))
    }));
    vxn_core_ui_web::open_editor(parent, ctrl, corpus, config)
}

/// Splice the runtime param-descriptor JSON into the faceplate template. The
/// page reads it as `window.vxn.params = {...}`, a CLAP-id-keyed map of
/// `{name, label, kind, min, max, default, taper, unit, variants?}`.
///
/// CSS + the JS modules (bridge / browser / panels / dispatch) live in sibling
/// files spliced in here — the wry WebView serves the page via `with_html`,
/// so external `<link href>` / `<script src>` would need a custom protocol
/// handler to resolve. Inlining keeps the page self-contained without that
/// plumbing.
fn build_faceplate_html(matrices: &[MatrixTable; 2]) -> String {
    // Native plugin page: no web transport, so the `__WEB_BOOT_HEAD__` /
    // `__WEB_BOOT_LOADER__` slots are spliced empty.
    assemble_faceplate("", "", matrices)
}

/// Splice every faceplate placeholder. `web_boot_head` / `web_boot_loader`
/// fill the `__WEB_BOOT_HEAD__` / `__WEB_BOOT_LOADER__` slots that bracket
/// the inlined faceplate `<script>` — empty for the native plugin
/// ([`build_faceplate_html`]), the web transport shim + module loader for the
/// standalone build ([`build_web_faceplate_html`]). They are spliced FIRST,
/// before the `__*_JSON__` replaces, so the boot head's own placeholder tokens
/// pick up the very same descriptor data as the body.
///
/// The browser logic is shared (vxn-core-ui-web); splice it (ESM markers
/// stripped) immediately before VXN1b's `browser.js` glue, which calls the
/// `createPresetBrowser` it defines. Its CSS is appended to the faceplate
/// sheet.
fn assemble_faceplate(
    web_boot_head: &str,
    web_boot_loader: &str,
    matrices: &[MatrixTable; 2],
) -> String {
    let browser_js = format!(
        "{}\n;\n{}",
        strip_esm_exports(vxn_core_ui_web::PRESET_BROWSER_JS),
        strip_esm_exports(BROWSER_JS),
    );
    let css = format!(
        "{}\n{}\n{}",
        FACEPLATE_CSS, vxn_core_ui_web::PRESET_BROWSER_CSS, vxn_core_ui_web::VALUE_POP_CSS,
    );
    // Shared widget primitives (valuePop / wireDrag / cutoff-tuned math).
    // Spliced into the bridge slot (which runs first) so their stripped
    // top-level bindings precede panels.js, which references them.
    let bridge_js = format!(
        "{}\n;\n{}",
        vxn_core_ui_web::shared_widgets_js(),
        strip_esm_exports(BRIDGE_JS),
    );
    PLACEHOLDER_HTML
        .replace("__WEB_BOOT_HEAD__", web_boot_head)
        .replace("__WEB_BOOT_LOADER__", web_boot_loader)
        .replace("__CSS__", &css)
        .replace("__BRIDGE_JS__", &bridge_js)
        .replace("__BROWSER_JS__", &browser_js)
        .replace("__PANELS_JS__", &panels_js())
        .replace("__DISPATCH_JS__", &strip_esm_exports(DISPATCH_JS))
        .replace("__PARAMS_JSON__", &build_params_json())
        .replace("__SUBDIVISIONS_JSON__", &build_subdivisions_json())
        .replace("__MATRIX_JSON__", &build_matrix_json(matrices))
        .replace("__PATCH_COUNT__", &PATCH_COUNT.to_string())
}

/// Serialise the mod-matrix vocab + **live** topology for the overlay (0219).
/// The page reads it as `window.vxn.matrix = { sources, dests, curves, slots }`:
/// each vocab entry is `{value, name, label}` (value = the wire `u8`), and
/// `slots[layer][i]` is `{source, dest, curve, scale}` for slot `i`.
/// Depths are **not** here — they ride `window.vxn.params` as CLAP params.
///
/// `matrices` MUST be the plugin's current per-layer topology, not the factory
/// default: topology is non-automatable, so nothing replays it to a freshly
/// opened page the way the host replays params. Seeding from the factory patch
/// is what made every source/dest combo revert on GUI close/reopen.
fn build_matrix_json(matrices: &[MatrixTable; 2]) -> String {
    use serde_json::{Value, json};
    use vxn1b_engine::matrix::{
        CURVE_LABELS, CURVE_NAMES, DEST_LABELS, DEST_NAMES, SOURCE_LABELS, SOURCE_NAMES,
    };
    let vocab = |names: &[&str], labels: &[&str]| -> Vec<Value> {
        names
            .iter()
            .zip(labels)
            .enumerate()
            .map(|(i, (n, l))| json!({ "value": i, "name": n, "label": l }))
            .collect()
    };
    json!({
        "sources": vocab(&SOURCE_NAMES, &SOURCE_LABELS),
        "dests": vocab(&DEST_NAMES, &DEST_LABELS),
        "curves": vocab(&CURVE_NAMES, &CURVE_LABELS),
        "slots": [slots_json(&matrices[0]), slots_json(&matrices[1])],
    })
    .to_string()
}

/// One layer's slots as `[{source, dest, curve, scale}, …]` — the wire shape the
/// page reads, shared by the open-time seed ([`build_matrix_json`]) and the
/// running echo (the `kind: "matrix"` view payload). One writer, so the two can
/// never drift into disagreeing about field names or value encodings.
fn slots_json(table: &MatrixTable) -> serde_json::Value {
    serde_json::Value::Array(
        table
            .slots
            .iter()
            .map(|s| {
                serde_json::json!({
                    "source": s.source as u8,
                    "dest": s.dest as u8,
                    "curve": s.curve as u8,
                    "scale": s.scale_src as u8,
                })
            })
            .collect(),
    )
}

/// Two-layer surface (0216): the faceplate's `patchCount` is the engine's
/// per-layer patch-param count, so bridge.js translates a Layer-2 binding by
/// `+PATCH_COUNT`. Spliced into the `__PATCH_COUNT__` slot.
const PATCH_COUNT: u32 = vxn1b_engine::PATCH_COUNT as u32;

/// Web boot head. Spliced into the `__WEB_BOOT_HEAD__` slot of
/// `faceplate.html`, which sits just BEFORE the inlined faceplate `<script>`,
/// so it runs first. Carries its own `__*_JSON__` placeholders — see
/// [`assemble_faceplate`], which splices this in before the JSON pass.
///
/// On the web there is no wry IPC: the faceplate posts opcodes via
/// `window.ipc.postMessage(json)`, so this installs a SYNCHRONOUS queuing stub
/// for `window.ipc` (and `__VXN_PARAMS__`/`__VXN_SUBDIVISIONS__`/`__VXN_PATCH_COUNT__`
/// fallbacks the shared bridge.js reads). The faceplate's `init()` fires a
/// `ready` opcode during page parse — before the async controller boot
/// finishes — so the stub buffers every opcode in `__VXN_UI_QUEUE__` until
/// `faceplate-bridge.mjs` drains it.
const WEB_BOOT_HEAD: &str = r#"<style>
/* DOM text-input popup (replaces the desktop floating NSWindow). */
.vxn-ti-backdrop {
  position: fixed; inset: 0; z-index: 1000;
  display: flex; align-items: center; justify-content: center;
  background: rgba(0, 0, 0, 0.45);
}
.vxn-ti-box {
  background: #1b1b1f; color: #eee; padding: 16px 18px; border-radius: 8px;
  min-width: 240px; box-shadow: 0 8px 32px rgba(0, 0, 0, 0.5);
  font: 13px system-ui, sans-serif;
}
.vxn-ti-title { margin-bottom: 8px; opacity: 0.85; }
.vxn-ti-input {
  width: 100%; box-sizing: border-box; padding: 6px 8px; font-size: 14px;
  background: #0e0e10; color: #fff; border: 1px solid #444; border-radius: 4px;
}
.vxn-ti-input:focus { outline: none; border-color: #6a8; }
</style>
<script>
// Web transport shim. No wry IPC here: buffer faceplate opcodes until the
// controller wasm is live, then faceplate-bridge.mjs drains the queue.
(function () {
  var q = (window.__VXN_UI_QUEUE__ = window.__VXN_UI_QUEUE__ || []);
  // `window.ipc` is what bridge.js `_post` calls. Install a queuing stub now
  // (synchronous, before the faceplate <script> runs init()); the bridge
  // module replaces `.postMessage` with the live router once booted and flushes
  // the queue.
  if (!window.ipc) {
    window.ipc = { postMessage: function (json) { q.push(json); } };
  }
  // Descriptor data the bridge module reads (params already spliced into the
  // faceplate too; this is a redundant, structured copy so the module need not
  // scrape the page).
  window.__VXN_PARAMS__ = __PARAMS_JSON__;
  window.__VXN_SUBDIVISIONS__ = __SUBDIVISIONS_JSON__;
  window.__VXN_PATCH_COUNT__ = __PATCH_COUNT__;
})();
</script>
"#;

/// Module-loader tag spliced into the `__WEB_BOOT_LOADER__` slot of
/// `faceplate.html`, just AFTER the inlined faceplate `<script>`: it boots
/// `WebHost` + `WebController` and runs the bridge. Deferred (module scripts
/// always are), so it runs after the faceplate's synchronous `init()`.
const WEB_BOOT_LOADER: &str = "<script type=\"module\" src=\"./faceplate-bridge.mjs\"></script>\n";

/// Assemble the faceplate page for the STANDALONE WEB build. Reuses the exact
/// native splice ([`build_faceplate_html`]) so the markup, CSS, JS, and — the
/// param-descriptor JSON are byte-identical to the plugin's faceplate; the only
/// difference is the transport head. The wry IPC bridge is replaced by a
/// queuing `window.ipc` stub + the `faceplate-bridge.mjs` module loader, both
/// injected around the inlined faceplate `<script>`.
pub fn build_web_faceplate_html() -> String {
    // The browser build boots a fresh engine from the factory state, so the
    // factory topology *is* its live topology.
    let factory = vxn1b_engine::PluginState::factory_default();
    assemble_faceplate(
        WEB_BOOT_HEAD,
        WEB_BOOT_LOADER,
        &[factory.layers[0].matrix, factory.layers[1].matrix],
    )
}

/// Drop ESM module syntax from every line of `src`. The faceplate JS modules
/// carry `export` markers (and a couple of cross-module `import`s) so Node can
/// load them for the test suite; the splice loader concatenates them into one
/// inline `<script>` where module syntax is illegal, so we strip per line
/// before splicing.
fn strip_esm_exports(src: &str) -> String {
    // Thin local alias over the shared crate's implementation.
    vxn_core_ui_web::strip_esm_exports(src)
}

/// Concatenate the split panel source files into one ESM-stripped blob for the
/// `__PANELS_JS__` slot. Joined the same way as the bridge / browser concats
/// (`\n;\n`) so a trailing expression in one file can't fuse with the next
/// file's leading token.
fn panels_js() -> String {
    PANELS_FILES
        .iter()
        .map(|src| strip_esm_exports(src))
        .collect::<Vec<_>>()
        .join("\n;\n")
}

/// Tempo-sync subdivision labels. VXN1b has no sync partner yet, so this is an
/// empty list — the forked bridge.js still reads the `__SUBDIVISIONS_JSON__`
/// slot into `window.vxn.subdivisions`.
fn build_subdivisions_json() -> String {
    "[]".to_string()
}

fn build_params_json() -> String {
    let entries: Vec<String> = (0..TOTAL_PARAMS)
        .filter_map(|id| desc_for_clap_id(id).map(|d| (id, d)))
        .map(|(id, d)| format!(r#""{id}":{}"#, descriptor_to_json(d)))
        .collect();
    format!("{{{}}}", entries.join(","))
}

/// Serialise one param descriptor for the spliced `window.vxn.params` map.
///
/// Near-identical to [`vxn_core_ui_web::descriptor_to_json`] but kept local
/// deliberately: this returns the `String` the faceplate splice wants (the
/// shared one returns a `serde_json::Value`). The shape is the same, so if the
/// two ever diverge, reconcile here — the JS reads them identically.
fn descriptor_to_json(d: &ParamDesc) -> String {
    use serde_json::{Map, Value, json};
    let mut obj = Map::new();
    obj.insert("name".into(), json!(d.name));
    obj.insert("label".into(), json!(d.label));
    obj.insert("min".into(), json!(d.min));
    obj.insert("max".into(), json!(d.max));
    obj.insert("default".into(), json!(d.default));
    match d.kind {
        ParamKind::Float { unit, taper } => {
            obj.insert("kind".into(), json!("float"));
            obj.insert("unit".into(), json!(unit));
            obj.insert("taper".into(), json!(taper_to_json(taper)));
        }
        ParamKind::Int { unit } => {
            obj.insert("kind".into(), json!("int"));
            obj.insert("unit".into(), json!(unit));
        }
        ParamKind::Bool => {
            obj.insert("kind".into(), json!("bool"));
        }
        ParamKind::Enum { variants } => {
            obj.insert("kind".into(), json!("enum"));
            obj.insert("variants".into(), json!(variants));
        }
    }
    Value::Object(obj).to_string()
}

fn taper_to_json(t: Taper) -> serde_json::Value {
    use serde_json::json;
    match t {
        Taper::Linear => json!({"kind": "linear"}),
        Taper::Exp { mid } => json!({"kind": "exp", "mid": mid}),
        Taper::BipolarExp { mid } => json!({"kind": "bipolar-exp", "mid": mid}),
    }
}

// ── Faceplate page ──────────────────────────────────────────────────────────

/// Faceplate HTML scaffold. Four-row panel grid (forked from VXN1; the compact
/// three-row re-lay is a follow-up). Controls populated at runtime by the JS
/// modules. The HTML carries placeholders for the CSS and the JS modules so
/// each file stays editable on its own; `build_faceplate_html` splices them
/// back together at editor-open time.
const PLACEHOLDER_HTML: &str = include_str!("../assets/faceplate.html");
/// Stylesheet — spliced into the `<style>__CSS__</style>` slot of the HTML.
const FACEPLATE_CSS: &str = include_str!("../assets/faceplate.css");
/// IPC bootstrap + shared UI scaffolding. Defines the globals every later
/// module relies on, so it splices first inside `<script>`.
const BRIDGE_JS: &str = include_str!("../assets/bridge.js");
/// Preset browser panel glue.
const BROWSER_JS: &str = include_str!("../assets/browser.js");
/// Panel UI, split into cohesive modules. Splice order: `util/drag.js` first
/// (the shared drag/paint/clamp primitives the rest reference), then the widget
/// modules; `keys.js` and `preset-bar.js` run load-time IIFEs so they sit last.
const PANEL_UTIL_DRAG_JS: &str = include_str!("../assets/util/drag.js");
const PANEL_FADER_JS: &str = include_str!("../assets/panels/fader.js");
const PANEL_DISCRETE_JS: &str = include_str!("../assets/panels/discrete.js");
const PANEL_KEYS_JS: &str = include_str!("../assets/panels/keys.js");
const PANEL_PRESET_BAR_JS: &str = include_str!("../assets/panels/preset-bar.js");
const PANEL_MATRIX_JS: &str = include_str!("../assets/panels/matrix.js");
const PANEL_METER_JS: &str = include_str!("../assets/panels/meter.js");
/// The split panel source files, in splice order.
const PANELS_FILES: &[&str] = &[
    PANEL_UTIL_DRAG_JS,
    PANEL_FADER_JS,
    PANEL_DISCRETE_JS,
    PANEL_KEYS_JS,
    PANEL_PRESET_BAR_JS,
    PANEL_MATRIX_JS,
    // Meters (0240) — must precede dispatch.js, which calls `makeMeter` /
    // `meterRegistry` from `wireMeters`. The splice is order-sensitive: these
    // files share one concatenated scope with no module resolution.
    PANEL_METER_JS,
];
/// `init()` + per-tick ViewEvent dispatcher + dim rules. Splices last because
/// it references the panel objects defined above.
const DISPATCH_JS: &str = include_str!("../assets/dispatch.js");

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Assemble once per test run — `build_faceplate_html` walks every CLAP
    // id to build the descriptor map, so caching keeps the checks cheap.
    /// The factory topology — the seed a page gets when the plugin is still at
    /// its default patch. Live topology travels the same path (0246).
    fn factory_matrices() -> [MatrixTable; 2] {
        let f = vxn1b_engine::PluginState::factory_default();
        [f.layers[0].matrix, f.layers[1].matrix]
    }

    fn assembled() -> &'static str {
        use std::sync::OnceLock;
        static CACHED: OnceLock<String> = OnceLock::new();
        CACHED
            .get_or_init(|| build_faceplate_html(&factory_matrices()))
            .as_str()
    }

    #[test]
    fn no_placeholders_survive_splice() {
        for ph in [
            "__CSS__",
            "__BRIDGE_JS__",
            "__BROWSER_JS__",
            "__PANELS_JS__",
            "__DISPATCH_JS__",
            "__PARAMS_JSON__",
            "__SUBDIVISIONS_JSON__",
            "__MATRIX_JSON__",
            "__PATCH_COUNT__",
            "__WEB_BOOT_HEAD__",
            "__WEB_BOOT_LOADER__",
        ] {
            assert!(!assembled().contains(ph), "native page leaks placeholder {ph}");
        }
    }

    #[test]
    fn esm_exports_stripped() {
        assert!(!assembled().contains("export "));
        assert!(!assembled().contains("import "));
    }

    /// Every control primitive the faceplate mounts must have styling in the
    /// spliced sheet.
    ///
    /// Regression guard: a bulk edit that removed the retired tabbed-FX rules
    /// (0220) sliced from the FX banner to the *next* banner comment and took
    /// the toggle / button / dropdown primitives with it. Nothing failed —
    /// every unit test still passed, because the breakage was purely visual:
    /// toggle boxes vanished and labels fell back to the browser default size
    /// across all three tabs. Selector presence is cheap to assert and would
    /// have caught it at the source.
    /// Collect the selectors that actually *head a rule* in `css`.
    ///
    /// A plain substring search is not enough: `.ctl-tg-box` also occurs inside
    /// `.ctl-tg-row.active .ctl-tg-box`, so deleting its own rule leaves the
    /// text present and a `contains` check green. Only a rule head means the
    /// element is actually styled.
    fn rule_heads(css: &str) -> std::collections::HashSet<String> {
        // Strip block comments so a selector mentioned in prose doesn't count.
        let mut stripped = String::with_capacity(css.len());
        let mut rest = css;
        while let Some(open) = rest.find("/*") {
            stripped.push_str(&rest[..open]);
            match rest[open..].find("*/") {
                Some(close) => rest = &rest[open + close + 2..],
                None => {
                    rest = "";
                    break;
                }
            }
        }
        stripped.push_str(rest);

        let mut heads = std::collections::HashSet::new();
        for chunk in stripped.split('{') {
            // The head is whatever follows the previous rule's closing brace.
            let head = chunk.rsplit('}').next().unwrap_or("").trim();
            if head.is_empty() || head.starts_with('@') {
                continue;
            }
            for part in head.split(',') {
                let norm = part.split_whitespace().collect::<Vec<_>>().join(" ");
                if !norm.is_empty() {
                    heads.insert(norm);
                }
            }
        }
        heads
    }

    #[test]
    fn css_covers_every_control_primitive() {
        let heads = rule_heads(FACEPLATE_CSS);
        for selector in [
            // Toggles: the strip switches, button groups and header on/offs.
            ".ctl-tg-row",
            ".ctl-tg-box",
            ".ctl-tg-lbl",
            ".ctl-buttongroup",
            ".panel-header-switch",
            // Faders, dials, wave knobs, dropdowns.
            ".ctl-fader",
            ".ctl-fader-track",
            ".ctl-fader-thumb",
            ".ctl-label",
            ".ctl-dropdown",
            ".dial-grid",
            // Composite + state classes. `.dimmed` is only ever compounded
            // onto a cell, so assert the real head rather than a bare class.
            ".ctl-detune",
            ".ctl-detune-legato",
            ".ctl.dimmed",
            "[data-layer2-gated].dimmed",
            // Meters (0240/0241).
            ".meter-mount",
            ".meter-bar",
            ".meter-fill",
            ".meter-peak",
            ".meter-bar.clipped",
            // The GR variant only ever styles its children, never a bare
            // `.meter-gr` box — assert the head that actually exists.
            ".meter-gr .meter-fill",
            // Layout scaffolding for the tab shell.
            ".tab-strip",
            ".tab-pane",
            ".panel-strip",
            ".mixer-strip",
            ".mixer-split",
        ] {
            assert!(
                heads.contains(selector),
                "faceplate CSS has no rule for `{selector}` — that primitive is unstyled"
            );
        }
    }

    /// Every `data-control` / `data-meter` mount in the markup must name a
    /// param the table actually has (or, for meters, a frame key). Catches a
    /// panel rewrite that renames or drops a param behind the HTML's back.
    #[test]
    fn faceplate_mounts_resolve_to_real_params() {
        let page = PLACEHOLDER_HTML;
        for cap in page.match_indices("data-param=\"") {
            let rest = &page[cap.0 + "data-param=\"".len()..];
            let name = &rest[..rest.find('"').expect("closing quote")];
            assert!(
                vxn1b_engine::ParamId::from_name(name).is_some(),
                "faceplate mounts unknown param `{name}`"
            );
        }
    }

    #[test]
    fn params_json_is_valid_and_covers_table() {
        let json = build_params_json();
        let v: serde_json::Value = serde_json::from_str(&json).expect("descriptor JSON");
        let obj = v.as_object().expect("object root");
        // Every CLAP id is present: the two-layer surface (0216) — two patch
        // blocks + globals, not the inner per-synth table.
        let present = (0..TOTAL_PARAMS).filter(|id| desc_for_clap_id(*id).is_some()).count();
        assert_eq!(obj.len(), present, "params JSON entry count drift");
        assert_eq!(obj.len(), TOTAL_PARAMS, "expected the full two-layer CLAP surface");
        // Every kind serialises to one of the four discriminants.
        for (_id, desc) in obj {
            let kind = desc["kind"].as_str().unwrap_or("");
            assert!(
                matches!(kind, "float" | "int" | "bool" | "enum"),
                "unknown kind \"{kind}\" in {desc}",
            );
        }
    }

    /// 0246: the page's topology snapshot must be the LIVE matrix, not the
    /// factory patch. Nothing replays topology to a fresh page (it is not a CLAP
    /// param), so seeding from the factory made every source/dest combo revert
    /// on GUI close/reopen.
    #[test]
    fn matrix_json_carries_the_live_topology_not_the_factory() {
        use vxn1b_engine::matrix::{Curve, DestId, MatrixSlot, SourceId};
        let mut live = factory_matrices();
        // A route the factory patch does not have, on the second layer, in a
        // slot the factory leaves inert — so a factory seed can't fake it.
        live[1].slots[7] = MatrixSlot {
            source: SourceId::Aftertouch,
            dest: DestId::HpfCutoff,
            depth: 0.5,
            curve: Curve::Exp,
            scale_src: SourceId::ModWheel,
        };
        let json = build_matrix_json(&live);
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let slot = &v["slots"][1][7];
        assert_eq!(slot["source"], SourceId::Aftertouch as u8);
        assert_eq!(slot["dest"], DestId::HpfCutoff as u8);
        assert_eq!(slot["curve"], Curve::Exp as u8);
        assert_eq!(slot["scale"], SourceId::ModWheel as u8);
        // Layer 1 still reads the (unmodified) live table it was handed.
        assert_eq!(v["slots"][0][0]["source"], SourceId::Env2 as u8);
        // And the page really carries it — the splice, not just the builder.
        assert!(build_faceplate_html(&live).contains(&json));
    }

    /// 0247: the running echo and the open-time seed must describe a slot the
    /// same way, or the page would swap one wire shape for another and read
    /// `undefined` out of every combo. `slots_json` is the single writer — this
    /// pins that the seed really goes through it.
    #[test]
    fn echo_slot_shape_matches_the_open_time_seed() {
        use vxn1b_engine::matrix::{Curve, DestId, MatrixSlot, SourceId};
        let mut live = factory_matrices();
        live[0].slots[3] = MatrixSlot {
            source: SourceId::Velocity,
            dest: DestId::Resonance,
            depth: -0.25,
            curve: Curve::Log,
            scale_src: SourceId::Env1,
        };
        let seed: serde_json::Value =
            serde_json::from_str(&build_matrix_json(&live)).expect("valid JSON");
        assert_eq!(seed["slots"][0], slots_json(&live[0]));
        assert_eq!(seed["slots"][1], slots_json(&live[1]));
        // Depth is a CLAP param and rides ParamChanged — it must not appear in
        // either shape, or the page gets two sources of truth for one value.
        assert!(seed["slots"][0][3].get("depth").is_none());
    }

    #[test]
    fn web_page_params_are_byte_identical_to_native() {
        let json = build_params_json();
        let native = build_faceplate_html(&factory_matrices());
        let web = build_web_faceplate_html();
        assert!(native.contains(&json), "native page must carry params JSON");
        assert!(web.contains(&json), "web page must carry the SAME params JSON");
    }

    #[test]
    fn web_page_wires_boot() {
        let page = build_web_faceplate_html();
        assert!(page.contains("__VXN_UI_QUEUE__"), "boot head queue missing");
        assert!(
            page.contains(r#"<script type="module" src="./faceplate-bridge.mjs">"#),
            "faceplate-bridge module loader missing",
        );
        assert!(page.contains("vxn-ti-backdrop"), "text-input popup CSS missing");
    }
}
