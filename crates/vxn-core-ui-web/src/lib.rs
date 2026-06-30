//! Shared wry WebView editor backend.
//!
//! Provides the JS↔Rust IPC bridge, the batched `evaluate_script` view-event
//! sink, the corpus-snapshot JSON helper, and the native text-input popup.
//! HTML / CSS / JS assets and per-synth opcodes / view events are NOT here —
//! each synth bundles its own faceplate and supplies a [`WebEditorConfig`]
//! with the HTML string plus optional callbacks for `UiEvent::Custom` parse
//! and `ViewEvent::Custom` serialise.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Arc;

use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, WindowHandle as RwhWindowHandle,
};
use vxn_core_app::{
    ControllerHandle, CorpusHandle, EditorBackend, ParamDesc, ParamId, ParamKind,
    PresetSource, Taper, UiEvent, ViewEvent,
};
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::{Rect, WebView, WebViewBuilder};

mod text_input;
pub use text_input::prompt_text;

/// Max bytes per `evaluate_script` payload (default). The JSON-array
/// literal interpolated into the JS source is bounded here; under heavy
/// automation the batch is split across multiple calls so wry never sees
/// a giant string. 100 KB is a sane cap.
pub const DEFAULT_MAX_BATCH_BYTES: usize = 100_000;

/// Shared preset browser ES module (two-pane folders/presets panel,
/// search, context menu, drag-and-drop, modals, follow-path). Synth-
/// agnostic: each synth splices this (after [`strip_esm_exports`]) into
/// its inline `<script>`, then a tiny glue calls `createPresetBrowser(cfg)`
/// with its own bridge adapter. The ESM `export` markers exist so the
/// Node/vitest suite can `import` the pure helpers + the factory; they are
/// stripped at splice time (module syntax is illegal in an inline script).
pub const PRESET_BROWSER_JS: &str = include_str!("../assets/preset-browser.js");

/// Stylesheet for the shared preset browser. Spliced into each synth's
/// faceplate `<style>`; uses CSS custom properties (`--panel-bg`,
/// `--ctl-value`, `--ctl-label`, `--panel-corner`, `--glyph-active`,
/// `--text`, `--pad-outer`, `--banner-h`, `--preset-bar-h`, `--row-vgap`,
/// `--editor-w`) that both synths define.
pub const PRESET_BROWSER_CSS: &str = include_str!("../assets/preset-browser.css");

/// Shared faceplate widget primitives (0140), each an ESM-authored asset
/// both synths splice (after [`strip_esm_exports`]) ahead of their own
/// panels so the stripped top-level bindings — `valuePop`, `wireDrag`,
/// `noteName` / `midiToHz` / `cutoffTuned*` / `CUTOFF_TUNED_MIDI_*` — are in
/// the shared inline-script scope before any consumer references them.
/// Splice ORDER matters: these must precede the panel modules (`const`
/// bindings don't hoist). The `export` markers exist for the Node/vitest
/// suites, which `import` the pure helpers directly.
pub const VALUE_POP_JS: &str = include_str!("../assets/value-pop.js");
pub const CUTOFF_TUNED_JS: &str = include_str!("../assets/cutoff-tuned.js");
pub const WIRE_DRAG_JS: &str = include_str!("../assets/wire-drag.js");

/// Stylesheet for the shared floating value popup. Appended to each synth's
/// faceplate `<style>` (like [`PRESET_BROWSER_CSS`]); the single
/// `.value-pop` ruleset replaces the per-synth copies.
pub const VALUE_POP_CSS: &str = include_str!("../assets/value-pop.css");

/// The shared widget primitives ([`VALUE_POP_JS`], [`CUTOFF_TUNED_JS`],
/// [`WIRE_DRAG_JS`]), ESM markers stripped and joined in dependency order,
/// ready to splice ahead of a synth's own panel modules. One owner of the
/// order so both faceplates can't drift on it.
pub fn shared_widgets_js() -> String {
    [VALUE_POP_JS, CUTOFF_TUNED_JS, WIRE_DRAG_JS]
        .map(strip_esm_exports)
        .join("\n;\n")
}

