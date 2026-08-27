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
//! What stays here is the faceplate asset splice, the param-descriptor JSON
//! builder, and VXN1b's two custom-payload hooks: [`parse_custom_op`] (the
//! opcodes the page posts that aren't params — key mode, split point, LFO 2
//! link, matrix topology, the layer copy, the scope tap) and
//! [`serialise_custom_payload`] (the view-bound customs — matrix + keyboard
//! echoes, meter frames, scope frames). Both are free functions rather than
//! closures so the wire shape is testable without a WebView.

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
/// 26 tab-strip + 3×8 chrome gaps + (124 + 124 + 164 rows + 2×8 pane gaps) = 554
/// nominal, **556 as laid out**: the banner and the tab strip carry 1 px borders
/// outside their declared heights, and the host window has to fit the page as
/// rendered, not as summed. At 554 the last 2 px — the bottom row's panel border
/// — were cut off.
///
/// The FX/Global pane is two rows (mixer + FX), and since its first row grew to
/// hold the whole mixer strip it comes to 424 against the Layer pane's 428 — so
/// it fills the window without driving its height.
///
/// Width was widened from the initial 760 for top-row breathing room + the
/// standalone Dynamics panel. Keep in sync with `--editor-w` / the row heights.
pub const EDITOR_WIDTH: u32 = 1060;
pub const EDITOR_HEIGHT: u32 = 616;

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
    config.parse_custom_ui = Some(std::sync::Arc::new(parse_custom_op));
    config.serialise_custom_view = Some(std::sync::Arc::new(serialise_custom_payload));
    vxn_core_ui_web::open_editor(parent, ctrl, corpus, config)
}

/// Parse a VXN1b-specific opcode the shared vocabulary doesn't cover.
///
/// Two-layer key-mode opcodes (0219): the faceplate posts `set_key_mode`
/// (Layer 2 toggle) / `set_split_point` as non-automatable custom ops, parsed
/// into a `KeyOp` payload the clap controller applies to the shared KeyState.
/// Matrix topology, the bulk layer copy and the scope tap ride the same hook.
///
/// A free function rather than a closure inside [`open_editor`] so the wire
/// shape is testable without a WebView.
fn parse_custom_op(op: &str, v: &serde_json::Value) -> Option<UiEvent> {
    {
        use vxn1b_engine::{KeyOp, Layer, MatrixEdit, MatrixField, PatchOp, ScopeOp, ScopeTap};
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
            // Bulk patch duplication (0265). A `PatchOp`, not a `KeyOp`: this
            // rewrites params and topology rather than KeyState.
            "copy_layer" => {
                let side = |k: &str| -> Option<Layer> {
                    Some(match v.get(k)?.as_str()? {
                        "lower" => Layer::L2,
                        "upper" => Layer::L1,
                        _ => return None,
                    })
                };
                Some(UiEvent::Custom(Box::new(PatchOp::CopyLayer {
                    from: side("from")?,
                    to: side("to")?,
                })))
            }
            // Reset one layer to the factory patch (0307). Like `copy_layer` a
            // `PatchOp`; unlike it, the mixer strip resets too.
            "reset_layer" => {
                let layer = match v.get("layer")?.as_str()? {
                    "lower" => Layer::L2,
                    "upper" => Layer::L1,
                    _ => return None,
                };
                Some(UiEvent::Custom(Box::new(PatchOp::ResetLayer { layer })))
            }
            // Oscilloscope tap select. Pure view state — which layer's trace is
            // on screen — so it rides a custom op and never reaches the patch.
            // `off` is what the page sends when the scope is not showing.
            "set_scope_source" => {
                let tap = match v.get("source")?.as_str()? {
                    "upper" => ScopeTap::Layer1,
                    "lower" => ScopeTap::Layer2,
                    "off" => ScopeTap::Off,
                    _ => return None,
                };
                Some(UiEvent::Custom(Box::new(ScopeOp::SetTap(tap))))
            }
            _ => None,
        }
    }
}

