//! CLAP `gui` extension: mounts the `vxn3-ui-web` faceplate as a child of the
//! host's parent window. The ceremonial `PluginGuiImpl` methods, the
//! parent-handle branch and the timer period are `vxn_core_clap::gui`'s,
//! shared with vxn-1b and vxn-2 (0317). Mirrors `vxn-2/crates/vxn2-clap/src/gui.rs`. Editor IPC
//! → controller goes through `ControllerHandle`; the per-tick drain + flush run
//! from the timer extension (`on_timer` in `lib.rs`).

use clack_extensions::gui::*;
use clack_extensions::timer::HostTimer;
use clack_plugin::prelude::*;
use std::sync::Arc;

use crate::{VxnMainThread, lock_mut};

impl PluginGuiImpl for VxnMainThread<'_> {
    vxn_core_clap::impl_fixed_size_gui_boilerplate!(
        vxn3_ui_web::EDITOR_WIDTH,
        vxn3_ui_web::EDITOR_HEIGHT
    );

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

    fn set_parent(&mut self, window: Window) -> Result<(), PluginError> {
        // The per-OS branch lives in core: without it the accessor returns
        // `None` off macOS and the editor never opens (the Windows "no UI"
        // bug), and that fix should not have to be made three times.
        let parent = vxn_core_clap::gui::parent_pointer(&window)?;

        let ctrl_handle = lock_mut(&self.controller).handle();
        let corpus = Arc::clone(&self.corpus);
        // Construction failure surfaces as PluginError (never a panic across the
        // host C ABI — vxn-1 ticket 0115); the host may retry set_parent.
        self.gui = Some(vxn3_ui_web::open_editor(parent, ctrl_handle, corpus)?);

        if let Some(host) = self.host.as_mut() {
            if let Some(host_timer) = host.shared().info().get_extension::<HostTimer>() {
                if let Ok(id) = host_timer.register_timer(host, vxn_core_clap::gui::WEBVIEW_TIMER_PERIOD_MS) {
                    self.timer = Some((host_timer, id));
                }
            }
        }
        Ok(())
    }

}
