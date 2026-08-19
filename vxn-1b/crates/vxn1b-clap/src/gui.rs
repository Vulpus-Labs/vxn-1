//! CLAP `gui` extension (E038 / 0209): embeds the HTML WebView faceplate into
//! the host's parent window. The editor talks to the engine through the
//! [`vxn_core_app::Controller`]; view-event drain + controller tick run off the
//! host's main-thread timer (see [`crate::VxnMainThread::on_timer`]).
//!
//! Mirrors VXN1's `vxn-clap/src/gui.rs` — the platform parent-handle branch, the
//! fixed-size editor, and the ~16 ms timer registration are the same shape;
//! only the crate names (`vxn1b_ui_web`) and dimensions differ.

use crate::VxnMainThread;
use clack_extensions::gui::*;
use clack_extensions::timer::HostTimer;
use clack_plugin::prelude::*;
use std::sync::Arc;

/// 16 ms ≈ 60 Hz — responsive on automation echo, inside CLAP's supported
/// timer envelope (hosts are asked to support at least 30 Hz).
const WEBVIEW_TIMER_PERIOD_MS: u32 = 16;

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
        if let Some((host_timer, id)) = self.timer.take() {
            // Best-effort: a host that lost track of the timer between register
            // and unregister isn't worth a panic — the editor is tearing down.
            let _ = host_timer.unregister_timer(&mut self.host, id);
        }
        if let Some(mut handle) = self.gui.take() {
            handle.close();
        }
        // Stop scope capture with the window. `on_timer` would do it too, but
        // the timer is unregistered above — so without this the audio thread
        // would keep filling a ring nobody reads until the editor reopens.
        self.shared.scope.set_source(vxn1b_engine::ScopeTap::Off.code());
    }

    fn set_scale(&mut self, _scale: f64) -> Result<(), PluginError> {
        Ok(())
    }

    fn get_size(&mut self) -> Option<GuiSize> {
        Some(GuiSize {
            width: vxn1b_ui_web::EDITOR_WIDTH,
            height: vxn1b_ui_web::EDITOR_HEIGHT,
        })
    }

    fn set_size(&mut self, _size: GuiSize) -> Result<(), PluginError> {
        // Fixed-size editor for now; accept whatever the host asks.
        Ok(())
    }

    fn set_parent(&mut self, window: Window) -> Result<(), PluginError> {
        // The host hands us its native parent window for the current platform's
        // GUI API (gated by `is_api_supported`/`get_preferred_api`). Pull the
        // raw pointer per platform; the WebView wraps it inside `open_editor`.
        // Without the per-OS branch the accessor returns `None` off-macOS, so
        // the editor never opens (the Windows "no UI" bug).
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

        // The webview takes the parent, a controller handle, and the shared
        // preset-corpus snapshot — the editor's browser re-reads it on every
        // `PresetCorpusChanged`. View-event drain + controller tick run off the
        // host timer (registered below), not an editor-internal idle hook.
        let ctrl_handle = crate::lock_mut(&self.controller).handle();
        let corpus = Arc::clone(&self.corpus);
        // The matrix topology is NOT a CLAP param, so the host replays nothing
        // for it on GUI open — the page's `window.vxn.matrix` snapshot has to be
        // seeded from the live store, or every source/dest combo comes back
        // showing the factory patch after a close/reopen.
        let matrices = self.shared.params.matrix_snapshot();
        // Construction failure (bad parent, wry build error) surfaces as
        // PluginError via clack's blanket `From<E: Error>` — never a panic
        // across the host's C ABI. The plugin stays alive; the host may retry.
        self.gui = Some(vxn1b_ui_web::open_editor(parent, ctrl_handle, corpus, &matrices)?);

        // Register a periodic main-thread timer so `on_timer` can drain
        // ViewEvents into the WebView. Hosts without `timer-support` leave the
        // editor static (UI gestures still post straight to the controller's
        // channel), but automation won't echo to the page until a tick lands —
        // a degraded mode, not a broken one, so we don't fail GUI creation.
        if let Some(host_timer) = self.host.shared().info().get_extension::<HostTimer>() {
            if let Ok(id) = host_timer.register_timer(&mut self.host, WEBVIEW_TIMER_PERIOD_MS) {
                self.timer = Some((host_timer, id));
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