/// Drop ESM module syntax from `src` so an ESM-authored asset can be inlined
/// into a single `<script>` (where `export` / `import` are syntax errors).
/// The splice concatenates every module into one shared script scope, so
/// cross-module references resolve without the imports and re-exports.
///
/// ## Contract
///
/// Operates line-wise — a line's first non-whitespace token decides its
/// fate, so leading indentation no longer matters (an indented `export` /
/// `import` is handled the same as one at column 0). Dropped lines become
/// blank lines, not deletions, so line numbers — hence stack traces — stay
/// aligned. The forms handled, each covered by a unit test below:
///
/// - `export const/let/var/function/class/async … X = …` → the `export `
///   keyword is removed, leaving the bare declaration (indentation kept).
/// - `export default X` → `X`.
/// - `import …;` (side-effect, default, named, namespace) → dropped. A
///   multi-line import (`import {` with no terminating `;` on the opening
///   line) drops every line through the one carrying its `;`.
/// - `export { … }` (export-list), `export { … } from '…'` and
///   `export * from '…'` (re-exports) → dropped whole, single- or
///   multi-line; the named bindings already live in the shared scope.
///
/// Statement boundaries are found by the terminating `;`, so every `import`
/// / multi-line `export { … }` MUST be semicolon-terminated (the repo's
/// asset style always is). A line is classified by its leading token, so a
/// comment or string whose first token is literally `import`/`export ` would
/// be mis-stripped — same line-wise assumption the original had, just now
/// trim-aware.
///
/// Shared so both synth faceplate crates and any other ESM-authored shared
/// asset strip identically.
pub fn strip_esm_exports(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    // True while swallowing the continuation lines of a multi-line `import`
    // or `export { … }` statement, up to and including its terminating `;`.
    let mut in_multiline_drop = false;
    for (i, line) in src.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if in_multiline_drop {
            // Continuation of a dropped statement → emit a blank line; the
            // terminating `;` ends the swallow.
            if line.contains(';') {
                in_multiline_drop = false;
            }
            continue;
        }
        let rest = line.trim_start();
        let indent = &line[..line.len() - rest.len()];
        // `import …` (any form) and `export { … }` / `export * …` re-exports
        // are pure binding plumbing — the inline splice already shares one
        // scope, so drop them whole. Multi-line forms (no `;` on the opening
        // line) swallow their continuation until the `;`.
        let is_import = rest == "import" || rest.starts_with("import ") || rest.starts_with("import{");
        let is_reexport = rest.starts_with("export {")
            || rest.starts_with("export{")
            || rest.starts_with("export *")
            || rest.starts_with("export*");
        if is_import || is_reexport {
            if !line.contains(';') {
                in_multiline_drop = true;
            }
            continue;
        }
        // `export default X` → `X`; `export <decl>` → `<decl>`. Indentation
        // is re-attached so an indented declaration keeps its shape.
        let stripped = rest
            .strip_prefix("export default ")
            .or_else(|| rest.strip_prefix("export "))
            .unwrap_or(rest);
        if std::ptr::eq(stripped, rest) {
            // No prefix removed — emit the original line verbatim (keeps any
            // leading whitespace exactly).
            out.push_str(line);
        } else {
            out.push_str(indent);
            out.push_str(stripped);
        }
    }
    if src.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// User-supplied parse hook for `UiEvent::Custom` payloads. Called from
/// the WebView IPC handler when [`parse_ui_event_default`] returns
/// `None`. The hook receives the already-parsed JSON value and the
/// opcode string; returns `Some(UiEvent::Custom(...))` if it recognises
/// the opcode, `None` to log-and-drop.
pub type ParseCustomUi =
    Arc<dyn Fn(&str, &serde_json::Value) -> Option<UiEvent> + Send + Sync>;

/// User-supplied serialise hook for `ViewEvent::Custom` payloads.
/// Called from the chunk builder when serialising a `Custom` event.
/// Returns the JSON value the page sees, or `None` to skip.
pub type SerialiseCustomView =
    Arc<dyn Fn(&dyn Any) -> Option<serde_json::Value> + Send + Sync>;

/// Configuration for [`open_editor`].
pub struct WebEditorConfig {
    /// Final faceplate HTML (each synth bundles + splices its own CSS /
    /// JS modules / params JSON; the shared crate sees only the result).
    pub html: String,
    /// Logical pixel dimensions of the editor.
    pub width: u32,
    pub height: u32,
    /// Display label for the virtual root group of the user preset corpus.
    pub uncategorised_label: &'static str,
    /// Max bytes per `evaluate_script` payload. Use
    /// [`DEFAULT_MAX_BATCH_BYTES`] unless you have a reason.
    pub max_batch_bytes: usize,
    /// Vendor / product slugs for the Windows WebView2 user-data folder
    /// override (avoids `C:\Program Files\<host>\<exe>.WebView2` which
    /// is admin-only). Path is
    /// `%LOCALAPPDATA%\<vendor>\<product>\WebView2`. Skipped if either
    /// is `None`.
    pub webview2_vendor: Option<&'static str>,
    pub webview2_product: Option<&'static str>,
    /// Optional parser for per-synth opcodes that aren't in the shared
    /// vocabulary. Returns the matching [`UiEvent::Custom`] payload.
    pub parse_custom_ui: Option<ParseCustomUi>,
    /// Optional serialiser for [`ViewEvent::Custom`] payloads.
    pub serialise_custom_view: Option<SerialiseCustomView>,
}

