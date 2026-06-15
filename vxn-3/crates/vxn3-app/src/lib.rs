//! VXN3 controller glue (ticket 0052).
//!
//! vxn-3's faceplate edits are *structured* sequencer state (grid cells, engine
//! selection, knobs), not a flat CLAP-param array — so unlike vxn-2 there is no
//! `ParamModel` of real params. We still reuse `vxn-core-app`'s `Controller`
//! (it owns the UI/view event channels + the GUI's preset corpus plumbing) with
//! a **zero-param** [`Vxn3Model`]; every edit travels through the
//! `UiEvent::Custom` escape hatch as a [`Vxn3UiCustom`], and [`tick_vxn3`]
//! translates it into an engine [`EngineCommand`] (data) or an engine swap
//! (heap-built on the main thread). Playhead state flows back as
//! [`Vxn3ViewCustom`].

use std::any::Any;
use std::path::{Path, PathBuf};

use vxn_core_app::params::ParamDesc;
use vxn_core_app::preset::{PresetLoad, PresetMeta, PresetStore, UserFolderEntry};
use vxn_core_app::{Controller, ParamId, ParamModel};
use vxn3_engine::io::EngineIo;
use vxn3_engine::{EngineCommand, EngineKind, N_TRACKS, make};

/// A preset store with nothing in it. vxn-3 has no preset system yet (deferred
/// breadth); the `Controller` still requires a store, so this satisfies it.
pub struct NullStore;

impl PresetStore for NullStore {
    fn factory_len(&self) -> usize {
        0
    }
    fn factory_load(&self, _index: usize) -> Result<PresetLoad, String> {
        Err("no presets".into())
    }
    fn factory_meta(&self, _index: usize) -> Option<PresetMeta> {
        None
    }
    fn user_load(&self, _path: &Path) -> Result<PresetLoad, String> {
        Err("no presets".into())
    }
    fn user_save(
        &self,
        _name: &str,
        _folder: Option<&str>,
        _meta: &PresetMeta,
        _blob: &[u8],
    ) -> Result<PathBuf, String> {
        Err("readonly".into())
    }
    fn user_delete(&self, _path: &Path) -> Result<(), String> {
        Err("readonly".into())
    }
    fn user_rename(&self, _path: &Path, _new_name: &str) -> Result<PathBuf, String> {
        Err("readonly".into())
    }
    fn user_move(&self, _path: &Path, _dest_folder: Option<&str>) -> Result<PathBuf, String> {
        Err("readonly".into())
    }
    fn user_create_folder(&self, _suggested: &str) -> Result<(PathBuf, String), String> {
        Err("readonly".into())
    }
    fn user_rename_folder(&self, _old: &str, _new: &str) -> Result<(PathBuf, String), String> {
        Err("readonly".into())
    }
    fn user_delete_folder(&self, _name: &str) -> Result<(), String> {
        Err("readonly".into())
    }
    fn list_user_tree(&self) -> Vec<UserFolderEntry> {
        Vec::new()
    }
}

/// Zero-param model. vxn-3 has no flat CLAP params (0052); all edits go through
/// the custom-event path, so every accessor is inert.
#[derive(Default)]
pub struct Vxn3Model;

impl ParamModel for Vxn3Model {
    fn total(&self) -> usize {
        0
    }
    fn get(&self, _id: ParamId) -> f32 {
        0.0
    }
    fn set(&self, _id: ParamId, _plain: f32) {}
    fn get_normalized(&self, _id: ParamId) -> f32 {
        0.0
    }
    fn set_normalized(&self, _id: ParamId, _norm: f32) {}
    fn gesture(&self, _id: ParamId) -> bool {
        false
    }
    fn set_gesture(&self, _id: ParamId, _on: bool) {}
    fn descriptor(&self, _id: ParamId) -> Option<&'static ParamDesc> {
        None
    }
    fn snapshot_bytes(&self) -> Vec<u8> {
        Vec::new()
    }
    fn restore_from_bytes(&self, _blob: &[u8]) -> Result<(), String> {
        Ok(())
    }
}

/// A structured UI edit (the `UiEvent::Custom` payload from the faceplate).
#[derive(Debug)]
pub enum Vxn3UiCustom {
    /// A data-only engine edit (grid cell, length, gain, pan, knob…).
    Edit(EngineCommand),
    /// Select a track's engine — built on the main thread, swapped in.
    SetEngine { track: u8, kind: EngineKind },
}

/// A view update pushed to the faceplate.
#[derive(Debug, Clone)]
pub enum Vxn3ViewCustom {
    /// Per-lane current step index (`u32::MAX` = stopped) + play state.
    Playhead {
        steps: [u32; N_TRACKS],
        playing: bool,
    },
}

/// Drive one controller tick: translate queued [`Vxn3UiCustom`] edits into
/// engine commands / swaps over the shared [`EngineIo`]. `sample_rate` is used
/// to build a freshly selected engine on the main thread (the audio thread also
/// re-applies the rate on install, so a stale value is harmless).
pub fn tick_vxn3(controller: &mut Controller<Vxn3Model>, io: &EngineIo, sample_rate: f32) {
    let mut on_ui = |_ctrl: &mut Controller<Vxn3Model>, payload: Box<dyn Any + Send>| {
        let Ok(boxed) = payload.downcast::<Vxn3UiCustom>() else {
            return;
        };
        match *boxed {
            Vxn3UiCustom::Edit(cmd) => {
                let _ = io.edits.push(cmd);
            }
            Vxn3UiCustom::SetEngine { track, kind } => {
                if let Some(swap) = io.swaps.get(track as usize) {
                    let _ = swap.send(make(kind, sample_rate));
                }
            }
        }
    };
    let mut on_host = |_: &mut Controller<Vxn3Model>, _: Box<dyn Any + Send>| {};
    controller.tick(&mut on_ui, &mut on_host);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use vxn3_engine::EngineCommand;
    use vxn3_engine::track_engine::TrackEngine;
    use vxn_core_app::UiEvent;

    fn controller() -> Controller<Vxn3Model> {
        let (ctrl, _rx, _corpus) = Controller::new(Arc::new(Vxn3Model), Box::new(NullStore));
        ctrl
    }

    #[test]
    fn edit_event_reaches_the_command_queue() {
        let mut ctrl = controller();
        let io = EngineIo::new();
        ctrl.handle()
            .post(UiEvent::Custom(Box::new(Vxn3UiCustom::Edit(
                EngineCommand::SetGain { track: 1, gain: 0.5 },
            ))))
            .unwrap();
        tick_vxn3(&mut ctrl, &io, 48_000.0);
        assert_eq!(
            io.edits.pop(),
            Some(EngineCommand::SetGain { track: 1, gain: 0.5 })
        );
    }

    #[test]
    fn set_engine_event_queues_a_swap() {
        let mut ctrl = controller();
        let io = EngineIo::new();
        ctrl.handle()
            .post(UiEvent::Custom(Box::new(Vxn3UiCustom::SetEngine {
                track: 2,
                kind: EngineKind::Noise,
            })))
            .unwrap();
        tick_vxn3(&mut ctrl, &io, 48_000.0);

        // The freshly built engine is waiting in track 2's swap mailbox.
        let mut active: Box<dyn TrackEngine> = make(EngineKind::KickTone, 48_000.0);
        assert!(io.swaps[2].try_install(&mut active));
        assert_eq!(active.kind(), EngineKind::Noise);
    }
}
