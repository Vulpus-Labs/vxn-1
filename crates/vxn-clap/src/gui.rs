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
        let nsview = window.as_cocoa_nsview().ok_or(PluginError::Message(
            "Expected a Cocoa (NSView) parent window",
        ))?;
        self.gui = Some(vxn_ui::open_editor(nsview, Arc::clone(&self.shared.params)));
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
