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
use vxn_core_app::{Controller, ParamId, ParamModel, ViewEvent};
use vxn3_engine::flavour::Flavour;
use vxn3_engine::io::EngineIo;
use vxn3_engine::{EngineCommand, EngineKind, N_TRACKS, Pattern, make};

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
    /// A data-only engine edit (hit, lane geometry, gain, pan, knob…).
    Edit(EngineCommand),
    /// Select a track's engine — built on the main thread, swapped in.
    SetEngine { track: u8, kind: EngineKind },
    /// Assign a **voice** (engine kind + flavour) to a lane (0185): update the
    /// main-thread flavour store, mirror the kind, and swap in a fresh engine with the
    /// flavour applied. The single edit path for the voice library — a voice edit
    /// re-sends this for every lane using the voice.
    AssignVoice { track: u8, kind: EngineKind, flavour: Flavour },
}

/// A view update pushed to the faceplate.
#[derive(Debug, Clone)]
pub enum Vxn3ViewCustom {
    /// Per-lane current subdivision-slot index (`u32::MAX` = stopped) + play state.
    Playhead {
        steps: [u32; N_TRACKS],
        playing: bool,
    },
    /// One lane of the model, re-announced to the view (ticket 0366) — its geometry
    /// and its hits, in fire order.
    ///
    /// Only sent when the model was **replaced** rather than edited: a state
    /// restore. Every other change to a lane came from the view in the first place,
    /// so the view already has it, and echoing those back would fight a drag in
    /// flight with a copy of what it had already drawn. The ordinary reopen case
    /// needs no event at all — the page is *built* from the model
    /// ([`vxn3_ui_web::build_html`]), so it opens in agreement rather than catching
    /// up.
    ///
    /// **Fire-order indices are positional and carry no stable identity**, and
    /// deliberately so (0348 re-sorts on insert and on any position or geometry
    /// edit). The view and the model resolve a fire time with the same arithmetic
    /// (0353), so their lists agree index for index as long as they *started* in
    /// agreement — which is what building the page from the model guarantees, and
    /// what this event restores when the model is replaced underneath it. Adding a
    /// stable per-hit id would change the wire form of every hit-keyed opcode to
    /// buy what starting in agreement already gives.
    ///
    /// It follows that applying one is a **hard resync**: it invalidates any index a
    /// gesture in flight is holding, so the view drops that gesture.
    ///
    /// `Box`ed because a [`Pattern`] is kilobytes and this rides the same channel as
    /// the per-tick playhead.
    Lane { track: u8, pattern: Box<Pattern> },
}

/// Drive one controller tick: translate queued [`Vxn3UiCustom`] edits into
/// model mutations + engine commands / swaps over the shared [`EngineIo`].
/// `sample_rate` is used to build a freshly selected engine on the main thread
/// (the audio thread also re-applies the rate on install, so a stale value is
/// harmless).
///
/// This is the **C** of vxn-3's MVC (0366): the view signals an edit here, the
/// controller writes it to the model and ships the same command to the audio
/// thread's copy, and the view reads the model back.
pub fn tick_vxn3(controller: &mut Controller<Vxn3Model>, io: &EngineIo, sample_rate: f32) {
    let mut on_ui = |_ctrl: &mut Controller<Vxn3Model>, payload: Box<dyn Any + Send>| {
        let Ok(boxed) = payload.downcast::<Vxn3UiCustom>() else {
            return;
        };
        match *boxed {
            // Queue first, then advance the model — and only if the queue took it.
            // A full ring drops the command (0052), and a model that had moved on
            // anyway would be a lane the engine never plays and the editor never
            // stops showing. Dropped means dropped, on both sides.
            Vxn3UiCustom::Edit(cmd) => {
                if io.edits.push(cmd) {
                    io.patterns.apply(cmd);
                }
            }
            Vxn3UiCustom::SetEngine { track, kind } => {
                if let Some(swap) = io.swaps.get(track as usize) {
                    let _ = swap.send(make(kind, sample_rate));
                    // Mirror the selection so the host's value-text reads it (0172).
                    io.kinds.set(track as usize, kind);
                }
            }
            Vxn3UiCustom::AssignVoice { track, kind, flavour } => {
                let t = track as usize;
                // Store the deep patch (for save + value-text), mirror the kind, and swap
                // in a fresh engine with the flavour applied (re-resolves at next trig).
                io.flavours.set(t, flavour.clone());
                io.kinds.set(t, kind);
                if let Some(swap) = io.swaps.get(t) {
                    let mut engine = make(kind, sample_rate);
                    engine.apply_flavour(flavour);
                    let _ = swap.send(engine);
                }
            }
        }
    };
    let mut on_host = |_: &mut Controller<Vxn3Model>, _: Box<dyn Any + Send>| {};
    // No post-load hook: vxn-3 has no preset system. A `clap.state` restore
    // announces itself through the model instead — see `announce_replaced_lanes`.
    let mut on_loaded = |_: &mut Controller<Vxn3Model>| {};
    controller.tick(&mut on_ui, &mut on_host, &mut on_loaded);
    announce_replaced_lanes(controller, io);
}

