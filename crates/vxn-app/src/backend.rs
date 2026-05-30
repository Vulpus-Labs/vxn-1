//! Pluggable editor backend (ADR 0007 §2, §4).
//!
//! The clack shell talks only through this trait; whichever editor crate is
//! compiled in (`vxn-ui-vizia` today, `vxn-ui-web` after E010) provides the
//! impl. Parent-window type is associated so this crate stays free of any
//! windowing dependency.

use crate::controller::ControllerHandle;
use crate::events::ViewEvent;

pub trait EditorBackend: 'static {
    /// Concrete handle returned by [`Self::open`] — the host keeps this alive
    /// for the editor's lifetime.
    type Handle;

    /// Backend-specific parent window descriptor (raw window handle for
    /// Vizia/baseview; an `NSView` pointer for the WebView crate).
    type ParentWindow;

    fn open(parent: Self::ParentWindow, ctrl: ControllerHandle) -> Self::Handle;
    fn close(handle: &mut Self::Handle);

    /// Forward a `ViewEvent` into the backend's render context. Called from
    /// the controller's thread; the backend is responsible for marshalling
    /// onto its own UI thread if needed.
    fn push_view_event(handle: &Self::Handle, event: ViewEvent);
}
