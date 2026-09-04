//! Keyboard-focus guard (0364).
//!
//! An embedded WebView is not a normal child widget as far as the host's
//! keyboard routing is concerned. Clicking the faceplate parks native focus
//! inside the browser control, which then consumes key events before the DAW
//! sees them: no spacebar transport, no computer-MIDI keyboard, no shortcuts.
//! Reported against Ableton Live and Logic; REAPER's window/input architecture
//! happens to be accommodating enough that it never showed up there.
//!
//! Policy: **the WebView gets the mouse; the host gets the keyboard, unless the
//! page has claimed it.** The mouse half already worked without focus — that is
//! what `with_accept_first_mouse(true)` in [`crate::open_editor`] buys.
//!
//! The guard runs from [`crate::EditorHandle::flush_view_events`], i.e. the
//! ~60 Hz main-thread timer every clack shell already registers. That is why
//! this lands with no per-synth changes: there is no new call site.
//!
//! Deliberately NOT done here: synthesising key events and injecting them into
//! the host (the `SendInput` approach some plugin developers use for FL Studio).
//! Handing focus back is the whole mechanism. Injection is what those same
//! developers then report as unreliable, and it can't be verified against the
//! hosts available to this project.
//!
//! macOS and Windows only. Linux is a no-op, matching the
//! [`crate::text_input`] stub.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use wry::WebView;

/// Set this to disable the guard and restore the pre-0364 behaviour (the
/// WebView keeps the keyboard once clicked).
///
/// A support lever rather than a convenience: Logic, FL Studio and Cubase can't
/// be tested here, so a user who hits a regression in one of them needs a fix
/// that doesn't require a rebuild. Any non-empty value other than `0` disables.
pub const KEYBOARD_ENV: &str = "VXN_WEBVIEW_KEYBOARD";

/// Whether the guard should run at all. Read once per process — the host's
/// environment doesn't change under us, and `open_editor` runs on the audio
/// host's main thread where re-reading per editor would buy nothing.
pub fn guard_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| match std::env::var(KEYBOARD_ENV) {
        Ok(v) => {
            let v = v.trim();
            v.is_empty() || v == "0"
        }
        Err(_) => true,
    })
}

/// Per-editor focus guard. Cheap to tick: when the page holds no claim and the
/// focus is already where it belongs, this is two pointer reads.
pub struct FocusGuard {
    enabled: bool,
    /// Set by the page over IPC (`want_keyboard`). While true the guard stands
    /// down — a text field is focused, or an overlay wants Escape / arrows.
    claim: Arc<AtomicBool>,
    /// The host's native parent (NSView / HWND) — where focus goes back to.
    parent: *mut c_void,
    /// The native control we're taking focus away from: the WKWebView on
    /// macOS, wry's `WRY_WEBVIEW` container HWND on Windows (WebView2 nests
    /// its own windows under that one). Null if it couldn't be resolved, which
    /// disables the guard for this editor rather than guessing.
    native: *mut c_void,
}

impl FocusGuard {
    /// Resolve the native handles once, at editor-open time.
    pub fn new(parent: *mut c_void, webview: &WebView, claim: Arc<AtomicBool>) -> Self {
        let enabled = guard_enabled();
        let native = if enabled {
            native_handle(parent, webview)
        } else {
            std::ptr::null_mut()
        };
        Self {
            enabled,
            claim,
            parent,
            native,
        }
    }

    /// Whether this guard will do anything. `false` when the env opt-out is
    /// set, when the platform has no implementation, or when the native handle
    /// didn't resolve.
    pub fn is_active(&self) -> bool {
        self.enabled && !self.native.is_null() && !self.parent.is_null()
    }

    /// One tick. Hands the keyboard back to the host if the page has drifted
    /// into owning it and hasn't claimed it.
    pub fn tick(&self) {
        if !self.is_active() {
            return;
        }
        if self.claim.load(Ordering::Relaxed) {
            return;
        }
        // SAFETY: `native` and `parent` are the handles resolved at open time;
        // the editor (hence both windows) is alive for as long as the handle
        // that owns this guard, and every call below is a read or a focus move
        // on the host's own GUI thread — the only thread `flush_view_events`
        // ever runs on.
        unsafe { yank(self.native, self.parent) }
    }
}