impl WebEditorConfig {
    pub fn new(html: String, width: u32, height: u32) -> Self {
        Self {
            html,
            width,
            height,
            uncategorised_label: "Uncategorised",
            max_batch_bytes: DEFAULT_MAX_BATCH_BYTES,
            webview2_vendor: None,
            webview2_product: None,
            parse_custom_ui: None,
            serialise_custom_view: None,
        }
    }
}

/// Why [`open_editor`] failed. Returned instead of panicking — the call
/// sits on the CLAP `gui.set_parent` path, where an unwind would cross
/// the host's `extern "C"` frame (UB with `panic = "unwind"`). The clack
/// shell maps this into a `PluginError` (`impl std::error::Error` + the
/// blanket `From` make `?` work); the plugin instance stays alive, audio
/// keeps rendering, and the host may retry `set_parent` later.
#[derive(Debug)]
pub enum OpenEditorError {
    /// The host handed a null / zero native parent handle.
    BadParent(&'static str),
    /// wry failed to construct the WebView under the parent (missing
    /// WebView2 runtime, webkit2gtk init failure, …).
    WebViewBuild(wry::Error),
}

impl std::fmt::Display for OpenEditorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadParent(what) => write!(f, "invalid parent window handle: {what}"),
            Self::WebViewBuild(e) => write!(f, "WebView construction failed: {e}"),
        }
    }
}

impl std::error::Error for OpenEditorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::WebViewBuild(e) => Some(e),
            Self::BadParent(_) => None,
        }
    }
}

