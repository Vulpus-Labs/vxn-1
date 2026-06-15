//! CLAP `gui` extension: mounts the `vxn3-ui-web` faceplate as a child of the
//! host's parent window. Mirrors `vxn-2/crates/vxn2-clap/src/gui.rs`. Editor IPC
//! → controller goes through `ControllerHandle`; the per-tick drain + flush run
//! from the timer extension (`on_timer` in `lib.rs`).

use clack_extensions::gui::*;
use clack_extensions::timer::HostTimer;
use clack_plugin::prelude::*;
use std::sync::Arc;

use crate::{VxnMainThread, lock_mut};

/// 16 ms ≈ 60 Hz: responsive playhead without hosts clamping it.
pub(crate) const WEBVIEW_TIMER_PERIOD_MS: u32 = 16;

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
        if config.is_floating
            || Some(config.api_type) != GuiApiType::default_for_current_platform()
        {
            return Err(PluginError::Message("Unsupported GUI configuration"));
        }
        Ok(())
    }

    fn destroy(&mut self) {
        if let Some((host_timer, id)) = self.timer.take() {
            if let Some(host) = self.host.as_mut() {
                let _ = host_timer.unregister_timer(host, id);
            }
        }
        if let Some(mut handle) = self.gui.take() {
            handle.close();
        }
    }

    fn set_scale(&mut self, _scale: f64) -> Result<(), PluginError> {
        Ok(())
    }

    fn get_size(&mut self) -> Option<GuiSize> {
        Some(GuiSize {
            width: vxn3_ui_web::EDITOR_WIDTH,
            height: vxn3_ui_web::EDITOR_HEIGHT,
        })
    }

    fn set_size(&mut self, _size: GuiSize) -> Result<(), PluginError> {
        Ok(())
    }

    fn set_parent(&mut self, window: Window) -> Result<(), PluginError> {
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

        let ctrl_handle = lock_mut(&self.controller).handle();
        let corpus = Arc::clone(&self.corpus);
        // Construction failure surfaces as PluginError (never a panic across the
        // host C ABI — vxn-1 ticket 0115); the host may retry set_parent.
        self.gui = Some(vxn3_ui_web::open_editor(parent, ctrl_handle, corpus)?);

        if let Some(host) = self.host.as_mut() {
            if let Some(host_timer) = host.shared().info().get_extension::<HostTimer>() {
                if let Ok(id) = host_timer.register_timer(host, WEBVIEW_TIMER_PERIOD_MS) {
                    self.timer = Some((host_timer, id));
                }
            }
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
