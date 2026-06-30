//! VXN1 web editor backend (E010 / 0039; thin shim since E024 / 0077).
//!
//! Thin wrapper over [`vxn_core_ui_web`]: [`open_editor`] assembles VXN1's
//! faceplate HTML (markup + CSS + the bridge/browser/panels/dispatch JS
//! modules + the param-descriptor JSON) and builds a
//! [`vxn_core_ui_web::WebEditorConfig`] carrying that HTML plus VXN1's
//! `parse_custom_ui` / `serialise_custom_view` hooks, then calls
//! `vxn_core_ui_web::open_editor`. The WebView lifecycle, the JS↔Rust IPC
//! bridge, the batched `evaluate_script` view-event sink, the corpus-
//! snapshot push, the parent-window adapter, and the native text-input
//! popup all live in the shared crate — this crate touches neither wry nor
//! raw-window-handle directly.
//!
//! The shared vocabulary (param/gesture/preset opcodes, `ParamChanged` /
//! `PresetLoaded` / … view events) is handled by `vxn-core-ui-web`; what
//! stays here is VXN1-specific: the faceplate asset splice, the
//! `Vxn1UiCustom` opcode parse ([`PARSE_CUSTOM`]) and `Vxn1ViewCustom`
//! serialise ([`SERIALISE_CUSTOM`]) hooks, the param-descriptor JSON
//! builder, and the standalone-web page assembly ([`build_web_faceplate_html`]).

use std::ffi::c_void;

use vxn_app::{
    ControllerHandle, CorpusHandle, KeyMode, Layer, PATCH_COUNT, ParamDesc,
    ParamKind, TOTAL_PARAMS, UNCATEGORIZED, UiEvent,
    desc_for_clap_id,
};
// `ViewEvent` is only named by the test-gated `batch_chunks` /
// `view_event_to_json` wrappers (and the test module via `use super::*`)
// since the live flush path moved into the shared crate (0077).
#[cfg(test)]
use vxn_app::ViewEvent;
use vxn_core_ui_web::{DEFAULT_MAX_BATCH_BYTES, WebEditorConfig};

// The WebView lifecycle, IPC bridge, batched view-event sink, corpus
// snapshot push, and native text-input popup all live in the shared
// crate (E001 / E024 0077). This crate re-exports its handle + error and
// supplies a `WebEditorConfig` carrying VXN1's faceplate HTML and the
// VXN1-specific opcode-parse / view-serialise hooks.
pub use vxn_core_ui_web::{EditorHandle, OpenEditorError, prompt_text};

/// Logical pixel dimensions of the editor. Matches the vizia editor's
/// [`vxn_ui_vizia::EDITOR_WIDTH`] / `_HEIGHT` so swapping backends doesn't reflow
/// the host's plugin window.
pub const EDITOR_WIDTH: u32 = 1024;
pub const EDITOR_HEIGHT: u32 = 772;

/// Open the VXN1 editor under `parent`. Thin wrapper over
/// [`vxn_core_ui_web::open_editor`]: builds VXN1's faceplate HTML and a
/// [`WebEditorConfig`] carrying the VXN1-specific opcode-parse
/// ([`PARSE_CUSTOM`]) and view-serialise ([`SERIALISE_CUSTOM`]) hooks, then
/// hands off to the shared crate. The WebView lifecycle, IPC bridge,
/// batched view-event sink, corpus-snapshot push, the 100 KB batch
/// chunking, the unsafe parent-handle plumbing, and the WebView2 user-data
/// folder override all live in core — this crate touches neither wry nor
/// raw-window-handle directly (E024 0077).
///
/// `parent` is the same raw pointer the host hands the clack shell in
/// `gui::set_parent` (NSView / HWND / xcb-window-id).
///
/// Errors (never panics — 0115, inherited from the shared `open_editor`)
/// on a null parent handle or a wry build failure; the clack shell maps it
/// to `PluginError` in `set_parent` so no unwind crosses the C ABI and the
/// plugin keeps rendering.
pub fn open_editor(
    parent: *mut c_void,
    ctrl: ControllerHandle,
    corpus: CorpusHandle,
) -> Result<EditorHandle, OpenEditorError> {
    let html = build_faceplate_html();
    let mut config = WebEditorConfig::new(html, EDITOR_WIDTH, EDITOR_HEIGHT);
    config.uncategorised_label = UNCATEGORIZED;
    config.max_batch_bytes = DEFAULT_MAX_BATCH_BYTES;
    // WebView2 user-data folder: `%LOCALAPPDATA%\VulpusLabs\VXN1\WebView2`
    // (the shared crate joins vendor/product/"WebView2"). Avoids the
    // admin-only `C:\Program Files\<host>\<exe>.WebView2` default.
    config.webview2_vendor = Some("VulpusLabs");
    config.webview2_product = Some("VXN1");
    config.parse_custom_ui = Some(PARSE_CUSTOM.clone());
    config.serialise_custom_view = Some(SERIALISE_CUSTOM.clone());
    vxn_core_ui_web::open_editor(parent, ctrl, corpus, config)
}

/// Splice the runtime param-descriptor JSON into the faceplate template. The
/// page reads it as `window.vxn.params = {...}`, a CLAP-id-keyed map of
/// `{name, label, kind, min, max, default, taper, unit, variants?}`. JSON
/// generation is one place so a future schema bump (e.g. adding a `module`
/// field) stays self-contained.
///
/// CSS + the three JS modules (bridge / panels / dispatch) live in sibling
/// files spliced in here — the wry WebView serves the page via `with_html`,
/// so external `<link href>` / `<script src>` would need a custom protocol
/// handler to resolve. Inlining keeps the page self-contained without that
/// plumbing. JS splice order matters: bridge defines `window.vxn` /
/// `__vxn` / `valuePop` / `statusPill`, panels register controls and
/// browser/preset/keys UI against that bridge, dispatch wires `init()` and
/// the ViewEvent fan-out last.
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
/// before the `__*_JSON__` replaces, so the boot head's own `__PARAMS_JSON__`
/// / `__SUBDIVISIONS_JSON__` / `__PATCH_COUNT__` tokens pick up the very same
/// descriptor data as the body — byte-identical, single-sourced, no separate
/// pre-replace pass.
///
/// The browser logic is shared (vxn-core-ui-web); splice it (ESM markers
/// stripped) immediately before VXN1's `browser.js` glue, which calls the
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
    // Shared widget primitives (0140): valuePop / wireDrag / cutoff-tuned
    // math. Spliced into the bridge slot (which runs first) so their stripped
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
        .replace("__PANELS_JS__", &strip_esm_exports(PANELS_JS))
        .replace("__DISPATCH_JS__", &strip_esm_exports(DISPATCH_JS))
        .replace("__PARAMS_JSON__", &build_params_json())
        .replace("__SUBDIVISIONS_JSON__", &build_subdivisions_json())
        .replace("__PATCH_COUNT__", &PATCH_COUNT.to_string())
}

/// Web boot head (E018 / 0057). Spliced into the `__WEB_BOOT_HEAD__` slot of
/// `faceplate.html`, which sits just BEFORE the inlined faceplate `<script>`,
/// so it runs first. Carries its own `__*_JSON__` placeholders — see
/// [`assemble_faceplate`], which splices this in before the JSON pass.
///
/// On the web there is no wry IPC: the faceplate posts opcodes via
/// `window.ipc.postMessage(json)`, so this installs a SYNCHRONOUS queuing stub
/// for `window.ipc` (and `__VXN_PARAMS__`/`__VXN_SUBDIVISIONS__`/`__VXN_PATCH_COUNT__`
/// fallbacks the shared bridge.js reads when the `__*_JSON__` placeholders are
/// left unspliced). The faceplate's `init()` fires a `ready` opcode during page
/// parse — before the async controller boot finishes — so the stub buffers every
/// opcode in `__VXN_UI_QUEUE__` until `faceplate-bridge.mjs` drains it. The
/// faceplate splice ALWAYS replaces the `__*_JSON__` placeholders here (so the
/// descriptor table is byte-identical to the plugin), so the globals below are a
/// belt-and-braces echo of the same data for the bridge module to read without
/// re-parsing the page.
const WEB_BOOT_HEAD: &str = r#"<style>
/* E018 / 0061 DOM text-input popup (replaces the desktop floating NSWindow). */
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
// E018 web transport shim. No wry IPC here: buffer faceplate opcodes until the
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

/// Module-loader tag (E018 / 0057) spliced into the `__WEB_BOOT_LOADER__`
/// slot of `faceplate.html`, just AFTER the inlined faceplate `<script>`: it
/// boots `WebHost` + `WebController` and runs the bridge. Deferred (module
/// scripts always are), so it runs after the faceplate's synchronous `init()`.
const WEB_BOOT_LOADER: &str = "<script type=\"module\" src=\"./faceplate-bridge.mjs\"></script>\n";

/// Assemble the faceplate page for the STANDALONE WEB build (E018). Reuses the
/// exact native splice ([`build_faceplate_html`]) so the markup, CSS, JS, and —
/// critically — the param-descriptor JSON are byte-identical to the plugin's
/// faceplate; the only difference is the transport head. The wry IPC bridge is
/// replaced by a queuing `window.ipc` stub + the `faceplate-bridge.mjs` module
/// loader, both injected around the inlined faceplate `<script>`.
///
/// Used by the `gen-web-page` bin, which `cargo xtask web` runs to write
/// `target/web-dist/index.html`. Kept here (not in xtask) so the JSON-shaping
/// stays single-sourced and xtask carries no wry-pulling dependency.
pub fn build_web_faceplate_html() -> String {
    // The web boot head + module loader splice into the dedicated
    // `__WEB_BOOT_HEAD__` / `__WEB_BOOT_LOADER__` slots that bracket the
    // inlined faceplate `<script>` in `faceplate.html` — same `str::replace`
    // mechanism as every other splice point, no `<script>`-marker byte
    // surgery. `assemble_faceplate` splices the boot head before its
    // `__*_JSON__` pass, so the head's `__PARAMS_JSON__` / `__SUBDIVISIONS_JSON__`
    // / `__PATCH_COUNT__` tokens get the SAME descriptor data as the body
    // (byte-identical, single-sourced).
    assemble_faceplate(WEB_BOOT_HEAD, WEB_BOOT_LOADER)
}

