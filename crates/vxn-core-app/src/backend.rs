//! Pluggable editor backend trait.
//!
//! The CLAP shell talks to the editor only through this trait. The
//! parent-window type is associated so this crate stays free of any
//! windowing dependency — `vxn-core-ui-web` supplies the `wry` impl with
//! a `raw-window-handle` parent.

use crate::events::ViewEvent;

pub trait EditorBackend: 'static {
    /// Concrete handle returned when an editor opens — the host keeps this
    /// alive for the editor's lifetime.
    type Handle;

    /// Backend-specific parent window descriptor (a raw window handle
    /// for baseview/wry, an `NSView` pointer for a native macOS shell).
    type ParentWindow;

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
