//! CLAP `gui` extension (E038 / 0209): embeds the HTML WebView faceplate into
//! the host's parent window. The editor talks to the engine through the
//! [`vxn_core_app::Controller`]; view-event drain + controller tick run off the
//! host's main-thread timer (see [`crate::VxnMainThread::on_timer`]).
//!
//! The nine ceremonial `PluginGuiImpl` methods, the parent-handle branch and
//! the timer period are `vxn_core_clap::gui`'s, shared with vxn-2 and vxn-3
//! (0317). What is left here is what this product actually decides: which
//! editor to open, what to hand it, and what to tear down with the window.

use crate::VxnMainThread;
use clack_extensions::gui::*;
use clack_extensions::timer::HostTimer;
use clack_plugin::prelude::*;
use std::sync::Arc;

impl PluginGuiImpl for VxnMainThread<'_> {
    vxn_core_clap::impl_fixed_size_gui_boilerplate!(
        vxn1b_ui_web::EDITOR_WIDTH,
        vxn1b_ui_web::EDITOR_HEIGHT
    );

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
        // Same argument one channel over (0338): the timer is the topology
        // ring's regular resync servicer, and it has just stopped. A resync
        // owed at this moment — the editor drove the ring past full while the
        // host was not processing — would otherwise wait for the next store
        // edit. Service it on the way out instead.
        self.shared.params.service_topology_resync();
    }

    fn set_parent(&mut self, window: Window) -> Result<(), PluginError> {
        // The per-OS branch lives in core: without it the accessor returns
        // `None` off macOS and the editor never opens (the Windows "no UI"
        // bug), and that fix should not have to be made three times.
        let parent = vxn_core_clap::gui::parent_pointer(&window)?;

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
            let period = vxn_core_clap::gui::WEBVIEW_TIMER_PERIOD_MS;
            if let Ok(id) = host_timer.register_timer(&mut self.host, period) {
                self.timer = Some((host_timer, id));
            }
        }
        Ok(())
    }

}
