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
/// The FX/Global pane is shorter for now (0220 fills it). Width was widened from
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
) -> Result<EditorHandle, OpenEditorError> {
    let html = build_faceplate_html();
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
        use vxn1b_engine::KeyOp;
        let ev = match op {
            "set_key_mode" => KeyOp::SetKeyMode(v.get("mode")?.as_u64()? as u8),
            "set_split_point" => KeyOp::SetSplitPoint(v.get("note")?.as_u64()? as u8),
            _ => return None,
        };
        Some(UiEvent::Custom(Box::new(ev)))
    }));
    config.serialise_custom_view = None;
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
fn build_faceplate_html() -> String {
    // Native plugin page: no web transport, so the `__WEB_BOOT_HEAD__` /
    // `__WEB_BOOT_LOADER__` slots are spliced empty.
    assemble_faceplate("", "")
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
fn assemble_faceplate(web_boot_head: &str, web_boot_loader: &str) -> String {
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
        .replace("__PATCH_COUNT__", &PATCH_COUNT.to_string())
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
    assemble_faceplate(WEB_BOOT_HEAD, WEB_BOOT_LOADER)
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
/// The split panel source files, in splice order.
const PANELS_FILES: &[&str] = &[
    PANEL_UTIL_DRAG_JS,
    PANEL_FADER_JS,
    PANEL_DISCRETE_JS,
    PANEL_KEYS_JS,
    PANEL_PRESET_BAR_JS,
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
    fn assembled() -> &'static str {
        use std::sync::OnceLock;
        static CACHED: OnceLock<String> = OnceLock::new();
        CACHED.get_or_init(build_faceplate_html).as_str()
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

    #[test]
    fn web_page_params_are_byte_identical_to_native() {
        let json = build_params_json();
        let native = build_faceplate_html();
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
