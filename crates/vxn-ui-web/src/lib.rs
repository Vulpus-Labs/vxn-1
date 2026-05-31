//! VXN1 web editor backend (E010 / 0039 scaffold).
//!
//! A [`wry`] WebView attached as a child of the host's parent window. The HTML
//! is a placeholder for now — the real faceplate lands in 0040+. What ships
//! here is the *bridge*:
//!
//! - **JS → Rust:** the page calls `window.ipc.postMessage(json)`; the IPC
//!   handler parses one of the small set of opcodes below and posts the
//!   matching [`UiEvent`] onto the controller's UI sender.
//! - **Rust → JS:** [`EditorHandle::push_view_event`] serializes a
//!   [`ViewEvent`] and calls `webview.evaluate_script`, which the page picks
//!   up via `window.vxn.onViewEvent(ev)`. For 0039 the page just logs them;
//!   structured DOM updates land per-panel in 0041+.
//!
//! [`WebEditor`] is the [`EditorBackend`] impl the clack shell will hold once
//! 0047 flips it from vizia to this crate. Until then, the trait surface is
//! the contract a future shell programs against.

use std::ffi::c_void;

use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, WindowHandle as RwhWindowHandle,
};
use vxn_app::{
    ControllerHandle, EditorBackend, KeyMode, Layer, ParamId, PresetSource, UiEvent, ViewEvent,
};
use wry::{Rect, WebView, WebViewBuilder};
use wry::dpi::{LogicalPosition, LogicalSize};

/// Logical pixel dimensions of the editor. Matches the vizia editor's
/// [`vxn_ui::EDITOR_WIDTH`] / `_HEIGHT` so swapping backends doesn't reflow
/// the host's plugin window.
pub const EDITOR_WIDTH: u32 = 1024;
pub const EDITOR_HEIGHT: u32 = 772;

/// Live editor. Dropping it tears down the WebView; on macOS wry removes the
/// subview from the parent NSView as part of that.
pub struct EditorHandle {
    webview: WebView,
}

impl EditorHandle {
    /// Push one [`ViewEvent`] into the page. For 0039 the page just logs
    /// these; 0041+ will translate into DOM updates.
    pub fn push_view_event(&self, event: ViewEvent) {
        let payload = view_event_to_json(&event);
        let js = format!(
            "if(window.vxn&&window.vxn.onViewEvent){{window.vxn.onViewEvent({payload})}}"
        );
        let _ = self.webview.evaluate_script(&js);
    }
}

/// Zero-sized type that names this backend for trait-bounded code (the clack
/// shell, tests). All state lives in [`EditorHandle`].
pub struct WebEditor;

impl EditorBackend for WebEditor {
    type Handle = EditorHandle;
    /// Raw native parent: NSView pointer on macOS, HWND on Windows, xcb window
    /// id (zero-extended into a pointer slot) on Linux. The clack shell
    /// already extracts these per-platform in `gui::set_parent`.
    type ParentWindow = *mut c_void;

    fn open(parent: Self::ParentWindow, ctrl: ControllerHandle) -> Self::Handle {
        open_editor(parent, ctrl)
    }

    fn close(handle: &mut Self::Handle) {
        // Tear down by replacing the handle's WebView with… nothing useful.
        // The host owns the `EditorHandle`; close() is typically just a
        // marker call before drop, so we don't reach into wry internals.
        let _ = handle;
    }

    fn push_view_event(handle: &Self::Handle, event: ViewEvent) {
        handle.push_view_event(event);
    }
}

/// Build the WebView under `parent`, wire the IPC handler to `ctrl`, and load
/// the placeholder page. `parent` is the same raw pointer the host hands the
/// clack shell in `gui::set_parent` (NSView / HWND / xcb-window-id).
pub fn open_editor(parent: *mut c_void, ctrl: ControllerHandle) -> EditorHandle {
    let parent = ParentWindow { raw: build_raw(parent) };
    let webview = WebViewBuilder::new_as_child(&parent)
        .with_html(PLACEHOLDER_HTML)
        .with_bounds(Rect {
            position: LogicalPosition::new(0i32, 0i32).into(),
            size: LogicalSize::new(EDITOR_WIDTH, EDITOR_HEIGHT).into(),
        })
        .with_ipc_handler(move |req| {
            if let Some(ev) = parse_ui_event(req.body()) {
                let _ = ctrl.post(ev);
            }
        })
        .build()
        .expect("wry WebView build failed");
    EditorHandle { webview }
}

// ── Parent-window adapter ───────────────────────────────────────────────────

/// Newtype that lets a raw native parent pointer satisfy
/// [`HasWindowHandle`]. The host owns the underlying window for the editor's
/// lifetime — we never outlive it.
struct ParentWindow {
    raw: RawWindowHandle,
}

// `RawWindowHandle` is `!Send`/`!Sync`; wry doesn't require either on the
// `HasWindowHandle` impl, but the bounds aren't expressible without these
// unsafe asserts on some toolchains. Safe here because we hand the parent
// straight to wry on the same thread and never share it.
unsafe impl Send for ParentWindow {}
unsafe impl Sync for ParentWindow {}

impl HasWindowHandle for ParentWindow {
    fn window_handle(&self) -> Result<RwhWindowHandle<'_>, HandleError> {
        // SAFETY: `raw` was built from the host-provided native handle; it
        // stays valid as long as the host hasn't destroyed the GUI, which
        // strictly outlives every borrow wry takes here.
        Ok(unsafe { RwhWindowHandle::borrow_raw(self.raw) })
    }
}

