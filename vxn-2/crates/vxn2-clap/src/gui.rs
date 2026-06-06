//! CLAP `gui` extension: mounts the `vxn2-ui-web` HTML editor as a child
//! of the host's parent window.
//!
//! Structurally mirrors `vxn-1/crates/vxn-clap/src/gui.rs`. Editor IPC →
//! controller goes through `ControllerHandle`; view-event drain + per-
//! tick flush run from the timer extension (see [`crate::timer`]).

use clack_extensions::gui::*;
use clack_extensions::timer::HostTimer;
use clack_plugin::prelude::*;
use std::sync::Arc;

use crate::{VxnMainThread, lock_mut};

/// 16 ms ≈ 60 Hz. Fast enough to feel responsive on automation echo,
/// slow enough that hosts won't clamp it (CLAP spec asks for ≥ 30 Hz).
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
                // Best-effort: a host that lost track of the timer
                // between register and unregister isn't worth a panic —
                // the editor is tearing down anyway.
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
            width: vxn2_ui_web::EDITOR_WIDTH,
            height: vxn2_ui_web::EDITOR_HEIGHT,
        })
    }

    fn set_size(&mut self, _size: GuiSize) -> Result<(), PluginError> {
        // Fixed-size editor for now; accept whatever the host asks.
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
        self.gui = Some(vxn2_ui_web::open_editor(parent, ctrl_handle, corpus));

        // Register the main-thread timer so `on_timer` can drain
        // ViewEvents into the WebView. Hosts without `timer-support`
        // leave the editor static — UI gestures still flow (they post
        // straight to the controller's channel), but DAW automation
        // won't echo to the page until a tick lands. Degraded mode, not
        // a failure.
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