/// Redirect WebView2's user-data folder to a user-writable location.
/// Default is `<host_exe_dir>\<exe_name>.WebView2`, which inside
/// `C:\Program Files\<host>\` is admin-only and fails the WebView2 env
/// init with `E_ACCESSDENIED`. The WebView2 SDK honours
/// `WEBVIEW2_USER_DATA_FOLDER` if set before the environment is created,
/// so plant it once per process.
///
/// Idempotent: if the user (or another plugin instance) already set the
/// var we leave it alone.
#[cfg(target_os = "windows")]
fn ensure_webview2_data_dir(vendor: &str, product: &str) {
    const ENV: &str = "WEBVIEW2_USER_DATA_FOLDER";
    if std::env::var_os(ENV).is_some() {
        return;
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(vendor).join(product).join("WebView2");
    let _ = std::fs::create_dir_all(&dir);
    // SAFETY (0115): `set_var` is unsound if another thread reads the
    // process environment concurrently. This runs on the host's main
    // thread inside `gui.set_parent`, *before* the WebView2 environment
    // (and its worker threads) is created, which is the single-threaded-
    // at-init window the WebView2 SDK itself requires for this variable
    // to take effect. A host that reads env vars from another thread
    // during GUI creation could race — accepted: wry exposes no
    // per-environment user-data-folder argument, so the env var is the
    // only channel.
    unsafe { std::env::set_var(ENV, &dir) };
}

#[cfg(not(target_os = "windows"))]
#[inline]
fn ensure_webview2_data_dir(_vendor: &str, _product: &str) {}

/// Live editor. Dropping it tears down the WebView; on macOS wry removes
/// the subview from the parent NSView as part of that.
pub struct EditorHandle {
    webview: WebView,
    /// Per-tick batch buffer. The clack shell's `on_timer` calls
    /// [`Self::push_view_event`] once per event the controller produced
    /// this tick, then [`Self::flush_view_events`] once at the end —
    /// one `evaluate_script` per tick, not per event.
    buf: RefCell<Vec<ViewEvent>>,
    /// Raw native parent (NSView on macOS, HWND on Windows, xcb window
    /// id on Linux). Held for the editor's lifetime so the floating
    /// text-input popup can centre over the host plugin window without
    /// re-plumbing the parent through every ViewEvent.
    parent: *mut c_void,
    /// Controller post handle. The popup callback uses it to fire
    /// `UiEvent::TextInputResult` back when the user commits / cancels.
    ctrl: ControllerHandle,
    /// Shared preset corpus snapshot the controller refreshes on every
    /// disk-mutating preset op. Serialised + pushed to JS at first
    /// flush and on every `ViewEvent::PresetCorpusChanged` in the
    /// batch so the browser panel stays in sync without a
    /// controller→view payload channel for the full corpus.
    corpus: CorpusHandle,
    /// `false` until the first batch flush has carried a corpus
    /// snapshot.
    corpus_seeded: Cell<bool>,
    uncategorised_label: &'static str,
    max_batch_bytes: usize,
    serialise_custom: Option<SerialiseCustomView>,
}

impl EditorHandle {
    /// Buffer one [`ViewEvent`] for the current tick. Flushed by
    /// [`Self::flush_view_events`]; nothing crosses into JS until then.
    ///
    /// `ViewEvent::OpenTextInput` is intercepted here and dispatched to
    /// the native popup primitive — it never reaches the JS bridge. On
    /// commit / cancel the popup posts `UiEvent::TextInputResult`
    /// through the controller, which echoes `ViewEvent::TextInputResult`
    /// back into this buffer for the page's pending-callback map.
    pub fn push_view_event(&self, event: ViewEvent) {
        if let ViewEvent::OpenTextInput { id, title, initial } = event {
            self.open_text_input(id, title, initial);
            return;
        }
        self.buf.borrow_mut().push(event);
    }

    fn open_text_input(&self, id: String, title: String, initial: String) {
        let ctrl = self.ctrl.clone();
        text_input::prompt_text(self.parent, &title, &initial, move |value| {
            let _ = ctrl.post(UiEvent::TextInputResult { id, value });
        });
    }

    /// Drain the batch into one `__vxn.applyViewEvents` call (or
    /// several, if the JSON exceeds `max_batch_bytes`). `ParamChanged`
    /// events dedupe by id within the batch — only the latest value per
    /// param survives, capping the bridge at one update per param per
    /// tick regardless of how many automation writes the audio thread
    /// did between ticks.
    ///
    /// Corpus seeding: the preset corpus snapshot is sized like a few
    /// hundred metas and never deduped, so it ships as a separate
    /// `applyPresetCorpus` JS call rather than going through the
    /// `ViewEvent` batch. We push it once at first flush and once per
    /// flush that carries a `ViewEvent::PresetCorpusChanged`.
    pub fn flush_view_events(&self) {
        let events = std::mem::take(&mut *self.buf.borrow_mut());
        let needs_corpus = !self.corpus_seeded.get()
            || events
                .iter()
                .any(|e| matches!(e, ViewEvent::PresetCorpusChanged { .. }));
        if events.is_empty() && !needs_corpus {
            return;
        }
        if needs_corpus {
            if let Some(json) = self.serialize_corpus() {
                let js = format!(
                    "if(window.__vxn&&window.__vxn.applyPresetCorpus){{window.__vxn.applyPresetCorpus({json})}}"
                );
                let _ = self.webview.evaluate_script(&js);
                self.corpus_seeded.set(true);
            }
        }
        if events.is_empty() {
            return;
        }
        for chunk_json in batch_chunks(
            &events,
            self.max_batch_bytes,
            self.serialise_custom.as_ref(),
        ) {
            let js = format!(
                "if(window.__vxn&&window.__vxn.applyViewEvents){{window.__vxn.applyViewEvents({chunk_json})}}"
            );
            let _ = self.webview.evaluate_script(&js);
        }
    }

    fn serialize_corpus(&self) -> Option<String> {
        let corpus = self.corpus.lock().ok()?;
        Some(corpus_snapshot_json(&corpus, self.uncategorised_label))
    }

    /// Marker for shape parity with other backends — the clack shell
    /// calls this from `gui.destroy()`. wry's `WebView::Drop` already
    /// removes the subview from the parent NSView on macOS, so the
    /// real teardown happens when the host drops the handle.
    pub fn close(&mut self) {}
}

/// Zero-sized type that names this backend for trait-bounded code. All
/// state lives in [`EditorHandle`].
pub struct WebEditor;

impl EditorBackend for WebEditor {
    type Handle = EditorHandle;
    /// Raw native parent: NSView pointer on macOS, HWND on Windows,
    /// xcb window id on Linux. The clack shell extracts these per-
    /// platform in `gui::set_parent`.
    type ParentWindow = *mut c_void;

    fn open(
        _parent: Self::ParentWindow,
        _ctrl: ControllerHandle,
        _corpus: CorpusHandle,
    ) -> Result<Self::Handle, Box<dyn std::error::Error>> {
        // The shared trait surface has no room for a config payload, so
        // synth shells call [`open_editor`] directly with their config
        // rather than going through `WebEditor::open`. Erroring (not
        // panicking — 0115) keeps accidental use loud without an unwind.
        Err("vxn-core-ui-web::WebEditor::open: call open_editor(parent, ctrl, corpus, config) directly so the synth can supply its faceplate HTML + custom hooks".into())
    }

    fn close(handle: &mut Self::Handle) {
        handle.close();
    }

    fn push_view_event(handle: &Self::Handle, event: ViewEvent) {
        handle.push_view_event(event);
    }

    fn flush_view_events(handle: &Self::Handle) {
        handle.flush_view_events();
    }
}

/// Build the WebView under `parent`, wire the IPC handler to `ctrl`,
/// and load the supplied HTML. `parent` is the same raw pointer the
/// host hands the clack shell in `gui::set_parent` (NSView / HWND /
/// xcb-window-id).
///
/// Errors (never panics) on a null parent handle or a wry build
/// failure — see [`OpenEditorError`].
pub fn open_editor(
    parent: *mut c_void,
    ctrl: ControllerHandle,
    corpus: CorpusHandle,
    config: WebEditorConfig,
) -> Result<EditorHandle, OpenEditorError> {
    let WebEditorConfig {
        html,
        width,
        height,
        uncategorised_label,
        max_batch_bytes,
        webview2_vendor,
        webview2_product,
        parse_custom_ui,
        serialise_custom_view,
    } = config;
    let parent_raw = parent;
    let parent_wrap = ParentWindow { raw: build_raw(parent_raw)? };
    if let (Some(v), Some(p)) = (webview2_vendor, webview2_product) {
        ensure_webview2_data_dir(v, p);
    }
    let ipc_ctrl = ctrl.clone();
    let parse_custom = parse_custom_ui.clone();
    let webview = WebViewBuilder::new_as_child(&parent_wrap)
        .with_html(html)
        // macOS swallows the first click on an unfocused webview to activate
        // it; accept_first_mouse delivers that click to the control instead.
        .with_accept_first_mouse(true)
        .with_bounds(Rect {
            position: LogicalPosition::new(0i32, 0i32).into(),
            size: LogicalSize::new(width, height).into(),
        })
        .with_ipc_handler(move |req| {
            if let Some(ev) = parse_ui_event(req.body(), parse_custom.as_ref()) {
                let _ = ipc_ctrl.post(ev);
            }
        })
        .build()
        .map_err(OpenEditorError::WebViewBuild)?;
    Ok(EditorHandle {
        webview,
        buf: RefCell::new(Vec::new()),
        parent: parent_raw,
        ctrl,
        corpus,
        corpus_seeded: Cell::new(false),
        uncategorised_label,
        max_batch_bytes,
        serialise_custom: serialise_custom_view,
    })
}

// ── Parent-window adapter ───────────────────────────────────────────────────

struct ParentWindow {
    raw: RawWindowHandle,
}

// SAFETY: parent stays alive for the editor's lifetime; we hand it
// straight to wry on the same thread and never share it.
unsafe impl Send for ParentWindow {}
unsafe impl Sync for ParentWindow {}

impl HasWindowHandle for ParentWindow {
    fn window_handle(&self) -> Result<RwhWindowHandle<'_>, HandleError> {
        // SAFETY: `raw` was built from the host-provided native handle;
        // it stays valid as long as the host hasn't destroyed the GUI.
        Ok(unsafe { RwhWindowHandle::borrow_raw(self.raw) })
    }
}

