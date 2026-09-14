//! CLAP `gui` extension: mounts the `vxn4-ui-web` faceplate as a child of the
//! host's parent window (ticket 0387).
//!
//! The ceremonial `PluginGuiImpl` methods and the per-OS parent-handle branch
//! are `vxn_core_clap::gui`'s, shared with the other three shells (0317) — the
//! branch in particular is load-bearing, since a single-branch version compiles,
//! links, ships and then never opens an editor on Windows.
//!
//! ## No controller yet, and no timer
//!
//! The other three shells hand `open_editor` the handle of a live
//! [`vxn_core_app::Controller`] and register a ~60 Hz host timer to pump its
//! view events into the page. vxn-4 has neither: `vxn4-app` is ticket 0386 and
//! the bindings are 0388. Until then the page is self-contained — every control
//! holds its own value — so there is nothing to pump, and a timer callback
//! firing sixty times a second to flush an empty queue would be a cost the host
//! pays for nothing.
//!
//! So the editor opens with a [`ControllerHandle::detached`] and an empty
//! corpus. A UI intent posted by the page fails at the channel, which is the
//! truth: there is nowhere for it to go. The two lines that change when 0386
//! lands are the two below with this comment on them.

use clack_extensions::gui::*;
use clack_plugin::prelude::*;
use std::sync::{Arc, Mutex};

use vxn_core_app::ControllerHandle;

use crate::VxnMainThread;

impl PluginGuiImpl for VxnMainThread<'_> {
    vxn_core_clap::impl_fixed_size_gui_boilerplate!(
        vxn4_ui_web::EDITOR_WIDTH,
        vxn4_ui_web::EDITOR_HEIGHT
    );

    /// Tear the WebView down. `EditorHandle`'s `Drop` is what actually removes
    /// the subview from the parent NSView; `close` is the shape parity the
    /// other shells have, and taking the handle out of the option is what makes
    /// a destroy → create → destroy cycle leak nothing.
    fn destroy(&mut self) {
        if let Some(mut handle) = self.gui.take() {
            handle.close();
        }
    }

    fn set_parent(&mut self, window: Window) -> Result<(), PluginError> {
        let parent = vxn_core_clap::gui::parent_pointer(&window)?;
        // A host may call `set_parent` again without a `destroy` in between.
        // Drop the old WebView first, or the second one mounts on top of it and
        // the first is never released.
        if let Some(mut old) = self.gui.take() {
            old.close();
        }
        // 0386: the controller's own handle and corpus replace these two.
        let ctrl = ControllerHandle::detached();
        let corpus = Arc::new(Mutex::new(Default::default()));
        // Construction failure surfaces as PluginError, never as a panic across
        // the host's C ABI (vxn-1 ticket 0115); the host may retry set_parent.
        self.gui = Some(vxn4_ui_web::open_editor(parent, ctrl, corpus)?);
        Ok(())
    }
}