/// Tell the view about any lane the model **replaced** since the last tick (0366).
///
/// Replacement is the one change the view cannot already know about: every other
/// mutation of a lane originated as a `Vxn3UiCustom::Edit` from the view itself.
/// So this is silent on an ordinary tick, and speaks up exactly once per
/// [`vxn3_engine::PatternStore::set`] — which is what a state restore does.
fn announce_replaced_lanes(controller: &mut Controller<Vxn3Model>, io: &EngineIo) {
    for track in 0..N_TRACKS {
        if io.patterns.take_dirty(track) {
            controller.push_view_event(ViewEvent::Custom(Box::new(Vxn3ViewCustom::Lane {
                track: track as u8,
                pattern: Box::new(io.patterns.get(track)),
            })));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use vxn3_engine::EngineCommand;
    use vxn3_engine::track_engine::TrackEngine;
    use vxn_core_app::UiEvent;

    fn controller() -> Controller<Vxn3Model> {
        with_views().0
    }

    fn with_views() -> (Controller<Vxn3Model>, std::sync::mpsc::Receiver<vxn_core_app::ViewEvent>) {
        let (ctrl, rx, _corpus) = Controller::new(Arc::new(Vxn3Model), Box::new(NullStore));
        (ctrl, rx)
    }

    /// Every lane readback waiting on the view channel, as `(track, hit count)`.
    fn lane_readbacks(
        rx: &std::sync::mpsc::Receiver<vxn_core_app::ViewEvent>,
    ) -> Vec<(u8, usize)> {
        let mut out = Vec::new();
        while let Ok(vxn_core_app::ViewEvent::Custom(b)) = rx.try_recv() {
            if let Some(Vxn3ViewCustom::Lane { track, pattern }) =
                b.downcast_ref::<Vxn3ViewCustom>()
            {
                out.push((*track, pattern.len()));
            }
        }
        out
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

    /// AC (0366): the controller writes every lane edit to the model as well as to
    /// the engine's queue. That is what gives a reopened editor something to read.
    #[test]
    fn a_lane_edit_lands_in_the_model_and_on_the_queue() {
        let mut ctrl = controller();
        let io = EngineIo::new();
        let add = EngineCommand::AddHit {
            track: 3, beat: 2, sub: 1, f: 0.5, nudge: -4, y: 0.25, note: 41.0, velocity: 0.8,
        };
        ctrl.handle().post(UiEvent::Custom(Box::new(Vxn3UiCustom::Edit(add)))).unwrap();
        tick_vxn3(&mut ctrl, &io, 48_000.0);

        assert_eq!(io.edits.pop(), Some(add), "the engine's copy is told");
        let lane = io.patterns.get(3);
        assert_eq!(lane.len(), 1, "and so is the model");
        assert_eq!((lane.hits()[0].beat, lane.hits()[0].sub), (2, 1));
        assert_eq!(lane.hits()[0].nudge, -4);
        assert!(io.patterns.get(0).is_empty(), "other lanes untouched");
    }

    /// A dropped command is dropped on **both** sides. The model must not run ahead
    /// of an engine that never received the edit — that is a lane the editor shows
    /// and the engine never plays, and every later hit index would name a different
    /// hit on each side.
    #[test]
    fn an_edit_the_queue_rejects_does_not_advance_the_model() {
        let mut ctrl = controller();
        let io = EngineIo::new();
        while io.edits.can_push() {
            io.edits.push(EngineCommand::SetGain { track: 0, gain: 1.0 });
        }
        ctrl.handle()
            .post(UiEvent::Custom(Box::new(Vxn3UiCustom::Edit(EngineCommand::AddHit {
                track: 0, beat: 0, sub: 0, f: 0.0, nudge: 0, y: 0.5, note: 36.0, velocity: 1.0,
            }))))
            .unwrap();
        tick_vxn3(&mut ctrl, &io, 48_000.0);
        assert!(io.patterns.get(0).is_empty(), "model stays with the engine");
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
        // …and the main-thread kind mirror reflects the selection (0172).
        assert_eq!(io.kinds.get(2), EngineKind::Noise);
        assert_eq!(io.kinds.get(0), EngineKind::KickTone); // untouched default
    }

    #[test]
    fn assign_voice_updates_store_kind_and_swaps() {
        let mut ctrl = controller();
        let io = EngineIo::new();
        // A non-default Metal flavour with an edited base + macro name.
        let mut flav = vxn3_engine::default_flavour_for(EngineKind::Metal);
        flav.base[0] = 777.0;
        flav.macro_names[0] = "Ring".into();
        ctrl.handle()
            .post(UiEvent::Custom(Box::new(Vxn3UiCustom::AssignVoice {
                track: 4,
                kind: EngineKind::Metal,
                flavour: flav.clone(),
            })))
            .unwrap();
        tick_vxn3(&mut ctrl, &io, 48_000.0);

        // Kind mirror + flavour store updated, and a Metal engine queued for the swap.
        assert_eq!(io.kinds.get(4), EngineKind::Metal);
        assert_eq!(io.flavours.get(4), flav);
        let mut active: Box<dyn TrackEngine> = make(EngineKind::KickTone, 48_000.0);
        assert!(io.swaps[4].try_install(&mut active));
        assert_eq!(active.kind(), EngineKind::Metal);
    }

    /// AC (0366): a lane the model **replaced** — a state restore — is announced to
    /// the view once, so the page redraws it instead of holding a picture the model
    /// no longer agrees with.
    #[test]
    fn a_replaced_lane_is_announced_to_the_view() {
        let (mut ctrl, rx) = with_views();
        let io = EngineIo::new();

        // Nothing replaced: the view hears nothing.
        tick_vxn3(&mut ctrl, &io, 48_000.0);
        assert!(lane_readbacks(&rx).is_empty());

        // A restore writes the model, then flushes it down to the engine's copy.
        let mut p = Pattern::default();
        p.insert(vxn3_engine::Hit::at(1, 0));
        p.insert(vxn3_engine::Hit::at(0, 0));
        io.patterns.set(2, p);
        assert!(io.flush_lane(2));

        tick_vxn3(&mut ctrl, &io, 48_000.0);
        assert_eq!(lane_readbacks(&rx), vec![(2, 2)]);

        // Announced once, not every tick — the view is not a polling loop.
        tick_vxn3(&mut ctrl, &io, 48_000.0);
        assert!(lane_readbacks(&rx).is_empty());
    }

    /// An ordinary edit is never echoed back. The view sent it, so it already has
    /// it — and a copy arriving a tick later would fight a drag still in progress.
    #[test]
    fn ordinary_edits_are_not_echoed_to_the_view() {
        let (mut ctrl, rx) = with_views();
        let io = EngineIo::new();
        ctrl.handle()
            .post(UiEvent::Custom(Box::new(Vxn3UiCustom::Edit(EngineCommand::AddHit {
                track: 0, beat: 0, sub: 0, f: 0.0, nudge: 0, y: 0.5, note: 36.0, velocity: 1.0,
            }))))
            .unwrap();
        tick_vxn3(&mut ctrl, &io, 48_000.0);
        assert_eq!(io.patterns.get(0).len(), 1, "the model took the edit");
        assert!(lane_readbacks(&rx).is_empty(), "…and said nothing about it");
    }
}