/// Serialise a VXN1b-specific `ViewEvent::Custom` payload for the page.
///
/// The view-bound customs are the matrix + keyboard echoes and the two
/// telemetry frames (meters, scope). They ride the normal per-tick ViewEvent
/// batch — one `evaluate_script`, no separate bridge channel — and carry raw
/// values; the dB mapping, the ballistics and the trigger search all live in
/// the page (`panels/meter.js`, `panels/scope.js`).
///
/// Arrays rather than named l/r keys: the page indexes `[0]`/`[1]`, and the
/// frames ship tens of times a second, so the terser shape is worth it on the
/// wire. Extracted from [`open_editor`] for the same reason as
/// [`parse_custom_op`].
fn serialise_custom_payload(payload: &dyn std::any::Any) -> Option<serde_json::Value> {
    {
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
        // Oscilloscope trace: one window of the selected layer's output, oldest
        // → newest. Rounded to 3 dp on the way out — the canvas is ~120 px
        // tall, so anything finer is invisible, and full f32 printing would
        // triple the length of a 384-number array shipped 30×/s.
        if let Some(s) = payload.downcast_ref::<vxn1b_engine::ScopeFrame>() {
            let samples: Vec<f64> = s
                .samples
                .iter()
                // Clamped past the rails: the trace clips there anyway, and it
                // keeps one runaway sample from bloating the frame.
                .map(|&v| ((v.clamp(-2.0, 2.0) as f64) * 1000.0).round() / 1000.0)
                .collect();
            return Some(serde_json::json!({ "kind": "scope", "s": samples }));
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
    }
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
        .replace("__DEFAULT_BPM__", &vxn1b_engine::sync::DEFAULT_TEMPO_BPM.to_string())
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

/* Tempo control — WEB ONLY. The plugin takes BPM from its host transport;
   a browser has no host, and `sync.rs` resolves every LFO and delay
   subdivision against a tempo, so without this the synced rates are stuck
   at the default forever.
   Lives in the bottom-left web-chrome row beside the CPU meter, not on the
   faceplate: both are host-shaped readouts about the browser rather than
   parts of the instrument, so they belong together and away from the panels.
   Position comes from the row; `order:2` puts it right of the CPU badge
   whichever mounts first. */
.vxn-bpm {
  order: 2;
  display: flex; align-items: center; gap: 5px;
  font: 11px/1 system-ui, sans-serif; letter-spacing: 0.04em;
  color: #cfd3d8;
  background: rgba(20, 22, 26, 0.78);
  padding: 4px 7px; border-radius: 5px;
  user-select: none;
}
.vxn-bpm > span { opacity: 0.7; }
.vxn-bpm input {
  width: 44px; padding: 2px 4px;
  font: 11px/1 system-ui, sans-serif; text-align: right;
  background: #0e0e10; color: #d8d8de;
  border: 1px solid #33333a; border-radius: 3px;
}
.vxn-bpm input:focus { outline: none; border-color: #4a90d9; }
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
  window.__VXN_DEFAULT_BPM__ = __DEFAULT_BPM__;

  // Tempo control (E045 delta 5). Web only, for the reason in the CSS above.
  // Posts through the same `window.ipc` as every other opcode, so it queues
  // before the bridge is up and routes to the ring afterwards — tempo has no
  // model presence, so there is no echo that could carry it.
  document.addEventListener("DOMContentLoaded", function () {
    // The shared bottom-left chrome row (see cpu-meter.mjs). Created here if the
    // CPU meter has not mounted yet — it boots asynchronously and this runs on
    // DOMContentLoaded, so either can be first. Same id, same styles, both
    // idempotent; `order` decides the layout rather than mount order.
    var row = document.getElementById("vxn-web-chrome");
    if (!row) {
      row = document.createElement("div");
      row.id = "vxn-web-chrome";
      row.style.cssText =
        "position:fixed;left:10px;bottom:102px;z-index:9999;" +
        "display:flex;align-items:center;gap:8px;";
      document.body.appendChild(row);
    }
    var wrap = document.createElement("div");
    wrap.className = "vxn-bpm";
    var label = document.createElement("span");
    label.textContent = "BPM";
    var input = document.createElement("input");
    input.type = "number";
    input.min = "20";
    input.max = "300";
    input.step = "1";
    input.value = String(window.__VXN_DEFAULT_BPM__);
    var send = function () {
      var bpm = Number(input.value);
      // Clamp rather than trust: a non-finite or absurd tempo would divide
      // through every synced rate in the engine.
      if (!isFinite(bpm)) return;
      bpm = Math.min(300, Math.max(20, bpm));
      input.value = String(bpm);
      window.ipc.postMessage(JSON.stringify({ op: "set_tempo", bpm: bpm }));
    };
    input.addEventListener("change", send);
    // Typing a tempo must not trigger the faceplate's single-key shortcuts.
    input.addEventListener("keydown", function (ev) { ev.stopPropagation(); });
    wrap.appendChild(label);
    wrap.appendChild(input);
    row.appendChild(wrap);
    // Seed the engine so a synced LFO is right before anyone touches this.
    send();
  });
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

/// Tempo-sync subdivision labels, spliced into the page as
/// `window.vxn.subdivisions` (0267). A synced LFO-rate / delay-time fader reads
/// its popup label out of this table by fader position, so it must stay the
/// same table and the same order the engine resolves against
/// (`vxn1b_engine::sync`).
fn build_subdivisions_json() -> String {
    let labels: Vec<String> = vxn1b_engine::sync::SUBDIVISIONS
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
/// modules; `preset-bar.js` runs a load-time IIFE so it sits last.
const PANEL_UTIL_DRAG_JS: &str = include_str!("../assets/util/drag.js");
const PANEL_FADER_JS: &str = include_str!("../assets/panels/fader.js");
const PANEL_DISCRETE_JS: &str = include_str!("../assets/panels/discrete.js");
const PANEL_PRESET_BAR_JS: &str = include_str!("../assets/panels/preset-bar.js");
const PANEL_MATRIX_JS: &str = include_str!("../assets/panels/matrix.js");
const PANEL_METER_JS: &str = include_str!("../assets/panels/meter.js");
const PANEL_SCOPE_JS: &str = include_str!("../assets/panels/scope.js");
/// The split panel source files, in splice order.
const PANELS_FILES: &[&str] = &[
    PANEL_UTIL_DRAG_JS,
    PANEL_FADER_JS,
    PANEL_DISCRETE_JS,
    PANEL_PRESET_BAR_JS,
    PANEL_MATRIX_JS,
    // Meters (0240) — must precede dispatch.js, which calls `makeMeter` /
    // `meterRegistry` from `wireMeters`. The splice is order-sensitive: these
    // files share one concatenated scope with no module resolution.
    PANEL_METER_JS,
    // Scope — same ordering constraint as the meters: dispatch.js calls
    // `makeScope` from `wireScope`.
    PANEL_SCOPE_JS,
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
            // Rocker (Voice mode, NoiseColor, FilterSlope, Env Shape) + the
            // stacked-cell columns that carry it / the under-fader toggles.
            ".ctl-rocker",
            ".ctl-rocker-body",
            ".ctl-rocker-track",
            ".ctl-rocker-knob",
            ".ctl-rocker-lbl",
            ".ctl-col",
            ".ctl-col-center",
            // Multi-column button groups (Voice's six widths).
            ".ctl-buttongroup[data-columns] .ctl-tg-rows",
            // Layer scope.
            ".scope-mount",
            ".scope-canvas",
            // Confirmation modal (Copy L1 → L2).
            ".confirm-panel",
            ".confirm-message",
            ".confirm-actions",
            // Faders, dials, wave knobs, dropdowns.
            ".ctl-fader",
            ".ctl-fader-track",
            ".ctl-fader-thumb",
            ".ctl-label",
            ".dial-grid",
            // Stacked-column + state classes. `.dimmed` is only ever compounded
            // onto a cell, so assert the real head rather than a bare class.
            ".ctl-col",
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

    /// The scope tap is view state, so it must reach the ring as a `ScopeOp`
    /// and never as a param write or a `KeyOp` (which would land it in the
    /// patch, the state blob and the host's undo stack).
    #[test]
    fn scope_source_opcode_parses_to_a_tap() {
        use vxn1b_engine::{ScopeOp, ScopeTap};
        let parse = |src: &str| {
            let v = serde_json::json!({ "source": src });
            match parse_custom_op("set_scope_source", &v) {
                Some(UiEvent::Custom(payload)) => payload.downcast::<ScopeOp>().ok().map(|b| *b),
                _ => None,
            }
        };
        assert_eq!(parse("upper"), Some(ScopeOp::SetTap(ScopeTap::Layer1)));
        assert_eq!(parse("lower"), Some(ScopeOp::SetTap(ScopeTap::Layer2)));
        assert_eq!(parse("off"), Some(ScopeOp::SetTap(ScopeTap::Off)));
        // A malformed source is dropped rather than defaulting to a tap — the
        // audio thread would otherwise start capturing on a typo.
        assert!(parse("sideways").is_none());
        assert!(parse_custom_op("set_scope_source", &serde_json::json!({})).is_none());
    }

    /// The trace's wire shape: `{kind:'scope', s:[…]}`, oldest → newest, 3 dp.
    /// The rounding is what keeps a 384-sample frame at ~30 Hz cheap, and the
    /// page's `ev.s` read depends on the key.
    #[test]
    fn scope_frame_serialises_rounded_samples() {
        let frame = vxn1b_engine::ScopeFrame {
            samples: vec![0.0, 0.123_456_7, -0.5, 3.0, -9.0],
        };
        let v = serialise_custom_payload(&frame).expect("scope frame serialises");
        assert_eq!(v["kind"], "scope");
        let s = v["s"].as_array().expect("sample array");
        assert_eq!(s.len(), 5);
        assert_eq!(s[1].as_f64(), Some(0.123), "3 dp keeps the frame small");
        assert_eq!(s[2].as_f64(), Some(-0.5));
        // Clamped past the rails — the trace clips there anyway.
        assert_eq!(s[3].as_f64(), Some(2.0));
        assert_eq!(s[4].as_f64(), Some(-2.0));
        // Printed short, not as a 17-digit f32→f64 expansion.
        assert!(
            !v.to_string().contains("0.12345"),
            "rounded samples must serialise short: {v}"
        );
    }

    /// The scope shares the serialise hook with the meter frame; a payload
    /// reaching the wrong arm would silently redraw one as the other.
    #[test]
    fn the_custom_payloads_keep_their_own_kinds() {
        let meters = serialise_custom_payload(&vxn1b_engine::MeterFrame::default())
            .expect("meter frame serialises");
        assert_eq!(meters["kind"], "meters");
        let scope = serialise_custom_payload(&vxn1b_engine::ScopeFrame { samples: vec![0.0; 4] })
            .expect("scope frame serialises");
        assert_eq!(scope["kind"], "scope");
        // An unrecognised payload is skipped, not mis-serialised.
        assert!(serialise_custom_payload(&42u8).is_none());
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

    // ── JS suite gate (0321) ────────────────────────────────────────────
    //
    // The Vitest + jsdom suite under `assets/__tests__/` is the behavioural
    // net for the faceplate JS. Shelling `npm test` from a `#[test]` keeps
    // `cargo test -p vxn1b-ui-web` the single command a contributor runs.
    // The env-gate keeps a default `cargo test` Rust-only (no Node dep); CI
    // sets the var, so the gate is real there.
    //
    // Mirrors `vxn-ui-web`'s gate deliberately — same var, same shape, so
    // one CI env setting un-gates both.
    /// Every opcode the page can post must reach a handler.
    ///
    /// This is the guard 0307 asked for. Three dead opcodes survived the fork
    /// from vxn-1 — `reset_layer` among them, wired to a visible RESET button
    /// that silently did nothing — because nothing checked that the sender
    /// surface and the parser agree. The senders live in `bridge.js` as
    /// `op: '<name>'` literals, so they can be read straight out of the spliced
    /// source and driven through the real `parse_ui_event`.
    ///
    /// A payload carrying every field any opcode wants is passed to all of
    /// them; each arm takes what it needs via `v.get(..)?` and ignores the
    /// rest. That keeps the test one table instead of one fixture per opcode.
    #[test]
    fn every_opcode_the_page_posts_has_a_handler() {
        // Handled in the page and deliberately never sent onward. Adding to
        // this list is a decision; forgetting to handle an opcode is not.
        const IN_PAGE_ONLY: [&str; 1] = [
            // The faceplate rebinds its own cells on a layer flip; nothing
            // downstream needs the news.
            "set_edit_layer",
        ];

        let mut posted: Vec<&str> = Vec::new();
        let mut rest = BRIDGE_JS;
        while let Some(i) = rest.find("op: '") {
            rest = &rest[i + "op: '".len()..];
            let end = rest.find('\'').expect("unterminated op literal in bridge.js");
            posted.push(&rest[..end]);
            rest = &rest[end..];
        }
        posted.sort_unstable();
        posted.dedup();
        assert!(
            posted.len() > 15,
            "scanned only {} opcodes out of bridge.js — the scan broke, not the senders",
            posted.len()
        );

        // Superset payload: every field name any opcode reads, at the type it
        // expects. Arms take what they need and ignore the rest.
        //
        // Two shapes, because `id` and `value` are polymorphic across the
        // vocabulary — `set_param` wants a numeric id, `request_text_input` a
        // string one. An opcode passes if EITHER shape parses, which is the
        // property worth asserting: some well-formed payload reaches a handler.
        let fat = serde_json::json!({
            "id": 0, "plain": 1.0, "norm": 0.5,
            "layer": "upper", "from": "upper", "to": "lower",
            "mode": 1, "note": 60, "on": true,
            "slot": 0, "field": "source", "value": 1,
            "source": "upper",
            "index": 0, "delta": 1,
            "path": "a.toml", "new_name": "b", "old_name": "a",
            "dest_folder": "f", "name": "n", "suggested": "s", "folder": "f",
            "title": "t", "initial": "i", "text": "x",
        });
        let mut stringy = fat.clone();
        stringy["id"] = serde_json::json!("prompt-id");
        stringy["value"] = serde_json::json!("text");

        let custom: vxn_core_ui_web::ParseCustomUi = std::sync::Arc::new(parse_custom_op);

        for op in posted {
            if IN_PAGE_ONLY.contains(&op) {
                continue;
            }
            let parses = [&fat, &stringy].iter().any(|shape| {
                let mut one = (*shape).clone();
                one["op"] = serde_json::Value::String(op.to_string());
                let body = serde_json::to_string(&one).unwrap();
                vxn_core_ui_web::parse_ui_event(&body, Some(&custom)).is_some()
            });
            assert!(
                parses,
                "bridge.js posts `{op}` and nothing parses it — either handle it, \
                 or add it to IN_PAGE_ONLY with a reason"
            );
        }
    }

    #[test]
    fn js_suite_passes() {
        if std::env::var("VXN_JS_TESTS").is_err() {
            eprintln!(
                "VXN_JS_TESTS unset; skipping JS suite. \
                 Run `VXN_JS_TESTS=1 cargo test -p vxn1b-ui-web` to enable."
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