#[cfg(target_os = "macos")]
fn build_raw(ptr: *mut c_void) -> Result<RawWindowHandle, OpenEditorError> {
    use raw_window_handle::AppKitWindowHandle;
    use std::ptr::NonNull;
    let ns_view =
        NonNull::new(ptr).ok_or(OpenEditorError::BadParent("parent NSView is null"))?;
    Ok(RawWindowHandle::AppKit(AppKitWindowHandle::new(ns_view)))
}

#[cfg(target_os = "windows")]
fn build_raw(ptr: *mut c_void) -> Result<RawWindowHandle, OpenEditorError> {
    use raw_window_handle::Win32WindowHandle;
    use std::num::NonZeroIsize;
    let hwnd = NonZeroIsize::new(ptr as isize)
        .ok_or(OpenEditorError::BadParent("parent HWND is zero"))?;
    Ok(RawWindowHandle::Win32(Win32WindowHandle::new(hwnd)))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn build_raw(ptr: *mut c_void) -> Result<RawWindowHandle, OpenEditorError> {
    use raw_window_handle::XcbWindowHandle;
    use std::num::NonZeroU32;
    let win = NonZeroU32::new(ptr as usize as u32)
        .ok_or(OpenEditorError::BadParent("parent xcb window is zero"))?;
    Ok(RawWindowHandle::Xcb(XcbWindowHandle::new(win)))
}

// ── IPC inbound: JSON → UiEvent ─────────────────────────────────────────────

/// Parse one IPC message into a [`UiEvent`]. Recognises the shared
/// vocabulary; unknown opcodes go through `parse_custom` if supplied,
/// otherwise drop silently.
pub fn parse_ui_event(body: &str, parse_custom: Option<&ParseCustomUi>) -> Option<UiEvent> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let op = v.get("op")?.as_str()?;
    if let Some(ev) = parse_ui_event_default(op, &v) {
        return Some(ev);
    }
    parse_custom.and_then(|f| f(op, &v))
}