// The guard is only ever touched from the host's main thread (the CLAP timer
// callback), same as the rest of `EditorHandle`, whose `parent` pointer already
// carries this reasoning.
unsafe impl Send for FocusGuard {}
unsafe impl Sync for FocusGuard {}

#[cfg(target_os = "macos")]
fn native_handle(_parent: *mut c_void, webview: &WebView) -> *mut c_void {
    use wry::WebViewExtMacOS;
    // Cast straight to `c_void` rather than naming `cocoa::base::id`: that
    // would couple this crate to whichever `cocoa` / `objc` versions wry
    // resolved, and all we ever do with it is send messages.
    webview.webview() as *mut c_void
}

#[cfg(target_os = "windows")]
fn native_handle(parent: *mut c_void, _webview: &WebView) -> *mut c_void {
    win32::container(parent)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn native_handle(_parent: *mut c_void, _webview: &WebView) -> *mut c_void {
    std::ptr::null_mut()
}

#[cfg(target_os = "macos")]
unsafe fn yank(native: *mut c_void, parent: *mut c_void) {
    // SAFETY: forwarded from `FocusGuard::tick`, which has already checked
    // both handles are non-null and established the thread contract.
    unsafe { macos::yank(native, parent) }
}

#[cfg(target_os = "windows")]
unsafe fn yank(native: *mut c_void, parent: *mut c_void) {
    // SAFETY: as above.
    unsafe { win32::yank(native, parent) }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
unsafe fn yank(_native: *mut c_void, _parent: *mut c_void) {}

#[cfg(target_os = "macos")]
#[allow(unsafe_op_in_unsafe_fn)]
mod macos {
    //! wry mounts the WKWebView as a bare subview of the host's NSView
    //! (`addSubview:`, the `is_child` branch). A click makes the webview's
    //! internal content view the window's `firstResponder`, WebKit consumes
    //! `keyDown:` from there, and the host's responder chain never sees it.
    //! Putting the first responder back on the host's own view is enough for
    //! its key handling to resume.

    use std::ffi::c_void;
    use std::ptr;

    use objc::runtime::{BOOL, NO, Object};
    use objc::{class, msg_send, sel, sel_impl};

    /// True while any mouse button is down, anywhere in the process.
    ///
    /// Belt and braces: macOS mouse tracking is anchored on the view that took
    /// `mouseDown:`, not on the first responder, so moving focus mid-drag
    /// shouldn't disturb a knob gesture. A drag dying halfway is the expensive
    /// failure mode though, and this check is two instructions.
    unsafe fn mouse_is_down() -> bool {
        let buttons: usize = msg_send![class!(NSEvent), pressedMouseButtons];
        buttons != 0
    }

    pub(super) unsafe fn yank(webview: *mut c_void, parent: *mut c_void) {
        let webview = webview as *mut Object;
        let window: *mut Object = msg_send![webview, window];
        if window.is_null() {
            // Editor not in a window yet (or already torn out of one).
            return;
        }
        if mouse_is_down() {
            return;
        }
        let responder: *mut Object = msg_send![window, firstResponder];
        if responder.is_null() {
            return;
        }
        // `isDescendantOf:` is NSView-only, and the first responder is often
        // not a view at all — the window itself, or a text field's field
        // editor. Check before asking.
        let is_view: BOOL = msg_send![responder, isKindOfClass: class!(NSView)];
        if is_view == NO {
            return;
        }
        // Returns YES for the webview itself as well as its content view, so
        // this catches both the click-on-chrome and click-on-page cases.
        let inside: BOOL = msg_send![responder, isDescendantOf: webview];
        if inside == NO {
            return;
        }
        // Hand it back. A host view that refuses first-responder status
        // returns NO; falling back to `nil` makes the window itself the first
        // responder, which is still out of WebKit's hands and still lets the
        // host's own key routing run.
        let parent = parent as *mut Object;
        let took: BOOL = msg_send![window, makeFirstResponder: parent];
        if took == NO {
            let nil: *mut Object = ptr::null_mut();
            let _: BOOL = msg_send![window, makeFirstResponder: nil];
        }
    }
}

#[cfg(target_os = "windows")]
mod win32 {
    //! wry creates a container window of class `WRY_WEBVIEW` as a direct child
    //! of the host HWND (`create_container_hwnd`) and WebView2 nests its own
    //! windows under it. Once clicked, the GUI thread's focus lives somewhere
    //! in that subtree and keystrokes go to the browser process.
    //!
    //! `GetFocus` / `SetFocus` are per-thread, and this runs on the host's GUI
    //! thread (the CLAP timer callback), which is the thread that owns both
    //! windows — so the thread-local focus is exactly the one we want.

    use std::ffi::c_void;
    use std::ptr;

    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetCapture, GetFocus, SetFocus};
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowExW, IsChild};

    /// wry's container window class, UTF-16 null-terminated for `FindWindowExW`.
    /// Pinned here as a literal because wry doesn't expose the HWND on
    /// `WebViewExtWindows` in 0.45 (only `controller()` and `reparent()`), and
    /// going through the WebView2 controller would couple us to whichever
    /// `webview2-com` version wry resolved.
    static WRY_CLASS: &[u16] = &[
        b'W' as u16, b'R' as u16, b'Y' as u16, b'_' as u16, b'W' as u16, b'E' as u16,
        b'B' as u16, b'V' as u16, b'I' as u16, b'E' as u16, b'W' as u16, 0,
    ];

    /// Find the container wry parented under the host window. Null if it isn't
    /// there — which disables the guard rather than having it flail at a
    /// window that doesn't exist.
    pub(super) fn container(parent: *mut c_void) -> *mut c_void {
        if parent.is_null() {
            return ptr::null_mut();
        }
        unsafe {
            FindWindowExW(
                parent as HWND,
                ptr::null_mut(),
                WRY_CLASS.as_ptr(),
                ptr::null(),
            ) as *mut c_void
        }
    }

    pub(super) unsafe fn yank(container: *mut c_void, parent: *mut c_void) {
        // SAFETY: every call below is a focus read or move on the caller's own
        // GUI thread, with handles the caller has already null-checked.
        unsafe {
            // A drag in progress has the mouse captured; leave it alone. Same
            // reasoning as the macOS `pressedMouseButtons` check.
            if !GetCapture().is_null() {
                return;
            }
            let focus = GetFocus();
            if focus.is_null() {
                return;
            }
            let container = container as HWND;
            if focus != container && IsChild(container, focus) == 0 {
                // Focus is somewhere else in the host — not ours to move.
                return;
            }
            SetFocus(parent as HWND);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The opt-out has to be readable by a support email: "set this to 1".
    /// The parsing is the only part of the guard that runs off-platform, so it
    /// is the only part with a unit test — the rest needs a host.
    #[test]
    fn guard_enabled_reads_the_documented_values() {
        // `guard_enabled` memoises per process, so exercise the classifier
        // rather than the env var itself (tests share a process, and mutating
        // the environment from one is unsound under a threaded runner).
        fn enabled_for(v: Option<&str>) -> bool {
            match v {
                Some(v) => {
                    let v = v.trim();
                    v.is_empty() || v == "0"
                }
                None => true,
            }
        }
        assert!(enabled_for(None), "unset must leave the guard on");
        assert!(enabled_for(Some("0")), "0 must leave the guard on");
        assert!(enabled_for(Some("")), "empty must leave the guard on");
        assert!(!enabled_for(Some("1")), "1 must disable the guard");
        assert!(!enabled_for(Some("yes")), "any other value disables");
    }

    /// A guard with no native handle must be inert, not merely harmless: on a
    /// platform without an implementation, or when the container window isn't
    /// found, `tick` has nothing to move and must not try.
    #[test]
    fn a_guard_without_handles_is_inactive() {
        let g = FocusGuard {
            enabled: true,
            claim: Arc::new(AtomicBool::new(false)),
            parent: std::ptr::null_mut(),
            native: std::ptr::null_mut(),
        };
        assert!(!g.is_active());
        g.tick(); // must not dereference anything
    }

    /// The page's claim is the whole reason the search field still works.
    #[test]
    fn a_claimed_keyboard_stands_the_guard_down() {
        let claim = Arc::new(AtomicBool::new(true));
        let g = FocusGuard {
            enabled: true,
            claim: claim.clone(),
            // Non-null so `is_active` passes; `tick` must bail on the claim
            // before it ever touches these.
            parent: 1usize as *mut c_void,
            native: 2usize as *mut c_void,
        };
        assert!(g.is_active());
        g.tick(); // claimed → returns before any native call
        claim.store(false, Ordering::Relaxed);
        // Not calling `tick` here: with the claim dropped it would follow the
        // dangling pointers above. The claim check is what this pins.
    }
}
