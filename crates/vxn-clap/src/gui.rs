//! CLAP `gui` extension: embeds whichever editor backend the build picked
//! (vizia or webview) into the host's parent window. The editor talks to the
//! engine through the controller (ADR 0007) — host echoes still go via
//! [`crate::local`]. Backend selection happens in [`crate::lib`]'s top-level
//! `vxn_editor` re-alias; this file only deals with the parent-window
//! plumbing and the per-backend `open_editor` call shapes.

use crate::{VxnMainThread, vxn_editor};
use clack_extensions::gui::*;
use clack_plugin::prelude::*;
#[cfg(feature = "vizia")]
use std::sync::Arc;
#[cfg(feature = "vizia")]
use vxn_app::ParamModel;

/// Backing scale factor of the host's parent NSView, via its window (falling
/// back to the main screen when the view isn't in a window yet). Used to pin
/// the vizia editor's HiDPI scale at attach time, since vizia's
/// `SystemScaleFactor` placeholder isn't reliably corrected after attach. The
/// webview backend uses CSS logical pixels and ignores this.
#[cfg(all(target_os = "macos", feature = "vizia"))]
fn parent_backing_scale(nsview: *mut std::ffi::c_void) -> f64 {
    use objc::runtime::Object;
    use objc::{msg_send, sel, sel_impl};
    if nsview.is_null() {
        return -1.0;
    }
    unsafe {
        let view = nsview as *mut Object;
        let window: *mut Object = msg_send![view, window];
        if !window.is_null() {
            return msg_send![window, backingScaleFactor];
        }
        let cls = objc::runtime::Class::get("NSScreen");
        if let Some(cls) = cls {
            let screen: *mut Object = msg_send![cls, mainScreen];
            if !screen.is_null() {
                return msg_send![screen, backingScaleFactor];
            }
        }
        0.0
    }
}

impl PluginGuiImpl for VxnMainThread<'_> {
    fn is_api_supported(&mut self, config: GuiConfiguration) -> bool {
        Some(config.api_type) == GuiApiType::default_for_current_platform() && !config.is_floating
    }

    fn get_preferred_api(&mut self) -> Option<GuiConfiguration<'_>> {
        Some(GuiConfiguration {
            api_type: GuiApiType::default_for_current_platform()?,
            is_floating: false,
        })
    }

    fn create(&mut self, config: GuiConfiguration) -> Result<(), PluginError> {
        if config.is_floating || Some(config.api_type) != GuiApiType::default_for_current_platform()
        {
            return Err(PluginError::Message("Unsupported GUI configuration"));
        }
        Ok(())
    }

    fn destroy(&mut self) {
        if let Some(mut handle) = self.gui.take() {
            handle.close();
        }
    }

    fn set_scale(&mut self, _scale: f64) -> Result<(), PluginError> {
        Ok(())
    }

    fn get_size(&mut self) -> Option<GuiSize> {
        Some(GuiSize {
            width: vxn_editor::EDITOR_WIDTH,
            height: vxn_editor::EDITOR_HEIGHT,
        })
    }

    fn set_size(&mut self, _size: GuiSize) -> Result<(), PluginError> {
        // Fixed-size editor for now; accept whatever the host asks.
        Ok(())
    }

    fn set_parent(&mut self, window: Window) -> Result<(), PluginError> {
        // The host hands us its native parent window for the current
        // platform's GUI API (gated by `is_api_supported`/`get_preferred_api`).
        // Pull the raw pointer per platform; each backend wraps it in its own
        // window-handle shape inside `open_editor`. Without the per-OS branch
        // the accessor returns `None` off-macOS, so the editor never opens
        // (the Windows "no UI" bug).
        #[cfg(target_os = "macos")]
        let parent = window.as_cocoa_nsview().ok_or(PluginError::Message(
            "Expected a Cocoa (NSView) parent window",
        ))?;
        #[cfg(target_os = "windows")]
        let parent = window.as_win32_hwnd().ok_or(PluginError::Message(
            "Expected a Win32 (HWND) parent window",
        ))?;
        #[cfg(target_os = "linux")]
        let parent = window
            .as_x11_handle()
            .map(|h| h as *mut std::ffi::c_void)
            .ok_or(PluginError::Message("Expected an X11 parent window"))?;

        #[cfg(feature = "vizia")]
        {
            // Pin the editor to the host window's real backing scale, read from
            // the parent NSView. vizia's `SystemScaleFactor` placeholder isn't
            // corrected on displays where the backing scale never changes after
            // attach, so the editor would otherwise render oversized.
            #[cfg(target_os = "macos")]
            let scale_override = Some(parent_backing_scale(parent)).filter(|s| *s > 0.0);
            #[cfg(not(target_os = "macos"))]
            let scale_override = None;

            // Build a `ControllerHandle` for UiEvent posts. The view-event
            // receiver + corpus + tick come straight from the main thread; the
            // model is `SharedParams` erased to `dyn ParamModel` so the editor
            // never needs the engine type.
            let model: Arc<dyn ParamModel> = self.shared.params.clone();
            let handle = crate::lock_mut(&self.controller).handle();
            let view_rx = Arc::clone(&self.view_rx);
            let corpus = Arc::clone(&self.corpus);
            let tick = self.tick.clone();
            self.gui = Some(vxn_editor::open_editor(
                parent,
                model,
                handle,
                view_rx,
                corpus,
                tick,
                scale_override,
            ));
        }
        #[cfg(feature = "webview")]
        {
            // The webview backend only needs the parent and a controller
            // handle — model reads / view drain / tick all wait for the
            // panel-binding tickets (0041+). For 0047 the placeholder HTML
            // renders structure-only, so the bridge alone is enough.
            let handle = crate::lock_mut(&self.controller).handle();
            self.gui = Some(vxn_editor::open_editor(parent, handle));
        }
        Ok(())
    }

    fn set_transient(&mut self, _window: Window) -> Result<(), PluginError> {
        Ok(())
    }

    fn show(&mut self) -> Result<(), PluginError> {
        Ok(())
    }

    fn hide(&mut self) -> Result<(), PluginError> {
        Ok(())
    }
}