/// Parse one of the shared vocabulary opcodes. Returns `None` for
/// unknown opcodes (caller falls through to the custom hook).
pub fn parse_ui_event_default(op: &str, v: &serde_json::Value) -> Option<UiEvent> {
    match op {
        "set_param" => Some(UiEvent::SetParam {
            id: ParamId::new(v.get("id")?.as_u64()? as usize),
            plain: v.get("plain")?.as_f64()? as f32,
        }),
        "set_param_norm" => Some(UiEvent::SetParamNorm {
            id: ParamId::new(v.get("id")?.as_u64()? as usize),
            norm: v.get("norm")?.as_f64()? as f32,
        }),
        "begin_gesture" => Some(UiEvent::BeginGesture {
            id: ParamId::new(v.get("id")?.as_u64()? as usize),
        }),
        "end_gesture" => Some(UiEvent::EndGesture {
            id: ParamId::new(v.get("id")?.as_u64()? as usize),
        }),
        "load_factory" => Some(UiEvent::LoadPreset {
            source: PresetSource::Factory {
                index: v.get("index")?.as_u64()? as usize,
            },
        }),
        "load_user" => Some(UiEvent::LoadPreset {
            source: PresetSource::User {
                path: std::path::PathBuf::from(v.get("path")?.as_str()?.to_owned()),
            },
        }),
        "rename_preset" => Some(UiEvent::RenamePreset {
            path: std::path::PathBuf::from(v.get("path")?.as_str()?.to_owned()),
            new_name: v.get("new_name")?.as_str()?.to_owned(),
        }),
        "delete_preset" => Some(UiEvent::DeletePreset {
            path: std::path::PathBuf::from(v.get("path")?.as_str()?.to_owned()),
        }),
        "move_preset" => Some(UiEvent::MovePreset {
            path: std::path::PathBuf::from(v.get("path")?.as_str()?.to_owned()),
            dest_folder: v
                .get("dest_folder")
                .and_then(|x| x.as_str())
                .map(str::to_owned),
        }),
        "rename_folder" => Some(UiEvent::RenameFolder {
            old_name: v.get("old_name")?.as_str()?.to_owned(),
            new_name: v.get("new_name")?.as_str()?.to_owned(),
        }),
        "delete_folder" => Some(UiEvent::DeleteFolder {
            name: v.get("name")?.as_str()?.to_owned(),
        }),
        "new_folder" => Some(UiEvent::NewFolder {
            suggested: v.get("suggested")?.as_str()?.to_owned(),
        }),
        "step_preset" => Some(UiEvent::StepPreset {
            delta: v.get("delta")?.as_i64()? as i32,
        }),
        "save_preset" => Some(UiEvent::SavePreset {
            name: v.get("name")?.as_str()?.to_owned(),
            folder: v.get("folder").and_then(|x| x.as_str()).map(str::to_owned),
        }),
        "ready" => Some(UiEvent::EditorReady),
        "request_text_input" => Some(UiEvent::RequestTextInput {
            id: v.get("id")?.as_str()?.to_owned(),
            title: v.get("title")?.as_str()?.to_owned(),
            initial: v.get("initial")?.as_str().unwrap_or("").to_owned(),
        }),
        "text_input_result" => Some(UiEvent::TextInputResult {
            id: v.get("id")?.as_str()?.to_owned(),
            value: v.get("value").and_then(|x| x.as_str()).map(|s| s.to_owned()),
        }),
        _ => None,
    }
}

// ── ViewEvent → JSON batches ────────────────────────────────────────────────

/// Dedupe `ParamChanged` events by id (latest value wins, preserves the
/// position of the last occurrence relative to non-`ParamChanged`
/// events).
fn dedup_param_changes(events: &[ViewEvent]) -> Vec<&ViewEvent> {
    let mut latest_for_id: HashMap<usize, usize> = HashMap::new();
    for (i, ev) in events.iter().enumerate() {
        if let ViewEvent::ParamChanged { id, .. } = ev {
            latest_for_id.insert(id.raw(), i);
        }
    }
    events
        .iter()
        .enumerate()
        .filter(|(i, ev)| match ev {
            ViewEvent::ParamChanged { id, .. } => latest_for_id.get(&id.raw()) == Some(i),
            _ => true,
        })
        .map(|(_, ev)| ev)
        .collect()
}

