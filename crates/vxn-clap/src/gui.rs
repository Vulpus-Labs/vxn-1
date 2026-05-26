//! CLAP `gui` extension: embeds the [`vxn_ui`] Vizia editor into the host's
//! parent window. The editor talks to the engine purely through the shared
//! parameter store (`vxn_engine::SharedParams`); see [`crate::local`] for how UI
//! edits are echoed to the host.

use crate::VxnMainThread;
use clack_extensions::gui::*;
use clack_plugin::prelude::*;
use std::sync::Arc;

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
        self.gui = Some(vxn_ui::open_editor(parent, Arc::clone(&self.shared.params)));
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
