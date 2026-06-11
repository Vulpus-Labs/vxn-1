//! Pluggable editor backend trait.
//!
//! The CLAP shell talks to the editor only through this trait. The
//! parent-window type is associated so this crate stays free of any
//! windowing dependency — `vxn-core-ui-web` supplies the `wry` impl with
//! a `raw-window-handle` parent.

use std::error::Error;

use crate::controller::{ControllerHandle, CorpusHandle};
use crate::events::ViewEvent;

pub trait EditorBackend: 'static {
    /// Concrete handle returned by [`Self::open`] — the host keeps this
    /// alive for the editor's lifetime.
    type Handle;

    /// Backend-specific parent window descriptor (a raw window handle
    /// for baseview/wry, an `NSView` pointer for a native macOS shell).
    type ParentWindow;

    /// `corpus` is the controller-published preset snapshot. The backend
    /// reads it on open to seed its browser panel and re-reads after
    /// each [`ViewEvent::PresetCorpusChanged`].
    ///
    /// Construction failure (bad parent handle, WebView build error) is
    /// an `Err`, never a panic — the call sits on the CLAP
    /// `gui.set_parent` path, where an unwind would cross the host's
    /// `extern "C"` frame (UB). The shell maps the error to
    /// `PluginError`; the plugin stays alive and audio keeps rendering.
    fn open(
        parent: Self::ParentWindow,
        ctrl: ControllerHandle,
        corpus: CorpusHandle,
    ) -> Result<Self::Handle, Box<dyn Error>>;

    fn close(handle: &mut Self::Handle);

    /// Forward a `ViewEvent` into the backend's render context. Called
    /// from the controller's thread; the backend marshals onto its own
    /// UI thread if needed.
    fn push_view_event(handle: &Self::Handle, event: ViewEvent);

    /// Flush any events buffered by [`Self::push_view_event`]. Called
    /// once per host tick after every push. Default is a no-op for
    /// backends that dispatch synchronously.
    fn flush_view_events(_handle: &Self::Handle) {}
}
