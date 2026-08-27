//! The parts of a `PluginGuiImpl` that are the same for every VXN synth (0317).
//!
//! All three plugins mount an HTML WebView editor as a child of the host's
//! parent window, fixed-size, driven by a ~60 Hz host timer. What differs is
//! the main-thread type, the editor-open call and the teardown side effects —
//! so this is not a generic `WebviewGui<E>`, which would need an associated
//! item per difference. It is the two pieces that are genuinely identical and
//! have a bug history:
//!
//! - [`parent_pointer`], the per-OS parent-handle branch. Without the branch
//!   the accessor returns `None` off macOS and the editor never opens — the
//!   Windows "no UI" bug, once fixed in one copy and absent from the others.
//! - [`impl_fixed_size_gui_boilerplate!`], the nine `PluginGuiImpl` methods
//!   that are pure ceremony for a fixed-size, non-floating, default-API editor.

use clack_extensions::gui::{GuiApiType, Window};
use clack_plugin::prelude::PluginError;

/// 16 ms ≈ 60 Hz — responsive on automation echo, and inside CLAP's supported
/// timer envelope (the spec asks hosts for at least 30 Hz).
pub const WEBVIEW_TIMER_PERIOD_MS: u32 = 16;

/// The host's native parent window as a raw pointer for the current platform.
///
/// The host hands over a handle for the platform's GUI API (gated by
/// `is_api_supported` / `get_preferred_api`); the WebView wraps the pointer.
/// **The per-OS branch is load-bearing**: the macOS accessor returns `None`
/// everywhere else, so a single-branch version compiles, links, ships and then
/// never opens an editor on Windows.
pub fn parent_pointer(window: &Window) -> Result<*mut core::ffi::c_void, PluginError> {
    #[cfg(target_os = "macos")]
    {
        window.as_cocoa_nsview().ok_or(PluginError::Message(
            "Expected a Cocoa (NSView) parent window",
        ))
    }
    #[cfg(target_os = "windows")]
    {
        window.as_win32_hwnd().ok_or(PluginError::Message(
            "Expected a Win32 (HWND) parent window",
        ))
    }
    #[cfg(target_os = "linux")]
    {
        window
            .as_x11_handle()
            .map(|h| h as *mut core::ffi::c_void)
            .ok_or(PluginError::Message("Expected an X11 parent window"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = window;
        Err(PluginError::Message(
            "No GUI parent-window API for this platform",
        ))
    }
}

/// Whether a requested configuration is one a fixed-size embedded WebView can
/// serve: the platform's default API, not floating.
pub fn config_is_supported(api_type: GuiApiType, is_floating: bool) -> bool {
    Some(api_type) == GuiApiType::default_for_current_platform() && !is_floating
}

/// The nine `PluginGuiImpl` methods that carry no per-product decision.
///
/// Expands inside an `impl PluginGuiImpl for …` block; the caller still writes
/// `destroy` and `set_parent`, which are where the products actually differ.
/// `$w` / `$h` are the editor's fixed logical size.
#[macro_export]
macro_rules! impl_fixed_size_gui_boilerplate {
    ($w:expr, $h:expr) => {
        fn is_api_supported(
            &mut self,
            config: ::clack_extensions::gui::GuiConfiguration,
        ) -> bool {
            $crate::gui::config_is_supported(config.api_type, config.is_floating)
        }

        fn get_preferred_api(
            &mut self,
        ) -> Option<::clack_extensions::gui::GuiConfiguration<'_>> {
            Some(::clack_extensions::gui::GuiConfiguration {
                api_type: ::clack_extensions::gui::GuiApiType::default_for_current_platform()?,
                is_floating: false,
            })
        }

        fn create(
            &mut self,
            config: ::clack_extensions::gui::GuiConfiguration,
        ) -> Result<(), ::clack_plugin::prelude::PluginError> {
            if !$crate::gui::config_is_supported(config.api_type, config.is_floating) {
                return Err(::clack_plugin::prelude::PluginError::Message(
                    "Unsupported GUI configuration",
                ));
            }
            Ok(())
        }

        fn set_scale(&mut self, _scale: f64) -> Result<(), ::clack_plugin::prelude::PluginError> {
            Ok(())
        }

        fn get_size(&mut self) -> Option<::clack_extensions::gui::GuiSize> {
            Some(::clack_extensions::gui::GuiSize {
                width: $w,
                height: $h,
            })
        }

        /// Fixed-size editor: accept whatever the host asks and keep our size.
        fn set_size(
            &mut self,
            _size: ::clack_extensions::gui::GuiSize,
        ) -> Result<(), ::clack_plugin::prelude::PluginError> {
            Ok(())
        }

        fn set_transient(
            &mut self,
            _window: ::clack_extensions::gui::Window,
        ) -> Result<(), ::clack_plugin::prelude::PluginError> {
            Ok(())
        }

        fn show(&mut self) -> Result<(), ::clack_plugin::prelude::PluginError> {
            Ok(())
        }

        fn hide(&mut self) -> Result<(), ::clack_plugin::prelude::PluginError> {
            Ok(())
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A floating window is refused even on the right API — the editor is
    /// embedded, and accepting the request would leave the host showing an
    /// empty floating frame.
    #[test]
    fn only_the_platform_default_api_and_embedded_windows_are_supported() {
        let Some(api) = GuiApiType::default_for_current_platform() else {
            return; // headless platform; nothing to assert
        };
        assert!(config_is_supported(api, false));
        assert!(!config_is_supported(api, true), "floating must be refused");
    }
}
