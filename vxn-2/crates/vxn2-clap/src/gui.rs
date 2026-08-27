//! CLAP `gui` extension: mounts the `vxn2-ui-web` HTML editor as a child
//! of the host's parent window.
//!
//! Editor IPC → controller goes through `ControllerHandle`; view-event
//! drain + per-tick flush run from the timer extension (see [`crate::timer`]).

use clack_extensions::gui::*;
use clack_extensions::timer::HostTimer;
use clack_plugin::prelude::*;
use std::sync::Arc;

use crate::{VxnMainThread, lock_mut};

impl PluginGuiImpl for VxnMainThread<'_> {
    vxn_core_clap::impl_fixed_size_gui_boilerplate!(
        vxn2_ui_web::EDITOR_WIDTH,
        vxn2_ui_web::EDITOR_HEIGHT
    );

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

    fn set_parent(&mut self, window: Window) -> Result<(), PluginError> {
        // The per-OS branch lives in core: without it the accessor returns
        // `None` off macOS and the editor never opens (the Windows "no UI"
        // bug), and that fix should not have to be made three times.
        let parent = vxn_core_clap::gui::parent_pointer(&window)?;

        let ctrl_handle = lock_mut(&self.controller).handle();
        let corpus = Arc::clone(&self.corpus);
        // Construction failure (bad parent, wry build error) surfaces as
        // PluginError via clack's blanket `From<E: Error>` — never a
        // panic across the host's C ABI. The plugin stays alive; the host
        // may retry set_parent.
        self.gui = Some(vxn2_ui_web::open_editor(parent, ctrl_handle, corpus)?);

        // Register the main-thread timer so `on_timer` can drain
        // ViewEvents into the WebView. Hosts without `timer-support`
        // leave the editor static — UI gestures still flow (they post
        // straight to the controller's channel), but DAW automation
        // won't echo to the page until a tick lands. Degraded mode, not
        // a failure.
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