/// Build one or more JSON-array literals from a tick batch. Each chunk
/// is a `[...]` string ≤ `max_bytes` (a single event larger than
/// `max_bytes` still ships on its own — splitting inside a JSON object
/// would corrupt the page).
pub fn batch_chunks(
    events: &[ViewEvent],
    max_bytes: usize,
    serialise_custom: Option<&SerialiseCustomView>,
) -> Vec<String> {
    let deduped = dedup_param_changes(events);
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::from("[");
    let mut first_in_chunk = true;
    for ev in deduped {
        let Some(s) = view_event_to_json(ev, serialise_custom) else {
            continue;
        };
        let projected = current.len() + s.len() + if first_in_chunk { 1 } else { 2 };
        if !first_in_chunk && projected > max_bytes {
            current.push(']');
            chunks.push(std::mem::replace(&mut current, String::from("[")));
            first_in_chunk = true;
        }
        if !first_in_chunk {
            current.push(',');
        }
        current.push_str(&s);
        first_in_chunk = false;
    }
    current.push(']');
    if current != "[]" {
        chunks.push(current);
    }
    chunks
}

/// Serialise a [`ViewEvent`] to a JSON value the page can read. Mirror
/// of [`parse_ui_event_default`]'s opcode shape: `{ "kind": "...",
/// ...fields }`. Returns `None` for `Custom` payloads the
/// `serialise_custom` hook didn't recognise (silently dropped).
pub fn view_event_to_json(
    ev: &ViewEvent,
    serialise_custom: Option<&SerialiseCustomView>,
) -> Option<String> {
    use serde_json::json;
    let v = match ev {
        ViewEvent::ParamChanged { id, plain, norm, display } => json!({
            "kind": "param_changed",
            "id": id.raw(),
            "plain": plain,
            "norm": norm,
            "display": display,
        }),
        ViewEvent::PresetLoaded { meta, source, warnings } => json!({
            "kind": "preset_loaded",
            "name": meta.name,
            "source": preset_source_json(source.as_ref()),
            "warnings": warnings,
        }),
        ViewEvent::PresetCorpusChanged { follow } => json!({
            "kind": "preset_corpus_changed",
            "follow": follow.as_ref().map(|p| p.display().to_string()),
        }),
        ViewEvent::Status { line } => json!({
            "kind": "status",
            "line": line,
        }),
        // OpenTextInput is intercepted in `push_view_event` before
        // batching, so this arm is unreachable on the happy path.
        // Serialise a benign marker rather than `panic!` so a future
        // refactor that leaks it into the buffer fails closed (JS
        // dispatcher ignores unknown `kind`s).
        ViewEvent::OpenTextInput { id, title, initial } => json!({
            "kind": "open_text_input",
            "id": id,
            "title": title,
            "initial": initial,
        }),
        ViewEvent::TextInputResult { id, value } => json!({
            "kind": "text_input_result",
            "id": id,
            "value": value,
        }),
        ViewEvent::Custom(payload) => {
            let serialise = serialise_custom?;
            serialise(payload.as_ref())?
        }
    };
    Some(v.to_string())
}

// `corpus_snapshot_json` moved to `vxn-core-app` (model layer) so the wasm web
// controller can build the same payload without pulling this wry-bound crate
// (ADR 0009). Re-exported here so existing `vxn_core_ui_web::corpus_snapshot_json`
// callers (vxn-ui-web) are unaffected.
pub use vxn_core_app::corpus_snapshot_json;

fn preset_source_json(src: Option<&PresetSource>) -> serde_json::Value {
    use serde_json::json;
    match src {
        None => serde_json::Value::Null,
        Some(PresetSource::Factory { index }) => json!({"kind": "factory", "index": index}),
        Some(PresetSource::User { path }) => json!({"kind": "user", "path": path.display().to_string()}),
    }
}

// ── ParamDesc → JSON helpers (used by per-synth params JSON builders) ───────