#[cfg(target_os = "macos")]
fn build_raw(ptr: *mut c_void) -> RawWindowHandle {
    use raw_window_handle::AppKitWindowHandle;
    use std::ptr::NonNull;
    let ns_view = NonNull::new(ptr).expect("parent NSView is null");
    RawWindowHandle::AppKit(AppKitWindowHandle::new(ns_view))
}

#[cfg(target_os = "windows")]
fn build_raw(ptr: *mut c_void) -> RawWindowHandle {
    use raw_window_handle::Win32WindowHandle;
    use std::num::NonZeroIsize;
    let hwnd = NonZeroIsize::new(ptr as isize).expect("parent HWND is zero");
    RawWindowHandle::Win32(Win32WindowHandle::new(hwnd))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn build_raw(ptr: *mut c_void) -> RawWindowHandle {
    use raw_window_handle::XcbWindowHandle;
    use std::num::NonZeroU32;
    // The clack shell hands us the xcb window id zero-extended into a pointer
    // slot; truncate back to u32. Matches `gui::set_parent`.
    let win = NonZeroU32::new(ptr as usize as u32).expect("parent xcb window is zero");
    RawWindowHandle::Xcb(XcbWindowHandle::new(win))
}

// ── IPC inbound: JSON → UiEvent ─────────────────────────────────────────────

/// Parse one IPC message into a [`UiEvent`]. Returns `None` for malformed
/// payloads or unknown opcodes (logged silently — surfacing parse errors is a
/// later ticket).
///
/// Wire shape: `{ "op": "<opcode>", ...fields }`. The opcode set below is the
/// minimum that lets 0041+ wire faders, transport, layer toggles, and
/// factory-bank loads against the controller. Path-based preset mutations
/// (save / rename / move / delete) join in 0049–0051 once the browser HTML
/// lands.
fn parse_ui_event(body: &str) -> Option<UiEvent> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let op = v.get("op")?.as_str()?;
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
        "reset_layer" => Some(UiEvent::ResetLayer {
            layer: parse_layer(v.get("layer")?)?,
        }),
        "load_factory" => Some(UiEvent::LoadPreset {
            source: PresetSource::Factory {
                index: v.get("index")?.as_u64()? as usize,
            },
        }),
        "set_key_mode" => Some(UiEvent::SetKeyMode {
            mode: parse_key_mode(v.get("mode")?)?,
        }),
        "set_split_point" => Some(UiEvent::SetSplitPoint {
            note: v.get("note")?.as_u64()? as u8,
        }),
        "set_edit_layer" => Some(UiEvent::SetEditLayer {
            layer: parse_layer(v.get("layer")?)?,
        }),
        _ => None,
    }
}

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

// ── ViewEvent → JSON ────────────────────────────────────────────────────────

/// Serialize a [`ViewEvent`] to a JSON value the page can read. Mirror of
/// [`parse_ui_event`]'s opcode shape: `{ "kind": "...", ...fields }`.
fn view_event_to_json(ev: &ViewEvent) -> String {
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
        ViewEvent::KeyModeChanged { mode } => json!({
            "kind": "key_mode_changed",
            "mode": *mode as u8,
        }),
        ViewEvent::Status { line } => json!({
            "kind": "status",
            "line": line,
        }),
    };
    v.to_string()
}

fn preset_source_json(src: Option<&PresetSource>) -> serde_json::Value {
    use serde_json::json;
    match src {
        None => serde_json::Value::Null,
        Some(PresetSource::Factory { index }) => json!({"kind": "factory", "index": index}),
        Some(PresetSource::User { path }) => json!({"kind": "user", "path": path.display().to_string()}),
    }
}

// ── Placeholder page ────────────────────────────────────────────────────────

const PLACEHOLDER_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<title>VXN1</title>
<style>
  html, body { margin: 0; height: 100%; background: #1a1a1a; color: #ddd;
    font-family: -apple-system, system-ui, sans-serif; }
  #root { display: flex; align-items: center; justify-content: center;
    height: 100%; font-size: 28px; letter-spacing: 0.15em; }
</style>
</head>
<body>
<div id="root">VULPUS LABS - VXN-1</div>
<script>
  // Bridge object: pages call window.vxn.send({op:'...',...}) to post a
  // UiEvent at the controller; Rust pushes ViewEvents back by calling
  // window.vxn.onViewEvent(ev).
  window.vxn = {
    send: function (msg) {
      try { window.ipc.postMessage(JSON.stringify(msg)); }
      catch (e) { console.warn('vxn.send failed', e); }
    },
    onViewEvent: function (ev) { console.log('vxn:view', ev); },
  };
</script>
</body>
</html>
"#;

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use vxn_app::PresetMeta;

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
    fn parses_layer_and_key_mode() {
        let ev = parse_ui_event(r#"{"op":"set_edit_layer","layer":"lower"}"#).unwrap();
        assert!(matches!(ev, UiEvent::SetEditLayer { layer: Layer::Lower }));
        let ev = parse_ui_event(r#"{"op":"set_key_mode","mode":2}"#).unwrap();
        assert!(matches!(ev, UiEvent::SetKeyMode { mode: KeyMode::Split }));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_ui_event("not json").is_none());
        assert!(parse_ui_event(r#"{"op":"unknown"}"#).is_none());
        assert!(parse_ui_event(r#"{"op":"set_param_norm","id":42}"#).is_none());
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
}
