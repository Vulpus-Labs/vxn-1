//! CLAP `gui` extension: embeds the [`vxn_ui`] Vizia editor into the host's
//! parent window. The editor talks to the engine purely through the shared
//! parameter store (`vxn_engine::SharedParams`); see [`crate::local`] for how UI
//! edits are echoed to the host.

use crate::VxnMainThread;
use clack_extensions::gui::*;
use clack_plugin::prelude::*;
use std::sync::Arc;

/// Read the backing scale of the display the host NSView is on, for pinning the
/// editor's HiDPI factor. Returns `None` — deferring to baseview's
/// self-correcting `SystemScaleFactor` — unless the view is actually attached to
/// an on-screen window (non-null `window` *and* `screen`); an unattached view
/// reports a stale 1.0, which we must not pin (an explicit `ScaleFactor` locks
/// out the later `viewDidChangeBackingProperties` correction).
#[cfg(target_os = "macos")]
fn read_macos_scale(parent: *mut std::ffi::c_void) -> Option<f64> {
    use objc::runtime::Object;
    use objc::{msg_send, sel, sel_impl};
    unsafe {
        let view = parent as *mut Object;
        let window: *mut Object = msg_send![view, window];
        if window.is_null() {
            return None;
        }
        let screen: *mut Object = msg_send![window, screen];
        if screen.is_null() {
            return None;
        }
        let s: f64 = msg_send![window, backingScaleFactor];
        if s > 0.0 {
            Some(s)
        } else {
            None
        }
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
            width: vxn_ui::EDITOR_WIDTH,
            height: vxn_ui::EDITOR_HEIGHT,
        })
    }

    fn set_size(&mut self, _size: GuiSize) -> Result<(), PluginError> {
        // Fixed-size editor for now; accept whatever the host asks.
        Ok(())
    }

    fn set_parent(&mut self, window: Window) -> Result<(), PluginError> {
        // The host hands us its native parent window for the current platform's
        // GUI API (gated by `is_api_supported`/`get_preferred_api`). Pull out the
        // raw handle pointer per platform; `vxn_ui::open_editor` wraps it in
        // vizia's `ParentWindow`, which rebuilds the matching raw-window-handle
        // for the same OS. Without the per-OS branch the accessor returns `None`
        // off-macOS, so the editor never opens (the Windows "no UI" bug).
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

        // Stash the parent pointer and open the editor lazily in `show`, not
        // here. On macOS the editor's HiDPI factor is pinned from the host
        // window's `backingScaleFactor` (see `read_macos_scale`), but at
        // `set_parent` some hosts have not yet inserted their NSView into the
        // on-screen window, so it reports a stale 1.0. Reading at `show` — the
        // next call in the embedded GUI lifecycle — gives the host time to
        // attach the view, so the scale is authoritative for the right display.
        self.pending_parent = Some(parent as usize);
        Ok(())
    }

    fn set_transient(&mut self, _window: Window) -> Result<(), PluginError> {
        Ok(())
    }

    fn show(&mut self) -> Result<(), PluginError> {
        // Open the editor on first `show` using the parent captured in
        // `set_parent`. Idempotent across hide/show cycles; `destroy` clears
        // `gui` so a fresh `set_parent`/`show` reopens.
        if self.gui.is_none() {
            let Some(parent) = self.pending_parent else {
                return Err(PluginError::Message("show before set_parent"));
            };
            let parent = parent as *mut std::ffi::c_void;

            // Pin the HiDPI scale to the backing scale of the display the host
            // window actually lives on. baseview's default `SystemScaleFactor`
            // resolves scale lazily and can latch the wrong screen on a
            // mixed-DPI setup (Retina primary + 1× external), rendering the
            // editor 2× and overflowing the 1× window. `None` keeps the
            // self-correcting system policy — used off-macOS (host drives scale
            // via `set_scale`) and as a fallback when the view still isn't
            // attached, where the lazy resolution is the only signal we have.
            #[cfg(target_os = "macos")]
            let scale = read_macos_scale(parent);
            #[cfg(not(target_os = "macos"))]
            let scale: Option<f64> = None;

            self.gui = Some(vxn_ui::open_editor(
                parent,
                Arc::clone(&self.shared.params),
                scale,
            ));
        }
        Ok(())
    }

    fn hide(&mut self) -> Result<(), PluginError> {
        Ok(())
    }
}