/// Serialise one [`ParamDesc`] in the shape the bundled JS bridge
/// expects: `{name, label, min, max, default, kind, [unit | variants],
/// [taper]}`.
pub fn descriptor_to_json(d: &ParamDesc) -> serde_json::Value {
    use serde_json::json;
    let mut v = json!({
        "name": d.name,
        "label": d.label,
        "min": d.min,
        "max": d.max,
        "default": d.default,
    });
    // Statically unreachable panic (0115 audit): `v` is the `json!({...})`
    // object literal four lines up — always `Value::Object`.
    let obj = v.as_object_mut().expect("json object");
    match d.kind {
        ParamKind::Float { unit, taper } => {
            obj.insert("kind".into(), json!("float"));
            obj.insert("unit".into(), json!(unit));
            obj.insert("taper".into(), taper_to_json(taper));
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
    v
}

/// Serialise a [`Taper`] for the bundled JS bridge's `taper` field.
pub fn taper_to_json(t: Taper) -> serde_json::Value {
    use serde_json::json;
    match t {
        Taper::Linear => json!({"kind": "linear"}),
        Taper::Exp { mid } => json!({"kind": "exp", "mid": mid}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_raw_null_parent_is_err_not_panic() {
        // 0115: a host handing a null/zero parent used to panic inside
        // `gui.set_parent` — an unwind across the C ABI. It must now be
        // an Err the shell can map to PluginError.
        let res = build_raw(std::ptr::null_mut());
        match res {
            Err(OpenEditorError::BadParent(_)) => {}
            other => panic!("expected BadParent, got {other:?}"),
        }
    }

    #[test]
    fn open_editor_error_display_and_source() {
        let e = OpenEditorError::BadParent("parent NSView is null");
        assert!(e.to_string().contains("parent NSView is null"));
        assert!(std::error::Error::source(&e).is_none());
    }

    // ── strip_esm_exports contract (0084) ──────────────────────────────
    //
    // One test per form the doc-comment claims to handle.

    #[test]
    fn strip_export_decls_and_default_at_line_start() {
        let src = "export const X = 1;\nexport function f() {}\nexport default 7;\nconst Y = 2;\n";
        assert_eq!(
            strip_esm_exports(src),
            "const X = 1;\nfunction f() {}\n7;\nconst Y = 2;\n",
        );
    }

    #[test]
    fn strip_preserves_trailing_newline_shape() {
        assert_eq!(strip_esm_exports("export const X = 1;"), "const X = 1;");
        assert_eq!(strip_esm_exports("export const X = 1;\n"), "const X = 1;\n");
    }

    #[test]
    fn strip_indented_export_keeps_indentation() {
        // `export` not at column 0 (the original line-prefix hack missed this).
        let src = "  export const X = 1;\n\texport function f() {}\n";
        assert_eq!(strip_esm_exports(src), "  const X = 1;\n\tfunction f() {}\n");
    }

    #[test]
    fn strip_single_line_import_drops_to_blank() {
        let src = "import { foo } from './bar.js';\nconst X = 1;\n";
        assert_eq!(strip_esm_exports(src), "\nconst X = 1;\n");
        // Indented + side-effect + namespace forms all drop.
        assert_eq!(strip_esm_exports("  import './side.js';\n"), "\n");
        assert_eq!(strip_esm_exports("import * as ns from './n.js';\n"), "\n");
    }

    #[test]
    fn strip_multi_line_import_drops_whole_statement() {
        let src = "import {\n  foo,\n  bar,\n} from './bar.js';\nconst X = 1;\n";
        // Every line of the statement → blank; the `;` line ends the swallow.
        assert_eq!(strip_esm_exports(src), "\n\n\n\nconst X = 1;\n");
    }

    #[test]
    fn strip_export_list_dropped_whole() {
        // `export { … };` (export-list, no `from`) is pure plumbing → gone.
        let src = "const X = 1;\nexport { X };\nconst Y = 2;\n";
        assert_eq!(strip_esm_exports(src), "const X = 1;\n\nconst Y = 2;\n");
    }

    #[test]
    fn strip_reexport_forms_dropped_whole() {
        // `export { … } from '…'` and `export * from '…'` re-exports → gone.
        let src = "export { a, b } from './x.js';\nexport * from './y.js';\nconst Z = 3;\n";
        assert_eq!(strip_esm_exports(src), "\n\nconst Z = 3;\n");
    }

    #[test]
    fn strip_multi_line_export_list_dropped_whole() {
        let src = "export {\n  a,\n  b,\n} from './x.js';\nconst Z = 3;\n";
        assert_eq!(strip_esm_exports(src), "\n\n\n\nconst Z = 3;\n");
    }

    // ── shared widget bundle (0140) ────────────────────────────────────

    #[test]
    fn shared_widgets_js_strips_exports_and_carries_every_symbol() {
        let js = shared_widgets_js();
        // ESM markers gone (illegal in the inline-script splice).
        assert!(!js.contains("export "), "shared widgets still carry `export `");
        // Every top-level binding the panels reference is present.
        for sym in [
            "const valuePop",
            "function wireDrag",
            "function noteName",
            "function midiToHz",
            "function cutoffTunedNormToHz",
            "const CUTOFF_TUNED_MIDI_MIN",
        ] {
            assert!(js.contains(sym), "shared widgets missing `{sym}`");
        }
    }
}