/// Drop ESM module syntax from every line of `src`. The four faceplate JS
/// modules carry `export` markers (and a couple of cross-module `import`s
/// since E015 / 0079) so Node can load them for the test suite; the splice
/// loader concatenates them into one inline `<script>` where module syntax
/// is illegal, so we strip per line before splicing. `export const X = …`
/// becomes `const X = …` (bare declaration — exactly what these files were
/// before E015); `import { ... } from '...';` becomes an empty line (the
/// splice already puts every binding in one shared scope, so cross-module
/// refs resolve without the import).
fn strip_esm_exports(src: &str) -> String {
    // Single source of truth lives in the shared crate now (both synths and
    // the shared preset-browser asset strip identically). Kept as a thin
    // local alias so the four splice call sites + the unit test below read
    // unchanged.
    vxn_core_ui_web::strip_esm_exports(src)
}

/// Tempo-sync subdivision labels (vxn_app::sync::SUBDIVISIONS), spliced into
/// the page as `window.vxn.subdivisions`. The LFO-rate fader's display reads
/// from this list when its sync partner is on (0042 / 0015) — matches the
/// vizia editor's `sync_partner` override, which indexes the same table.
fn build_subdivisions_json() -> String {
    let labels: Vec<String> = vxn_app::sync::SUBDIVISIONS
        .iter()
        .map(|s| format!("\"{}\"", s.label))
        .collect();
    format!("[{}]", labels.join(","))
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
/// (0020) deliberately: this returns the `String` the faceplate splice wants
/// (the shared one returns a `serde_json::Value`, and still routes through the
/// `as_object_mut().expect(...)` pattern this crate is purging). The shape is
/// the same, so if the two ever diverge, reconcile here — the JS reads them
/// identically.
fn descriptor_to_json(d: &ParamDesc) -> String {
    use serde_json::{Map, Value, json};
    // Build the object map directly (0020): no `json!({...})` +
    // `as_object_mut().expect(...)` round-trip, so there is no panic path to
    // reason about — the value is an object by construction.
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

fn taper_to_json(t: vxn_app::Taper) -> serde_json::Value {
    use serde_json::json;
    match t {
        vxn_app::Taper::Linear => json!({"kind": "linear"}),
        vxn_app::Taper::Exp { mid } => json!({"kind": "exp", "mid": mid}),
    }
}

// Parent-window adapter (`ParentWindow` + per-OS `build_raw`) moved to the
// shared crate (E024 0077); `vxn_core_ui_web::open_editor` builds the raw
// handle and returns `OpenEditorError::BadParent` on a null/zero parent.

// ── IPC inbound: JSON → UiEvent ─────────────────────────────────────────────

/// Parse one IPC message into a [`UiEvent`], routing unknown opcodes
/// through [`PARSE_CUSTOM`]. Test-only since 0077: the live IPC handler
/// inside the shared `open_editor` calls `vxn_core_ui_web::parse_ui_event`
/// with the same `PARSE_CUSTOM` hook (handed through the config), so this
/// wrapper exists only to keep the VXN1 opcode-parse contract unit-tested
/// this side without standing up a WebView.
///
/// Wire shape: `{ "op": "<opcode>", ...fields }`.
#[cfg(test)]
fn parse_ui_event(body: &str) -> Option<UiEvent> {
    vxn_core_ui_web::parse_ui_event(body, Some(&PARSE_CUSTOM))
}

/// Parse a VXN1-specific opcode into the matching `UiEvent::Custom`
/// payload. Called by `vxn_core_ui_web::parse_ui_event` after its
/// shared-vocabulary table returns `None`.
fn parse_vxn1_custom_ui(op: &str, v: &serde_json::Value) -> Option<UiEvent> {
    match op {
        "reset_layer" => Some(
            vxn_app::Vxn1UiCustom::ResetLayer {
                layer: parse_layer(v.get("layer")?)?,
            }
            .into_event(),
        ),
        "set_key_mode" => Some(
            vxn_app::Vxn1UiCustom::SetKeyMode {
                mode: parse_key_mode(v.get("mode")?)?,
            }
            .into_event(),
        ),
        "set_split_point" => Some(
            vxn_app::Vxn1UiCustom::SetSplitPoint {
                note: v.get("note")?.as_u64()? as u8,
            }
            .into_event(),
        ),
        "set_edit_layer" => Some(
            vxn_app::Vxn1UiCustom::SetEditLayer {
                layer: parse_layer(v.get("layer")?)?,
            }
            .into_event(),
        ),
        _ => None,
    }
}

static PARSE_CUSTOM: std::sync::LazyLock<vxn_core_ui_web::ParseCustomUi> =
    std::sync::LazyLock::new(|| std::sync::Arc::new(parse_vxn1_custom_ui));

fn parse_layer(v: &serde_json::Value) -> Option<Layer> {
    match v.as_str()? {
        "upper" => Some(Layer::Upper),
        "lower" => Some(Layer::Lower),
        _ => None,
    }
}

fn parse_key_mode(v: &serde_json::Value) -> Option<KeyMode> {
    Some(KeyMode::from_u8(v.as_u64()? as u8))
}

// ── ViewEvent → JSON batches ────────────────────────────────────────────────

/// Build one or more JSON-array literals from a tick batch. Delegates
/// to [`vxn_core_ui_web::batch_chunks`] with the VXN1 custom-serialise
/// hook so per-synth `Vxn1ViewCustom` payloads keep their existing
/// JSON shape on the wire. Test-only since 0077: the live flush path is
/// `vxn_core_ui_web::EditorHandle::flush_view_events`, which is handed
/// `SERIALISE_CUSTOM` through the config. Retained so the batching /
/// dedup / custom-serialise contract stays unit-tested this side.
#[cfg(test)]
fn batch_chunks(events: &[ViewEvent], max_bytes: usize) -> Vec<String> {
    vxn_core_ui_web::batch_chunks(events, max_bytes, Some(&SERIALISE_CUSTOM))
}

/// Serialise a [`ViewEvent`] to a JSON string the page can read. Thin
/// wrapper around [`vxn_core_ui_web::view_event_to_json`] with the
/// VXN1 custom hook. Test-only — the live path batches via the shared
/// `EditorHandle`.
#[cfg(test)]
fn view_event_to_json(ev: &ViewEvent) -> String {
    vxn_core_ui_web::view_event_to_json(ev, Some(&SERIALISE_CUSTOM)).unwrap_or_default()
}

/// VXN1's `Vxn1ViewCustom` → JSON. Wired into the shared
/// `batch_chunks` / `view_event_to_json` via [`SERIALISE_CUSTOM`].
fn serialise_vxn1_view_custom(payload: &dyn std::any::Any) -> Option<serde_json::Value> {
    use serde_json::json;
    let custom = payload.downcast_ref::<vxn_app::Vxn1ViewCustom>()?;
    Some(match custom {
        vxn_app::Vxn1ViewCustom::KeyModeChanged { mode } => json!({
            "kind": "key_mode_changed",
            "mode": *mode as u8,
        }),
        vxn_app::Vxn1ViewCustom::SplitPointChanged { note } => json!({
            "kind": "split_point_changed",
            "note": *note,
        }),
        vxn_app::Vxn1ViewCustom::EditLayerChanged { layer } => json!({
            "kind": "edit_layer_changed",
            "layer": match layer { Layer::Upper => "upper", Layer::Lower => "lower" },
        }),
    })
}

static SERIALISE_CUSTOM: std::sync::LazyLock<vxn_core_ui_web::SerialiseCustomView> =
    std::sync::LazyLock::new(|| std::sync::Arc::new(serialise_vxn1_view_custom));

/// Serialise a [`vxn_app::PresetCorpus`] for the JS browser panel. Thin
/// wrapper around [`vxn_core_ui_web::corpus_snapshot_json`] with VXN1's
/// `UNCATEGORIZED` label. Test-only since 0077: the live corpus push is
/// done by the shared `EditorHandle`, which holds the `uncategorised_label`
/// from the config. Retained so the grouping/sort contract stays tested
/// against VXN1's label this side.
#[cfg(test)]
fn corpus_snapshot_json(corpus: &vxn_app::PresetCorpus) -> String {
    vxn_core_ui_web::corpus_snapshot_json(corpus, UNCATEGORIZED)
}

// ── Faceplate page ──────────────────────────────────────────────────────────

/// Faceplate HTML scaffold (0040). Four-row panel grid; controls populated
/// at runtime by the JS modules. The HTML carries placeholders for the CSS
/// and the three JS modules so each file stays editable on its own without
/// hunting for the boundaries inside a 3500-line blob — `build_faceplate_html`
/// splices them back together at editor-open time.
const PLACEHOLDER_HTML: &str = include_str!("../assets/faceplate.html");
/// Stylesheet — spliced into the `<style>__CSS__</style>` slot of the HTML.
const FACEPLATE_CSS: &str = include_str!("../assets/faceplate.css");
/// IPC bootstrap + shared UI scaffolding (`window.vxn` / `window.__vxn`,
/// text-input bridge, value popup, status pill). Defines the globals every
/// later module relies on, so it splices first inside `<script>`.
const BRIDGE_JS: &str = include_str!("../assets/bridge.js");
/// Preset browser panel — corpus model, folder/preset rendering, search,
/// context menu, modal confirms (delete + save-as), DnD. Splices between
/// bridge and the rest of panels because the bar IIFE
/// (`const presetBar = …`) references `browserPanel`.
const BROWSER_JS: &str = include_str!("../assets/browser.js");
/// Panel UI — preset bar, Keys panel, waveform glyphs, control primitives
/// (fader / wave / switch / buttongroup / dropdown / header-switch /
/// detune-legato). Registers everything against `model.controls` so
/// dispatch can fan ViewEvents to the right cell.
const PANELS_JS: &str = include_str!("../assets/panels.js");
/// `init()` + per-tick ViewEvent dispatcher + dim rules + layer rebind.
/// Splices last because it references the panel objects defined above.
const DISPATCH_JS: &str = include_str!("../assets/dispatch.js");

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use vxn_app::{ParamId, PresetMeta, PresetSource};

    // The null-parent → `OpenEditorError::BadParent` (not panic) guarantee
    // (0115) is now covered by `vxn-core-ui-web`'s own
    // `build_raw_null_parent_is_err_not_panic` test, since `build_raw`
    // moved into the shared crate (E024 0077).

    #[test]
    fn parses_set_param_norm() {
        let ev = parse_ui_event(r#"{"op":"set_param_norm","id":42,"norm":0.5}"#).unwrap();
        match ev {
            UiEvent::SetParamNorm { id, norm } => {
                assert_eq!(id.raw(), 42);
                assert!((norm - 0.5).abs() < 1e-6);
            }
            _ => panic!("wrong variant: {ev:?}"),
        }
    }

    #[test]
    fn parses_factory_load() {
        let ev = parse_ui_event(r#"{"op":"load_factory","index":7}"#).unwrap();
        match ev {
            UiEvent::LoadPreset { source: PresetSource::Factory { index } } => {
                assert_eq!(index, 7);
            }
            _ => panic!("wrong variant: {ev:?}"),
        }
    }

    #[test]
    fn parses_mutation_ops() {
        // 0051: each of the user-side mutation flows posts a dedicated
        // op. The controller already handles the matching UiEvents.
        let ev = parse_ui_event(
            r#"{"op":"rename_preset","path":"/u/x.preset","new_name":"Y"}"#,
        ).unwrap();
        match ev {
            UiEvent::RenamePreset { path, new_name } => {
                assert_eq!(path, PathBuf::from("/u/x.preset"));
                assert_eq!(new_name, "Y");
            }
            _ => panic!("wrong variant: {ev:?}"),
        }
        let ev = parse_ui_event(r#"{"op":"delete_preset","path":"/u/x.preset"}"#).unwrap();
        assert!(matches!(ev, UiEvent::DeletePreset { ref path } if path == &PathBuf::from("/u/x.preset")));
        let ev = parse_ui_event(
            r#"{"op":"move_preset","path":"/u/x.preset","dest_folder":"Bass"}"#,
        ).unwrap();
        match ev {
            UiEvent::MovePreset { path, dest_folder } => {
                assert_eq!(path, PathBuf::from("/u/x.preset"));
                assert_eq!(dest_folder.as_deref(), Some("Bass"));
            }
            _ => panic!("wrong variant: {ev:?}"),
        }
        // dest_folder: null routes to user root.
        let ev = parse_ui_event(
            r#"{"op":"move_preset","path":"/u/x.preset","dest_folder":null}"#,
        ).unwrap();
        assert!(matches!(
            ev,
            UiEvent::MovePreset { dest_folder: None, .. },
        ));
        let ev = parse_ui_event(
            r#"{"op":"rename_folder","old_name":"Bass","new_name":"Bassline"}"#,
        ).unwrap();
        match ev {
            UiEvent::RenameFolder { old_name, new_name } => {
                assert_eq!(old_name, "Bass");
                assert_eq!(new_name, "Bassline");
            }
            _ => panic!("wrong variant: {ev:?}"),
        }
        let ev = parse_ui_event(r#"{"op":"delete_folder","name":"Bass"}"#).unwrap();
        assert!(matches!(ev, UiEvent::DeleteFolder { ref name } if name == "Bass"));
        let ev = parse_ui_event(r#"{"op":"new_folder","suggested":"Pads"}"#).unwrap();
        assert!(matches!(ev, UiEvent::NewFolder { ref suggested } if suggested == "Pads"));
    }

    #[test]
    fn parses_user_load() {
        // 0050: browser panel posts `load_user` with the absolute path
        // from the corpus snapshot when the user clicks a user-side row.
        let ev = parse_ui_event(r#"{"op":"load_user","path":"/u/p/Bass/My Patch.preset"}"#).unwrap();
        match ev {
            UiEvent::LoadPreset { source: PresetSource::User { path } } => {
                assert_eq!(path, PathBuf::from("/u/p/Bass/My Patch.preset"));
            }
            _ => panic!("wrong variant: {ev:?}"),
        }
    }

    #[test]
    fn parses_layer_and_key_mode() {
        use vxn_app::Vxn1UiCustom;
        let ev = parse_ui_event(r#"{"op":"set_edit_layer","layer":"lower"}"#).unwrap();
        let UiEvent::Custom(payload) = ev else {
            panic!("expected Custom");
        };
        let custom = *payload.downcast::<Vxn1UiCustom>().expect("downcast");
        assert!(matches!(custom, Vxn1UiCustom::SetEditLayer { layer: Layer::Lower }));

        let ev = parse_ui_event(r#"{"op":"set_key_mode","mode":2}"#).unwrap();
        let UiEvent::Custom(payload) = ev else {
            panic!("expected Custom");
        };
        let custom = *payload.downcast::<Vxn1UiCustom>().expect("downcast");
        assert!(matches!(custom, Vxn1UiCustom::SetKeyMode { mode: KeyMode::Split }));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_ui_event("not json").is_none());
        assert!(parse_ui_event(r#"{"op":"unknown"}"#).is_none());
        assert!(parse_ui_event(r#"{"op":"set_param_norm","id":42}"#).is_none());
    }

    #[test]
    fn parses_step_preset_signed_delta() {
        // 0049: prev posts -1, next posts +1. delta is signed so the parser
        // must accept negative values.
        let ev = parse_ui_event(r#"{"op":"step_preset","delta":-1}"#).unwrap();
        assert!(matches!(ev, UiEvent::StepPreset { delta: -1 }));
        let ev = parse_ui_event(r#"{"op":"step_preset","delta":1}"#).unwrap();
        assert!(matches!(ev, UiEvent::StepPreset { delta: 1 }));
    }

    #[test]
    fn parses_save_preset_with_and_without_folder() {
        // 0049: Save As. `folder: null` saves to user root; a string
        // names the destination subfolder (0050+ sources this from the
        // browser panel's selection).
        let ev = parse_ui_event(
            r#"{"op":"save_preset","name":"Pad 1","folder":null}"#,
        )
        .unwrap();
        match ev {
            UiEvent::SavePreset { name, folder } => {
                assert_eq!(name, "Pad 1");
                assert!(folder.is_none());
            }
            _ => panic!("wrong variant: {ev:?}"),
        }
        let ev = parse_ui_event(
            r#"{"op":"save_preset","name":"Brassy","folder":"Lead"}"#,
        )
        .unwrap();
        match ev {
            UiEvent::SavePreset { name, folder } => {
                assert_eq!(name, "Brassy");
                assert_eq!(folder.as_deref(), Some("Lead"));
            }
            _ => panic!("wrong variant: {ev:?}"),
        }
    }

    fn param_changed(id: usize, plain: f32) -> ViewEvent {
        ViewEvent::ParamChanged {
            id: ParamId::new(id),
            plain,
            norm: plain,
            display: format!("{plain}"),
        }
    }

    #[test]
    fn dedup_keeps_latest_param_per_id() {
        // Three writes to id 1 in a tick → only the last one ships.
        // Dedup lives in `vxn-core-ui-web::batch_chunks` post-0009; we
        // verify the behaviour by inspecting the batched output.
        let events = vec![
            param_changed(1, 0.1),
            param_changed(2, 0.2),
            param_changed(1, 0.3),
            param_changed(1, 0.4),
            ViewEvent::Status { line: "ok".into() },
            param_changed(2, 0.5),
        ];
        let chunks = batch_chunks(&events, 100_000);
        assert_eq!(chunks.len(), 1);
        let payload = &chunks[0];
        // Each id appears exactly once with its last value.
        assert!(payload.contains("\"id\":1") && payload.contains("0.4"));
        assert!(payload.contains("\"id\":2") && payload.contains("0.5"));
        // Earlier values dropped.
        assert!(!payload.contains("0.1"));
        assert!(!payload.contains("0.3"));
        // Status survives.
        assert!(payload.contains("\"kind\":\"status\""));
    }

    #[test]
    fn batch_chunks_single_under_cap() {
        let events = vec![param_changed(1, 0.5), param_changed(2, 0.5)];
        let chunks = batch_chunks(&events, 10_000);
        assert_eq!(chunks.len(), 1, "should fit in one chunk");
        let v: serde_json::Value = serde_json::from_str(&chunks[0]).unwrap();
        let arr = v.as_array().expect("array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["kind"], "param_changed");
    }

    #[test]
    fn batch_chunks_splits_above_cap() {
        // 200 distinct ids — each event JSON is ~80 bytes, so a tight cap
        // forces multiple chunks. Every chunk must parse as a JSON array,
        // and concatenating their contents must equal the deduped input.
        let events: Vec<ViewEvent> = (0..200).map(|i| param_changed(i, i as f32 * 0.01)).collect();
        let chunks = batch_chunks(&events, 1_000);
        assert!(chunks.len() > 1, "tight cap should split: got {}", chunks.len());
        let mut total = 0;
        for c in &chunks {
            let v: serde_json::Value = serde_json::from_str(c).unwrap();
            let arr = v.as_array().expect("array");
            total += arr.len();
            assert!(c.len() <= 1_000 + 200, "chunk size respects cap (slack: {})", c.len());
        }
        assert_eq!(total, 200, "all events present across chunks");
    }

    #[test]
    fn batch_chunks_empty_yields_nothing() {
        assert!(batch_chunks(&[], 10_000).is_empty());
    }

    #[test]
    fn batch_chunks_dedup_applies_before_chunking() {
        // Two writes to the same id collapse before chunking.
        let events = vec![param_changed(1, 0.1), param_changed(1, 0.9)];
        let chunks = batch_chunks(&events, 10_000);
        assert_eq!(chunks.len(), 1);
        let v: serde_json::Value = serde_json::from_str(&chunks[0]).unwrap();
        let arr = v.as_array().expect("array");
        assert_eq!(arr.len(), 1);
        assert!((arr[0]["plain"].as_f64().unwrap() - 0.9).abs() < 1e-6);
    }

    #[test]
    fn view_event_serializes() {
        let s = view_event_to_json(&ViewEvent::ParamChanged {
            id: ParamId::new(3),
            plain: 1.25,
            norm: 0.5,
            display: "1.25 Hz".into(),
        });
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["kind"], "param_changed");
        assert_eq!(v["id"], 3);
        assert_eq!(v["display"], "1.25 Hz");
    }

    #[test]
    fn preset_loaded_serializes_factory_source() {
        let s = view_event_to_json(&ViewEvent::PresetLoaded {
            meta: PresetMeta { name: "Brassy".into(), ..Default::default() },
            source: Some(PresetSource::Factory { index: 12 }),
            warnings: vec!["clamped".into()],
        });
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["kind"], "preset_loaded");
        assert_eq!(v["name"], "Brassy");
        assert_eq!(v["source"]["kind"], "factory");
        assert_eq!(v["source"]["index"], 12);
        assert_eq!(v["warnings"][0], "clamped");
    }

    #[test]
    fn corpus_changed_serializes_follow_path() {
        let s = view_event_to_json(&ViewEvent::PresetCorpusChanged {
            follow: Some(PathBuf::from("/tmp/x.preset")),
        });
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["kind"], "preset_corpus_changed");
        assert_eq!(v["follow"], "/tmp/x.preset");
    }

    // ── Faceplate structural checks (0040) ─────────────────────────────────
    //
    // Substring-only — pulling an HTML parser in just to assert presence
    // would be overkill. The asserts here catch silent regressions (a row
    // dropped, a panel renamed, a data attr lost) without pinning markup.

    // Assemble once per test run — `build_faceplate_html` walks every CLAP
    // id to build the descriptor map, so caching keeps the structural-check
    // suite under a millisecond instead of paying that per test.
    fn assembled() -> &'static str {
        use std::sync::OnceLock;
        static CACHED: OnceLock<String> = OnceLock::new();
        CACHED.get_or_init(build_faceplate_html).as_str()
    }

    fn count(needle: &str) -> usize {
        assembled().matches(needle).count()
    }

    #[test]
    fn faceplate_has_banner_and_preset_bar_slot() {
        assert!(assembled().contains(r#"class="banner""#));
        assert!(assembled().contains("VULPUS LABS"));
        assert!(assembled().contains("VXN-1"));
        assert!(assembled().contains(r#"class="preset-bar-slot""#));
    }

    #[test]
    fn faceplate_has_four_rows() {
        for r in 1..=4 {
            assert!(
                assembled().contains(&format!(r#"data-row="{r}""#)),
                "missing data-row=\"{r}\"",
            );
        }
        // Rows 1-3 = 5 panels each; row 4 = 4 panels (E018 / 0098 folded
        // Chorus/Delay/Reverb into a single tabbed FX panel). 5+5+5+4 = 19.
        // Catches an accidental row collapse or duplicate emit.
        assert_eq!(count(r#"class="panel""#), 19, "panel count drift");
    }

    #[test]
    fn faceplate_panel_names_match_rows() {
        // Same titles as `vxn_ui_vizia::ROWS`; reordering or rename would have to
        // happen here in lockstep. Row 4's FX panel (E018 / 0098) folded the
        // prior Chorus/Delay/Reverb trio behind a left vertical tab strip.
        let expected: &[&[&str]] = &[
            &["LFO 1", "LFO 2", "Osc 1", "Osc 2", "Mixer"],
            &["Env 1", "Env 2", "VCA", "Filter", "Filter Mod"],
            &["Pitch Mod", "PWM Mod", "Cross Mod", "Mod Wheel", "Bend"],
            &["Keys", "Voice", "FX", "Master"],
        ];
        for row in expected {
            for name in *row {
                assert!(
                    assembled().contains(&format!(r#"data-name="{name}""#)),
                    "missing panel {name}",
                );
            }
        }
    }

    #[test]
    fn faceplate_layered_panels_match_vxn_ui_vizia() {
        // Layered = panel has at least one per-patch (Upper/Lower) entry in
        // `vxn_ui_vizia::ROWS`. Mirror that list here so we notice if a panel's
        // entry mix changes upstream.
        let layered = [
            "LFO 1", "Osc 1", "Osc 2", "Mixer", "Env 1", "Env 2", "VCA",
            "Filter", "Filter Mod", "Pitch Mod", "PWM Mod", "Cross Mod",
            "Mod Wheel", "Bend", "Voice",
        ];
        for name in layered {
            let marker = format!(r#"data-name="{name}" data-layered"#);
            assert!(
                assembled().contains(&marker),
                "panel {name} missing data-layered",
            );
        }
        // Count attribute occurrences only — `data-layered>` skips the CSS
        // `[data-layered]` selector hit.
        assert_eq!(
            count("data-layered>"),
            layered.len(),
            "extra/missing data-layered panel",
        );
    }

    #[test]
    fn fx_panel_hosts_per_tab_header_switches() {
        // E018 / 0098 (revised): the FX panel no longer uses the panel-
        // header `data-header-toggle` idiom — each tab carries its own
        // on/off switch inline. Confirm no panel div emits the attribute
        // (`data-header-toggle ` with trailing space — followed by
        // another attribute on the same tag). The bare token still
        // appears inside CSS `[data-header-toggle]` selectors that the
        // splice bundles in, so a substring count on the raw token
        // overcounts. Each `.fx-tab` button hosts a `header-switch` slot
        // bound to its `*_on` param.
        assert_eq!(
            count("data-header-toggle "),
            0,
            "no panel div should still emit data-header-toggle after 0098 (tab-hosted switches)",
        );
        for param in ["phaser_on", "chorus_on", "delay_on", "reverb_on"] {
            let needle = format!(
                r#"<span class="panel-header-toggle-slot fx-tab-switch" data-control="header-switch" data-param="{param}""#,
            );
            assert!(
                assembled().contains(&needle),
                "FX tab missing in-tab header-switch for {param}",
            );
        }
    }

    #[test]
    fn faceplate_css_vars_match_vxn_ui_vizia_constants() {
        // Pixel literals live in CSS vars (ticket: "a future resize policy
        // should be one variable change"). Sanity check the load-bearing
        // ones against `vxn_ui_vizia` constants.
        assert!(assembled().contains("--panel-h: 156px"));
        assert!(assembled().contains("--col-h: 120px"));
        assert!(assembled().contains("--fader-h: 74px"));
        assert!(assembled().contains("--dial: 62px"));
        assert!(assembled().contains("--banner-h: 26px"));
        assert!(assembled().contains("--preset-bar-h: 30px"));
        assert!(assembled().contains("--pad-outer: 10px"));
    }

    #[test]
    fn faceplate_row_panel_widths_match_vizia() {
        // Stretch shares from `vxn_ui_vizia::panel_view`'s `match title` block. If
        // upstream tweaks a share, this fails — keeping the HTML pinned to
        // the vizia layout the user already approved.
        for (sel, share) in [
            ("LFO 1", "1.2"),
            ("LFO 2", "0.7"),
            ("Osc 1", "1.2"),
            ("Osc 2", "1.2"),
            ("Mixer", "1.1"),
            ("Env 1", "0.8"),
            ("Env 2", "0.8"),
            ("VCA", "0.75"),
            ("Filter", "1.15"),
            ("Filter Mod", "1.0"),
        ] {
            assert!(
                assembled()
                    .contains(&format!(r#".panel[data-name="{sel}"]"#))
                    && assembled().contains(&format!("flex-grow: {share}")),
                "share for {sel} ≠ {share}",
            );
        }
        // Bend is the only fixed-width panel.
        assert!(assembled().contains("flex: 0 0 54px"));
    }

    #[test]
    fn faceplate_bridge_object_intact() {
        // Bridge from 0039 still present — 0040 only adds layout.
        assert!(assembled().contains("window.vxn"));
        assert!(assembled().contains("window.ipc.postMessage"));
        assert!(assembled().contains("onViewEvent"));
    }

    #[test]
    fn faceplate_batched_bridge_wired() {
        // 0046: Rust calls `window.__vxn.applyViewEvents(arr)` once per
        // controller tick. Bootstrap installs a buffering stub; init() swaps
        // in the real dispatcher.
        assert!(assembled().contains("window.__vxn"));
        assert!(assembled().contains("applyViewEvents"));
        // Bootstrap stub still funnels into `_earlyViewEvents` so events
        // that race the inline init() are not lost.
        assert!(assembled().contains("_earlyViewEvents"));
    }

    #[test]
    fn faceplate_esm_exports_stripped() {
        // 0076: the four asset files declare ESM `export` markers so Node
        // can `import` them for the E015 test suite, but wry's inline
        // `<script>` slot can't take module syntax. `strip_esm_exports`
        // peels the prefix per line during splice; the assembled HTML
        // must contain no `export ` markers, and the load-bearing
        // declarations (`window.vxn = {`, `function init()`) survive
        // intact under their bare names.
        assert!(
            !assembled().contains("export "),
            "strip_esm_exports left an `export ` marker in the assembled HTML",
        );
        // E015 / 0079: cross-module `import { ... } from './...';` lines
        // must also drop. The strip leaves the line blank so concat-side
        // scope still owns the binding.
        assert!(
            !assembled().contains("import "),
            "strip_esm_exports left an `import ` line in the assembled HTML",
        );
        assert!(assembled().contains("function init()"));
        assert!(assembled().contains("window.vxn = {"));
    }

    #[test]
    fn strip_esm_exports_drops_prefix_per_line() {
        let src = "export const X = 1;\nexport function f() {}\nexport default 7;\nconst Y = 2;\n";
        let out = strip_esm_exports(src);
        assert_eq!(
            out,
            "const X = 1;\nfunction f() {}\n7;\nconst Y = 2;\n",
        );
        // Non-prefix lines pass through; trailing-newline shape preserved.
        let no_trailing = "export const X = 1;";
        assert_eq!(strip_esm_exports(no_trailing), "const X = 1;");
        // E015 / 0079: imports drop to empty lines.
        let with_import = "import { foo } from './bar.js';\nconst X = 1;\n";
        assert_eq!(strip_esm_exports(with_import), "\nconst X = 1;\n");
    }

    #[test]
    fn faceplate_text_input_bridge_wired() {
        // 0048: faceplate exposes `window.vxn.promptText(title, initial,
        // cb)` and the dispatcher routes `text_input_result` back to the
        // pending callback. JS plumbing only — the actual NSWindow is
        // verified by running the plugin in-DAW (see ticket Acceptance).
        assert!(assembled().contains("window.vxn.promptText"));
        assert!(assembled().contains("_textInputCallbacks"));
        assert!(assembled().contains("send.requestTextInput("));
        assert!(assembled().contains("ev.kind === 'text_input_result'"));
    }

    #[test]
    fn parses_request_and_result_text_input() {
        // Faceplate → controller: `request_text_input` carries the
        // correlation id + title + initial.
        let ev = parse_ui_event(
            r#"{"op":"request_text_input","id":"ti1","title":"Rename","initial":"Pad 1"}"#,
        )
        .unwrap();
        match ev {
            UiEvent::RequestTextInput { id, title, initial } => {
                assert_eq!(id, "ti1");
                assert_eq!(title, "Rename");
                assert_eq!(initial, "Pad 1");
            }
            _ => panic!("wrong variant: {ev:?}"),
        }
        // Direct page-side result post (in-page tests): null `value`
        // round-trips as `None`, string round-trips as `Some`.
        let ev = parse_ui_event(r#"{"op":"text_input_result","id":"ti1","value":null}"#).unwrap();
        assert!(matches!(
            ev,
            UiEvent::TextInputResult { ref id, value: None } if id == "ti1"
        ));
        let ev = parse_ui_event(
            r#"{"op":"text_input_result","id":"ti2","value":"new name"}"#,
        )
        .unwrap();
        match ev {
            UiEvent::TextInputResult { id, value } => {
                assert_eq!(id, "ti2");
                assert_eq!(value.as_deref(), Some("new name"));
            }
            _ => panic!("wrong variant: {ev:?}"),
        }
    }

    #[test]
    fn text_input_result_serializes() {
        // Controller → page: commit echoes the string; cancel echoes
        // null (JS dispatcher fires the pending callback with null).
        let s = view_event_to_json(&ViewEvent::TextInputResult {
            id: "ti9".into(),
            value: Some("Pad 1".into()),
        });
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["kind"], "text_input_result");
        assert_eq!(v["id"], "ti9");
        assert_eq!(v["value"], "Pad 1");

        let s = view_event_to_json(&ViewEvent::TextInputResult {
            id: "ti10".into(),
            value: None,
        });
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert!(v["value"].is_null());
    }

    #[test]
    fn faceplate_status_pill_wired() {
        // 0046: Status ViewEvent flashes the status chip. 0049 re-anchored
        // it from the lower-right corner into the preset bar; the
        // `.status-pill` class + `statusPill.flash` API are unchanged so
        // the bridge contract here stays the same.
        assert!(assembled().contains(".status-pill"));
        assert!(assembled().contains(".status-pill.visible"));
        assert!(assembled().contains("statusPill"));
        assert!(assembled().contains("statusPill.flash"));
        assert!(assembled().contains("ev.kind === 'status'"));
    }

    #[test]
    fn faceplate_preset_bar_wired() {
        // 0049: preset bar replaces the empty placeholder div. Markup
        // carries the current-name slot, prev/next walker buttons, the
        // Browse toggle, the Save As button, and the in-bar status chip.
        for id in [
            "id=\"pbar-prev\"",
            "id=\"pbar-name\"",
            "id=\"pbar-next\"",
            "id=\"pbar-browse\"",
            "id=\"pbar-save\"",
            "id=\"pbar-status\"",
        ] {
            assert!(assembled().contains(id), "preset bar missing {id}");
        }
        // JS bridge: prev/next post `step_preset` with signed delta; Save
        // As funnels through the 0048 popup then posts `save_preset` with
        // `folder: null`; preset_loaded sets the name.
        assert!(assembled().contains("send.stepPreset(-1)"));
        assert!(assembled().contains("send.stepPreset(1)"));
        assert!(assembled().contains("send.savePreset("));
        // Save As funnels through the in-WebView modal (name field +
        // folder dropdown) rather than going straight through the native
        // popup. The modal posts `save_preset` directly; presetBar just
        // opens it. `browserPanel.getSaveFolder()` is still exposed for
        // other call sites but no longer the Save As path.
        assert!(assembled().contains("browserPanel.openSaveAs"));
        assert!(assembled().contains("ev.kind === 'preset_loaded'"));
        assert!(assembled().contains("presetBar.setName"));
        // 0050: Browse toggles the panel itself via `browserPanel.setOpen`;
        // the `onOpenChange` callback drives the bar's active-class mirror.
        // (0081 dropped the dead `window.vxn._browserOpen` write.)
        assert!(assembled().contains("browserPanel.setOpen"));
    }

    #[test]
    fn faceplate_browser_mutation_flows_wired() {
        // 0051: every mutation op the controller exposes has a JS post
        // site inside the browser panel. The IIFE wires:
        // - Rename: posts `rename_preset` / `rename_folder` via the
        //   text-input popup.
        // - Delete: modal confirm posts `delete_preset` / `delete_folder`
        //   (the Vizia version's two-click row-armed pattern was scrapped
        //   here — the right-click menu obscured the row text).
        // - Move to: submenu posts `move_preset` with the destination
        //   folder (or null for user root).
        // - New Folder: "+ New" button on the user header posts
        //   `new_folder` after the popup commits.
        assert!(assembled().contains("send.renamePreset("));
        assert!(assembled().contains("send.renameFolder("));
        assert!(assembled().contains("send.deletePreset("));
        assert!(assembled().contains("send.deleteFolder("));
        assert!(assembled().contains("send.movePreset("));
        assert!(assembled().contains("send.newFolder("));
        // Modal confirm primitive present; ESC tears down modal → menu →
        // panel in that order (one level per press).
        assert!(assembled().contains("openDeleteConfirm"));
        assert!(assembled().contains(".browser-modal"));
        assert!(assembled().contains(".browser-modal-backdrop"));
        assert!(assembled().contains("if (modalEl)"));
        // Right-click hooks on both row types (factory rows must not
        // attach one — the JS gates by selectedFolder.kind / key.kind).
        assert!(assembled().contains("'contextmenu'"));
        // Move-to submenu helper present; mirrors `vxn_ui_vizia::move_targets`.
        // 0077 lifted `moveTargets` to module scope (so the Node test
        // suite can import it pure) and added `corpus` as an explicit arg.
        assert!(assembled().contains("moveTargets(currentName, corpus)"));
        assert!(assembled().contains(".browser-menu"));
        assert!(assembled().contains(".browser-submenu"));
        assert!(assembled().contains(".browser-new-folder"));
    }

    #[test]
    fn faceplate_save_as_modal_wired() {
        // Save As modal hosts a name field (captured via the native
        // popup for spacebar-safe entry) + a folder dropdown over user
        // folders. The modal posts `save_preset { name, folder }`.
        assert!(assembled().contains("openSaveAsModal"));
        // The name field reuses the injected `promptText` (VXN1's glue wires
        // it to `window.vxn.promptText`) so Space and friends still route
        // through the native NSWindow on macOS.
        assert!(assembled().contains("promptText('Preset name'"));
        // Folder choices come from a `<select>` populated from the corpus.
        assert!(assembled().contains("folderOptions"));
        assert!(assembled().contains(".save-as-select"));
        // Modal anchors over the faceplate root (injected by the glue), not
        // the browser panel — so Save As works whether the browser is open
        // or not.
        assert!(assembled().contains("faceplateRoot().appendChild(wrap)"));
        assert!(assembled().contains("faceplateRoot: () => document.getElementById('faceplate')"));
        // Save button is disabled until the name field is non-empty
        // (gateOk toggles the disabled attribute directly).
        assert!(assembled().contains("gateOk"));
        assert!(assembled().contains(".browser-modal-btn:disabled"));
    }

    #[test]
    fn faceplate_browser_search_is_cross_folder() {
        // Non-empty query: the right pane switches to flat search results
        // covering the whole corpus (factory + user) instead of filtering
        // within the selected folder only.
        assert!(assembled().contains("collectSearchHits"));
        assert!(assembled().contains("'Factory · '"));
        assert!(assembled().contains("'User · '"));
        // Search-mode row carries name + muted origin label.
        assert!(assembled().contains(".browser-row.search-row"));
        assert!(assembled().contains(".browser-row-origin"));
    }

    #[test]
    fn faceplate_browser_panel_wired() {
        // 0050: floating two-pane browser. Markup carries the search input,
        // the folders + presets panes, and the click-outside backdrop. The
        // panel and its backdrop start hidden (`hidden` attribute, toggled
        // by `setOpen`).
        for needle in [
            r#"id="browser-panel""#,
            r#"id="browser-backdrop""#,
            r#"id="browser-folders""#,
            r#"id="browser-presets""#,
            r#"id="browser-search-input""#,
            r#"id="browser-search-clear""#,
        ] {
            assert!(assembled().contains(needle), "browser panel missing {needle}");
        }
        // JS module + Rust→JS corpus channel.
        assert!(assembled().contains("const browserPanel"));
        assert!(assembled().contains("window.__vxn.applyPresetCorpus"));
        // Bootstrap stub funnels the first snapshot into `_earlyPresetCorpus`
        // so any corpus push that races init() is replayed.
        assert!(assembled().contains("_earlyPresetCorpus"));
        // Click handlers: folder click rerenders presets, preset click posts
        // load_factory or load_user (browser panel routes by folder kind).
        assert!(assembled().contains("send.loadFactory("));
        assert!(assembled().contains("send.loadUser("));
        // Dismissal: ESC + outside-click backdrop both close the panel.
        assert!(assembled().contains("e.key !== 'Escape'"));
        assert!(assembled().contains("backdropEl.addEventListener('click'"));
        // Highlight: preset_loaded fans `source` into the panel's
        // currently-loaded marker.
        assert!(assembled().contains("browserPanel.setCurrentSource"));
        // Section headers match the Vizia browser's labels.
        assert!(assembled().contains("'FACTORY'"));
        assert!(assembled().contains("'USER'"));
    }

    #[test]
    fn corpus_snapshot_groups_and_sorts() {
        use vxn_app::{UserFolderEntry, UserPresetEntry};
        let factory = vec![
            PresetMeta { name: "zeta".into(), category: Some("Lead".into()), ..Default::default() },
            PresetMeta { name: "Alpha".into(), category: Some("Lead".into()), ..Default::default() },
            PresetMeta { name: "Pad-A".into(), category: Some("Pad".into()), ..Default::default() },
            PresetMeta { name: "loose".into(), category: None, ..Default::default() },
        ];
        let user = vec![
            UserFolderEntry {
                name: Some("Bass".into()),
                presets: vec![
                    UserPresetEntry {
                        path: PathBuf::from("/u/Bass/B.preset"),
                        meta: PresetMeta { name: "B".into(), ..Default::default() },
                        folder: Some("Bass".into()),
                    },
                    UserPresetEntry {
                        path: PathBuf::from("/u/Bass/a.preset"),
                        meta: PresetMeta { name: "a".into(), ..Default::default() },
                        folder: Some("Bass".into()),
                    },
                ],
            },
            UserFolderEntry {
                name: None,
                presets: vec![UserPresetEntry {
                    path: PathBuf::from("/u/loose.preset"),
                    meta: PresetMeta { name: "loose".into(), ..Default::default() },
                    folder: None,
                }],
            },
            UserFolderEntry {
                name: Some("Aux".into()),
                presets: vec![],
            },
        ];
        let corpus = vxn_app::PresetCorpus { factory, user };
        let s = corpus_snapshot_json(&corpus);
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        // Factory groups sorted by category (case-insensitive), each
        // group's presets sorted by name.
        let fac = v["factory"].as_array().unwrap();
        let cats: Vec<&str> = fac.iter().map(|g| g["category"].as_str().unwrap()).collect();
        assert_eq!(cats, vec!["Lead", "Pad", UNCATEGORIZED]);
        let lead = fac[0]["presets"].as_array().unwrap();
        assert_eq!(lead[0]["name"], "Alpha");
        assert_eq!(lead[1]["name"], "zeta");
        // Factory index points back into the original corpus order
        // (so `load_factory { index }` works).
        assert_eq!(lead[0]["index"], 1);
        assert_eq!(lead[1]["index"], 0);
        // Uncategorised group carries the orphan factory preset.
        assert_eq!(fac[2]["presets"][0]["name"], "loose");

        // User folders: root (None) first, then sorted named folders;
        // each folder's presets sorted by name.
        let user = v["user"].as_array().unwrap();
        assert!(user[0]["name"].is_null(), "root folder must come first");
        assert_eq!(user[1]["name"], "Aux");
        assert_eq!(user[2]["name"], "Bass");
        let bass = user[2]["presets"].as_array().unwrap();
        assert_eq!(bass[0]["name"], "a");
        assert_eq!(bass[1]["name"], "B");
        assert_eq!(bass[0]["path"], "/u/Bass/a.preset");
    }

    // ── Row 1 + Row 2 control mount points (0041, 0041a, 0042, 0043) ────

    #[test]
    fn row1_osc_mixer_panels_have_expected_mounts() {
        // Wave + four faders per Osc panel; four level faders + one Col
        // switch on the Mixer; LFO 1 (Shape/Rate/Delay/Fade up top, Sync +
        // Free toggles in the strip) and LFO 2 (Shape/Rate, Sync in the
        // strip). Param names are descriptor `name`s so a `PatchParam` enum
        // reorder doesn't break the HTML.
        for (kind, name, label) in [
            // LFO 1
            ("wave",   "lfo_shape",       "Shape"),
            ("fader",  "lfo_rate",        "Rate"),
            ("fader",  "lfo1_delay_time", "Delay"),
            ("fader",  "lfo1_fade",       "Fade"),
            ("switch", "lfo_sync",        "Sync"),
            ("switch", "lfo1_free_run",   "Free"),
            // LFO 2
            ("wave",   "lfo2_shape", "Shape"),
            ("fader",  "lfo2_rate",  "Rate"),
            ("switch", "lfo2_sync",  "Sync"),
            // Osc 1
            ("wave",  "osc1_wave",   "Wave"),
            ("fader", "osc1_octave", "Oct"),
            ("fader", "osc1_coarse", "Semi"),
            ("fader", "osc1_fine",   "Fine"),
            ("fader", "osc1_pw",     "PW"),
            // Osc 2
            ("wave",  "osc2_wave",   "Wave"),
            ("fader", "osc2_octave", "Oct"),
            ("fader", "osc2_coarse", "Semi"),
            ("fader", "osc2_fine",   "Fine"),
            ("fader", "osc2_pw",     "PW"),
            // Mixer
            ("fader",  "osc1_level",  "Osc1"),
            ("fader",  "osc2_level",  "Osc2"),
            ("fader",  "sub_level",   "Sub"),
            ("fader",  "noise_level", "Noise"),
            ("switch", "noise_color", "Col"),
        ] {
            let marker = format!(
                r#"data-control="{kind}" data-param="{name}" data-label="{label}""#,
            );
            assert!(
                assembled().contains(&marker),
                "Row 1 mount point missing: {marker}",
            );
        }
    }

    #[test]
    fn row2_env_filter_panels_have_expected_mounts() {
        // Env 1/2: ADSR faders + Shape switch in the bottom strip (Vizia
        // maps the 2-variant Lin/Exp enum to a switch via `in_bottom_strip`).
        // VCA: AmpLfoSrc dropdown + Depth fader; AmpEnvBypass in strip.
        // Filter: HPF/Cutoff/Reso/Drive faders + Mode dropdown; Slope (12/24
        // dB enum) and KeyTrk (bool) ride the strip. Filter Mod: four fixed
        // depths into cutoff (E006), no source selectors. Names match the
        // `ParamDesc.name` fields so a `PatchParam` enum reorder doesn't
        // break the HTML.
        for (kind, name, label) in [
            // Env 1
            ("fader",  "env1_attack",  "A"),
            ("fader",  "env1_decay",   "D"),
            ("fader",  "env1_sustain", "S"),
            ("fader",  "env1_release", "R"),
            ("switch", "env1_shape",   "Shape"),
            // Env 2
            ("fader",  "env2_attack",  "A"),
            ("fader",  "env2_decay",   "D"),
            ("fader",  "env2_sustain", "S"),
            ("fader",  "env2_release", "R"),
            ("switch", "env2_shape",   "Shape"),
            // VCA
            ("buttongroup", "amp_lfo_src",    "LFO"),
            ("fader",       "amp_lfo_depth",  "Depth"),
            ("switch",      "amp_env_bypass", "Gate"),
            // Filter
            ("fader",       "hpf_cutoff",       "HPF"),
            ("fader",       "cutoff",           "Cutoff"),
            ("fader",       "resonance",        "Reso"),
            ("fader",       "drive",            "Drive"),
            ("buttongroup", "filter_mode",      "Mode"),
            ("switch",      "filter_slope",     "Slope"),
            ("switch",      "cutoff_tuned",     "Tuned"),
            // Filter Mod
            ("fader", "vel_cutoff_depth",  "Vel"),
            ("fader", "cutoff_lfo1_depth", "LFO1"),
            ("fader", "cutoff_lfo2_depth", "LFO2"),
            ("fader", "cutoff_env_depth",  "Env1"),
            ("fader", "filter_key_track",  "Key"),
        ] {
            let marker = format!(
                r#"data-control="{kind}" data-param="{name}" data-label="{label}""#,
            );
            assert!(
                assembled().contains(&marker),
                "Row 2 mount point missing: {marker}",
            );
        }
    }

    #[test]
    fn row3_mod_route_panels_have_expected_mounts() {
        // 0044: Pitch Mod / PWM Mod each carry two route columns (depth
        // fader + source buttongroup). Cross Mod is the wide custom panel
        // (Type buttongroup + Amount fader, Src buttongroup + Mod fader).
        // Mod Wheel = four cutoff/pwm/reso/pitch destination faders. Bend
        // is the single-fader pinned-width panel. Names match the
        // `ParamDesc.name` fields so a `PatchParam` enum reorder doesn't
        // break the HTML.
        for (kind, name, label) in [
            // Pitch Mod
            ("buttongroup", "pitch_lfo_src",      "LFO"),
            ("switch",      "pitch_lfo_mod_only", "Mod"),
            ("buttongroup", "pitch_env_src",      "Env"),
            ("switch",      "pitch_env_mod_only", "Mod"),
            // PWM Mod
            ("buttongroup", "pwm_lfo_src",   "LFO"),
            ("buttongroup", "pwm_env_src",   "Env"),
            // Cross Mod
            ("buttongroup", "cross_mod_type",       "Type"),
            ("fader",       "cross_mod_amount",     "Amt"),
            // Mod Wheel
            ("fader", "mod_wheel_pwm",        "PWM"),
            ("fader", "mod_wheel_cutoff",     "Cutoff"),
            ("fader", "mod_wheel_reso",       "Reso"),
            ("fader", "mod_wheel_cross_mod_sweep", "X-Mod"),
            // Bend
            ("fader", "pitch_wheel_depth", "Range"),
        ] {
            let marker = format!(
                r#"data-control="{kind}" data-param="{name}" data-label="{label}""#,
            );
            assert!(
                assembled().contains(&marker),
                "Row 3 mount point missing: {marker}",
            );
        }
        // Pitch Mod / PWM Mod depth faders carry `data-no-label` — the
        // route header (LFO / Env) is the only column label, matching the
        // source buttongroup beside them.
        for name in [
            "pitch_lfo_depth",
            "pitch_env_depth",
            "pwm_lfo_depth",
            "pwm_env_depth",
        ] {
            let marker = format!(
                r#"data-control="fader" data-param="{name}" data-dim-when-src-off="#,
            );
            assert!(
                assembled().contains(&marker),
                "Pitch Mod depth fader missing: {marker}",
            );
            assert!(
                !assembled().contains(&format!(r#"data-param="{name}" data-label="#)),
                "Pitch Mod depth fader {name} should not carry data-label",
            );
        }
    }

    #[test]
    fn row4_voice_master_fx_panels_have_expected_mounts() {
        // E018 / 0098: Voice / FX (tabbed Phaser/Chorus/Delay/Reverb) /
        // Master. Voice carries AssignMode (display-order 0,3,1,2 →
        // Poly/Twin/Unison/Solo) + Detune-Legato + Glide. Master is
        // Tune/Volume/Drift faders with Oversample + Limit toggles in the
        // bottom strip. The FX panel hosts four tab panes — every effect's
        // header-switch is mounted (CSS hides the inactive ones), and
        // every fader/switch behind each tab is bound normally. Names =
        // descriptor names.
        for (kind, name, label) in [
            // Voice
            ("buttongroup",   "assign_mode",     "Assign"),
            ("detune-legato", "unison_detune",   "Detune"),
            ("fader",         "portamento_time", "Glide"),
            // Master
            ("fader",  "master_tune",   "Tune"),
            ("fader",  "master_volume", "Volume"),
            ("fader",  "master_drift",  "Drift"),
            ("switch", "oversample",    "OvSmp"),
            ("switch", "limiter_on",    "Limit"),
            // FX → Phaser tab
            ("header-switch", "phaser_on",    ""),
            ("fader",         "phaser_rate",  "Rate"),
            ("fader",         "phaser_depth", "Depth"),
            ("fader",         "phaser_fb",    "FB"),
            ("fader",         "phaser_mix",   "Mix"),
            // FX → Chorus tab
            ("header-switch", "chorus_on",    ""),
            ("fader",         "chorus_rate",  "Rate"),
            ("fader",         "chorus_depth", "Depth"),
            ("fader",         "chorus_mix",   "Mix"),
            // FX → Delay tab
            ("header-switch", "delay_on",       ""),
            ("fader",         "delay_time",     "Time"),
            ("fader",         "delay_feedback", "FB"),
            ("fader",         "delay_mix",      "Mix"),
            ("switch",        "delay_sync",     "Sync"),
            // FX → Reverb tab (FDN — four direct knobs).
            ("header-switch", "reverb_on",    ""),
            ("fader",         "reverb_size",  "Size"),
            ("fader",         "reverb_decay", "Decay"),
            ("fader",         "reverb_damp",  "Damp"),
            ("fader",         "reverb_mix",   "Mix"),
        ] {
            // Header-switch slots carry no `data-label` attribute; assert
            // on the kind+name pair instead.
            let needle = if kind == "header-switch" {
                format!(r#"data-control="{kind}" data-param="{name}""#)
            } else {
                format!(
                    r#"data-control="{kind}" data-param="{name}" data-label="{label}""#,
                )
            };
            assert!(
                assembled().contains(&needle),
                "Row 4 mount point missing: {needle}",
            );
        }
        // Voice's AssignMode buttongroup carries the display permutation
        // (descriptor order = Poly/Unison/Solo/Twin → display order =
        // Poly/Twin/Unison/Solo). If the descriptor order changes, this
        // attribute changes alongside; the test guards the wiring.
        assert!(
            assembled().contains(r#"data-param="assign_mode" data-label="Assign" data-order="0,3,1,2""#),
            "AssignMode missing display-order remap",
        );
        // Detune-Legato carries its two extra param-name dependencies so
        // a layer rebind can re-resolve both alongside the primary param.
        assert!(
            assembled().contains(r#"data-legato-param="legato""#),
            "Detune-Legato missing data-legato-param",
        );
        assert!(
            assembled().contains(r#"data-mode-param="assign_mode""#),
            "Detune-Legato missing data-mode-param",
        );
    }

    #[test]
    fn control_tallies_match_all_rows() {
        // Global mount-point tally — catches duplicate mounts / typos that
        // accept a missing `<div>` somewhere else. Counts each control
        // kind across all four rows.
        //
        // Faders:
        //   Row 1: LFO1 3 (Rate/Delay/Fade), LFO2 1 (Rate), Osc1 4, Osc2 4, Mixer 3 = 15
        //   Row 2: Env1 4, Env2 4, VCA 1, Filter 4, FilterMod 5 (+ KeyTrk) = 18
        //   Row 3: PitchMod 2, PwmMod 2, CrossMod 1, ModWheel 4, Bend 1      = 10
        //   Row 4: Voice 1 (Glide), Master 3 (Tune/Volume/Drift),
        //          FX → Phaser 4 (Rate/Depth/FB/Mix),
        //          FX → Chorus 3 (Rate/Depth/Mix),
        //          FX → Delay 3 (Time/FB/Mix),
        //          FX → Reverb 4 (Size/Decay/Damp/Mix)                       = 18
        //   Total = 61.
        // Waves: 4 (LFO 1/2 Shape, Osc 1/2 Wave).
        // Switches:
        //   Row 1: 4 (LfoSync, Lfo2Sync, Lfo1FreeRun, NoiseColor)
        //   Row 2: 5 (Env1Shape, Env2Shape, Gate, Slope, CutoffTuned) —
        //          0100 moved KeyTrk out (bool switch → amount fader,
        //          lives in Filter Mod now); CutoffTuned added in the
        //          Filter panel strip.
        //   Row 3: 2 (PitchLfoModOnly, PitchEnvModOnly)
        //   Row 4: 3 (Oversample as multi-toggle row, LimiterOn,
        //            DelaySync)
        //   Total = 14.
        // Button groups:
        //   Row 2: 2 (AmpLfoSrc, FilterMode)
        //   Row 3: 5 (Pitch/PWM LFO+Env sources, CrossModType)
        //   Row 4: 1 (AssignMode) — Oversample renders as a horizontal switch
        //     row at the bottom of Master, not a vertical buttongroup column.
        //   Total = 8.
        // Header switches: 4 (Phaser, Chorus, Delay, Reverb) — all four are
        // mounted (one per `*_on` param); the FX panel's `data-active-tab`
        // CSS hides the inactive three.
        // Detune-Legato composite: 1 (Voice).
        // Leading space disambiguates DOM mounts (` data-control="X"`) from
        // CSS attribute selectors (`[data-control="X"]`) that the splice
        // bundles into the same assembled string.
        assert_eq!(
            assembled().matches(r#" data-control="fader""#).count(),
            63,
            "expected 63 fader cells across all four rows",
        );
        assert_eq!(
            assembled().matches(r#" data-control="wave""#).count(),
            4,
            "expected 4 wave cells (LFO 1, LFO 2, Osc 1, Osc 2)",
        );
        assert_eq!(
            assembled().matches(r#" data-control="switch""#).count(),
            14,
            "expected 14 switch cells (Row 1 + Row 2 + Row 3 + Row 4)",
        );
        assert_eq!(
            assembled().matches(r#" data-control="buttongroup""#).count(),
            8,
            "expected 8 buttongroup cells (Row 2 + Row 3 + Row 4)",
        );
        assert_eq!(
            assembled().matches(r#" data-control="dropdown""#).count(),
            0,
            "no dropdown cells expected (all enums fit ButtonGroup)",
        );
        assert_eq!(
            assembled().matches(r#" data-control="header-switch""#).count(),
            4,
            "expected 4 header-switch cells (Phaser, Chorus, Delay, Reverb)",
        );
        assert_eq!(
            assembled().matches(r#" data-control="detune-legato""#).count(),
            1,
            "expected 1 detune-legato composite (Voice)",
        );
    }

    #[test]
    fn mod_route_dim_rules_present() {
        // 0044: Cross Mod's Amount fader dims unless Type = FM (matches
        // vxn_ui_vizia::xmod_pair's FM-only enable); Mod fader dims when
        // Src = Off. Pitch/PWM Mod follow the same convention — the
        // *depth fader* dims when its source reads Off, not the source
        // selector itself (selector stays bright so a routed-Off path is
        // still readable / clickable).
        assert!(
            assembled().contains(r#"data-dim-unless-fm="cross_mod_type""#),
            "Cross Mod Amount missing data-dim-unless-fm wiring",
        );
        for (depth, src) in [
            ("pitch_lfo_depth", "pitch_lfo_src"),
            ("pitch_env_depth", "pitch_env_src"),
            ("pwm_lfo_depth",   "pwm_lfo_src"),
            ("pwm_env_depth",   "pwm_env_src"),
            // VCA's Amp LFO Depth follows the same rule (Off / LFO 1 /
            // LFO 2 source → fader dims on Off).
            ("amp_lfo_depth",   "amp_lfo_src"),
        ] {
            // Pitch Mod / PWM Mod depth faders dropped their `data-label`
            // (route header names the column), so the marker between
            // data-param and data-dim-when-src-off differs from the others.
            let marker = if depth.starts_with("pitch_") || depth.starts_with("pwm_") {
                format!(
                    r#"data-param="{depth}" data-dim-when-src-off="{src}""#,
                )
            } else {
                format!(
                    r#"data-param="{depth}" data-label="Depth" data-dim-when-src-off="{src}""#,
                )
            };
            assert!(
                assembled().contains(&marker),
                "route depth {depth} missing dim-when-src-off=\"{src}\"",
            );
        }
        // Route-column source selectors must NOT carry the self-dim
        // marker — selectors stay bright; only the paired fader dims.
        assert_eq!(
            assembled().matches("data-dim-when-zero").count(),
            0,
            "route-col source selectors should no longer self-dim",
        );
        // JS dispatch wires the generic dim rule into ParamChanged.
        assert!(assembled().contains("applyDimRulesFor("));
        assert!(assembled().contains("collectDimRuleSpecs"));
    }

    #[test]
    fn edit_layer_rebind_wired() {
        // 0045: EditLayerChanged ViewEvent dispatch + layer-rebind logic
        // present. The actual rebind walks LAYERED_CELLS and re-resolves
        // each per-patch name → id via paramIdByNameAtLayer using the
        // patchCount splice.
        assert!(assembled().contains("edit_layer_changed"));
        assert!(assembled().contains("rebindAllForLayer"));
        assert!(assembled().contains("paramIdByNameAtLayer"));
        // Placeholder lives in bridge.js pre-splice — assembled() has already
        // replaced it, so check the raw bridge module.
        assert!(BRIDGE_JS.contains("__PATCH_COUNT__"));
        // The splice replaces the placeholder at render time.
        let html = build_faceplate_html();
        assert!(!html.contains("__PATCH_COUNT__"), "patchCount placeholder must be replaced");
        assert!(
            html.contains(&format!("patchCount: {}", vxn_app::PATCH_COUNT)),
            "patchCount splice value missing from rendered html",
        );
    }

    #[test]
    fn header_switch_primitive_wired() {
        // 0045: Chorus + Delay carry a header-switch in
        // `.panel-header-toggle-slot`; CSS provides the active palette.
        assert!(assembled().contains("makeHeaderSwitch"));
        assert!(assembled().contains(".panel-header-switch"));
        assert!(assembled().contains(".panel-header-switch.active"));
    }

    #[test]
    fn edit_layer_changed_serializes() {
        // The web crate's view_event_to_json must encode the new variant
        // for the JS dispatcher to ever see it.
        let s = view_event_to_json(
            &vxn_app::Vxn1ViewCustom::EditLayerChanged { layer: Layer::Lower }.into_event(),
        );
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["kind"], "edit_layer_changed");
        assert_eq!(v["layer"], "lower");
    }

    #[test]
    fn split_point_changed_serializes() {
        // 0053: HTML Keys panel needs the controller's split-point
        // re-broadcast (preset / state-load / EditorReady) to reseed its
        // slider, since the page has no idle-poll loop the vizia editor
        // uses to read `SharedParams::split_point()` directly.
        let s = view_event_to_json(
            &vxn_app::Vxn1ViewCustom::SplitPointChanged { note: 72 }.into_event(),
        );
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["kind"], "split_point_changed");
        assert_eq!(v["note"], 72);
    }

    #[test]
    fn keys_panel_wired() {
        // 0053: Keys panel — mode/edit toggles, split slider with
        // C0..C7 range, note-name readout, Reset button. UiEvent posts:
        //   - set_key_mode (Whole / Dual / Split row)
        //   - set_edit_layer (Upper / Lower row, hidden in Whole)
        //   - set_split_point (slider, visible only in Split)
        //   - reset_layer (Reset button — both layers in Whole, the
        //     active layer otherwise)
        // ViewEvent dispatch:
        //   - key_mode_changed → keysPanel.setMode
        //   - edit_layer_changed → keysPanel.setLayer (in addition to
        //     the per-patch rebind)
        //   - split_point_changed → keysPanel.setSplit
        assert!(assembled().contains("const keysPanel = "));
        assert!(assembled().contains("send.setKeyMode("));
        assert!(assembled().contains("send.setEditLayer("));
        assert!(assembled().contains("send.setSplitPoint("));
        assert!(assembled().contains("send.resetLayer("));
        assert!(assembled().contains("ev.kind === 'key_mode_changed'"));
        assert!(assembled().contains("ev.kind === 'split_point_changed'"));
        assert!(assembled().contains("keysPanel.setMode"));
        assert!(assembled().contains("keysPanel.setLayer"));
        assert!(assembled().contains("keysPanel.setSplit"));
        // Note-name readout: covers a C0..C7 span, matches the vizia
        // editor's `note_name`. The helper is now the shared `noteName`
        // (vxn-core-ui-web/assets/cutoff-tuned.js, 0140; formerly
        // `keysNoteName`), spliced ahead of panels.js.
        assert!(assembled().contains("function noteName("));
        assert!(assembled().contains("KEYS_SPLIT_MIN = 12"));
        assert!(assembled().contains("KEYS_SPLIT_MAX = 96"));
        // Default split: matches DEFAULT_SPLIT_POINT (C4) so a
        // double-click reset lands on the same plain value the vizia
        // editor's `on_double_click` posts.
        assert_eq!(vxn_app::DEFAULT_SPLIT_POINT, 60);
        assert!(assembled().contains("KEYS_DEFAULT_SPLIT = 60"));
        // The slot reserved by 0040 now carries real markup, not a
        // bare placeholder. The vizia overlay note is gone.
        assert!(assembled().contains("data-name=\"Keys\""));
        assert!(!assembled().contains("still rendered by vizia"));
    }

    #[test]
    fn filter_mode_notch_dims_slope_strip() {
        // 0043: Filter Mode = Notch dims the Slope strip cell (DSP no-op,
        // see vxn-dsp/src/ota_ladder.rs). Test guards the wiring rather
        // than the runtime toggle:
        //   - CSS targets both `.ctl.dimmed` and `.ctl-strip.dimmed` (slope
        //     lives in the strip).
        //   - JS resolves `filter_mode` + `filter_slope` and looks up the
        //     Notch variant by label (so a `FILTER_MODE_LABELS` reorder
        //     doesn't desync).
        //   - The dispatch branch keys on `FILTER_MODE_ID`.
        // Asserting on the assembled HTML keeps the test substring-based —
        // the existing Free-run dim has the same shape.
        assert!(
            assembled().contains(".ctl-strip.dimmed"),
            "missing strip dim selector (slope dim relies on it)",
        );
        assert!(assembled().contains("BUILTIN_DIM_SPECS"));
        assert!(assembled().contains("'filter-notch'"));
        assert!(assembled().contains("variantIdx('filter_mode', 'Notch'"));
        assert!(assembled().contains("data-param=\"filter_slope\""));
        assert!(assembled().contains("applyDimRulesFor("));
    }

    #[test]
    fn faceplate_has_subdivisions_json_placeholder() {
        // SUBDIVISIONS table is spliced as a JSON array of labels; the LFO
        // rate fader's displayOverride indexes it when sync is on (0042).
        assert!(BRIDGE_JS.contains("__SUBDIVISIONS_JSON__"));
        let html = build_faceplate_html();
        assert!(!html.contains("__SUBDIVISIONS_JSON__"));
        // Sanity check: array matches the Rust table 1:1.
        let json = build_subdivisions_json();
        let v: serde_json::Value = serde_json::from_str(&json).expect("subdivisions JSON");
        let arr = v.as_array().expect("array root");
        assert_eq!(arr.len(), vxn_app::sync::SUBDIVISIONS.len());
        for (i, s) in vxn_app::sync::SUBDIVISIONS.iter().enumerate() {
            assert_eq!(arr[i], s.label);
        }
    }

    #[test]
    fn faceplate_has_params_json_placeholder() {
        // The template carries `__PARAMS_JSON__` for runtime descriptor
        // injection; build_faceplate_html() splices it.
        assert!(BRIDGE_JS.contains("__PARAMS_JSON__"));
        let html = build_faceplate_html();
        assert!(!html.contains("__PARAMS_JSON__"), "placeholder must be replaced");
        // Page references the bridge property; sanity check the rendered HTML
        // still contains the field literal.
        assert!(html.contains("params:"));
        // Splice the params JSON directly and prove its shape.
        let json = build_params_json();
        let v: serde_json::Value = serde_json::from_str(&json).expect("descriptor JSON");
        // Upper Osc1Wave is CLAP id 0.
        assert_eq!(v["0"]["name"], "osc1_wave");
        assert_eq!(v["0"]["kind"], "enum");
        assert_eq!(v["0"]["variants"][0], "Sine");
    }

    #[test]
    fn web_page_splices_clean_and_wires_boot() {
        // E018 / 0057: the standalone-web page reuses the native splice but
        // swaps the wry IPC head for the web boot head + module loader.
        let page = build_web_faceplate_html();
        // No unreplaced placeholders leak (params/subdivisions/patchCount + the
        // JS/CSS markers must all be spliced — including inside the boot head).
        for ph in [
            "__PARAMS_JSON__",
            "__SUBDIVISIONS_JSON__",
            "__PATCH_COUNT__",
            "__BRIDGE_JS__",
            "__DISPATCH_JS__",
            "__PANELS_JS__",
            "__BROWSER_JS__",
            "__CSS__",
        ] {
            assert!(!page.contains(ph), "web page leaks placeholder {ph}");
        }
        // The web boot head (queuing `window.ipc` stub) and the module loader
        // are both present.
        assert!(page.contains("__VXN_UI_QUEUE__"), "boot head queue missing");
        assert!(
            page.contains(r#"<script type="module" src="./faceplate-bridge.mjs">"#),
            "faceplate-bridge module loader missing",
        );
        // The DOM text-input popup CSS (0061) is injected.
        assert!(page.contains("vxn-ti-backdrop"), "text-input popup CSS missing");
        // The faceplate markup itself is intact (the mount root the bridge keys
        // off + a known control mount).
        assert!(page.contains(r#"id="faceplate""#));
        assert!(page.contains(r#"data-param="cutoff""#));
    }

    #[test]
    fn web_page_params_are_byte_identical_to_native() {
        // E018: the whole point of the reuse — the web page's descriptor JSON
        // must be the exact same `build_params_json` output the plugin splices.
        // Both pages carry it as `params:<json>` inside bridge.js; extract and
        // compare the literal so a future divergence in the web path is caught.
        let json = build_params_json();
        let native = build_faceplate_html();
        let web = build_web_faceplate_html();
        assert!(native.contains(&json), "native page must carry params JSON");
        assert!(web.contains(&json), "web page must carry the SAME params JSON");
    }

    #[test]
    fn descriptor_json_covers_every_kind() {
        // Walk every descriptor and confirm `kind` is one of the four expected
        // discriminants. Catches a future ParamKind variant slipping through
        // without a JSON-side handler.
        let v: serde_json::Value = serde_json::from_str(&build_params_json()).expect("params JSON");
        let obj = v.as_object().expect("object root");
        let mut seen_float = false;
        let mut seen_int = false;
        let mut seen_bool = false;
        let mut seen_enum = false;
        for (_id, desc) in obj {
            let kind = desc["kind"].as_str().unwrap_or("");
            assert!(
                matches!(kind, "float" | "int" | "bool" | "enum"),
                "unknown kind \"{kind}\" in {desc}",
            );
            match kind {
                "float" => seen_float = true,
                "int" => seen_int = true,
                "bool" => seen_bool = true,
                "enum" => seen_enum = true,
                _ => {}
            }
        }
        assert!(seen_float && seen_int && seen_bool && seen_enum);
    }

    #[test]
    fn faceplate_browser_drag_drop_wired() {
        // 0052: HTML5 DnD. Drag source = user-side preset rows (folder
        // view + search view). Drop target = user folder rows (left
        // pane). Factory rows have no DnD listeners on either side —
        // gated by `selectedFolder.kind === 'user'` /
        // `key.kind === 'user'`.
        //
        // Wire shape:
        //   - `row.draggable = true` + `dragstart` sets
        //     `dataTransfer.setData('vxn/preset', path)` (custom MIME
        //     guards against external dropzones receiving a preset path)
        //     plus module-level `dragSourcePath` / `dragSourceFolder`
        //     (read during `dragover` because `dataTransfer.getData` is
        //     not callable then).
        //   - Drop target preventDefaults `dragover` only when source is
        //     a vxn preset AND the target is not the source folder; the
        //     source folder shows `.drag-blocked` instead.
        //   - Drop posts `op: 'move_preset'` with the destination
        //     folder name (or null for the virtual user root).
        // The Move-to ▸ submenu (0051) shares the `move_preset` op
        // string, so this test additionally asserts the DnD-specific
        // bridge surface (drag listeners, MIME, drop CSS).
        assert!(assembled().contains("'vxn/preset'"));
        assert!(assembled().contains("wirePresetDragSource"));
        assert!(assembled().contains("'dragstart'"));
        assert!(assembled().contains("'dragover'"));
        assert!(assembled().contains("'dragleave'"));
        assert!(assembled().contains("'dragend'"));
        // The drop handler shares the `move_preset` op with the Move-to
        // submenu; the DnD-specific path passes `dragSourcePath` rather
        // than the menu's `target.path`. The `dragSourcePath` identifier
        // is used by both the dragstart write and the drop read — its
        // mere presence proves the bridge is wired through.
        assert!(assembled().contains("send.movePreset(dragSourcePath"));
        // Drop-target gating: factory rows must not get listeners. The
        // `appendFolderRow` gate keys on `key.kind === 'user'`; assert
        // the source folder no-op branch is present (key.name ===
        // dragSourceFolder).
        assert!(assembled().contains("key.name === dragSourceFolder"));
        // CSS for drop-target highlight + source-folder block + drag-
        // source dimming. `.drag-over` is the live drop highlight;
        // `.drag-blocked` shows the source folder mid-drag.
        assert!(assembled().contains(".browser-row.drag-over"));
        assert!(assembled().contains(".browser-row.drag-blocked"));
        assert!(assembled().contains(".browser-row.dragging"));
        // Follow-path plumbing: PresetCorpusChanged carries an
        // Option<PathBuf>; non-null means reselect the folder and
        // scroll the moved row into view. Dispatcher branch + module
        // method both present.
        assert!(assembled().contains("ev.kind === 'preset_corpus_changed'"));
        assert!(assembled().contains("browserPanel.followPath"));
        assert!(assembled().contains("function followPath("));
        // Rendered rows tag themselves with `data-path` so followPath
        // can locate the moved row via a CSS attribute selector.
        assert!(assembled().contains("r.dataset.path = p.path"));
        assert!(assembled().contains("r.dataset.path = h.source.path"));
    }

    // ── JS suite gate (E015 / 0078) ─────────────────────────────────────
    //
    // The Vitest + jsdom suite under `assets/__tests__/` is the
    // behavioural net for the four faceplate JS modules. We shell `npm
    // test` from a `#[test]` so `cargo test -p vxn-ui-web` is still the
    // single command a contributor runs locally. The env-gate keeps the
    // default `cargo test` Rust-only (no Node dep) — set `VXN_JS_TESTS=1`
    // to opt in. CI (when one lands) sets the var so the gate is real.
    #[test]
    fn js_suite_passes() {
        if std::env::var("VXN_JS_TESTS").is_err() {
            // No-op skip rather than `#[ignore]`: a build-script `cfg`
            // would work, but a runtime check keeps the gate one place
            // and matches the ticket-spec'd alternative.
            eprintln!(
                "VXN_JS_TESTS unset; skipping JS suite. \
                 Run `VXN_JS_TESTS=1 cargo test -p vxn-ui-web` to enable."
            );
            return;
        }
        let status = std::process::Command::new("npm")
            .args(["test", "--silent"])
            .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/assets"))
            .status()
            .expect("npm not found — install Node 20+ or unset VXN_JS_TESTS");
        assert!(status.success(), "JS suite failed under `npm test`");
    }
}
