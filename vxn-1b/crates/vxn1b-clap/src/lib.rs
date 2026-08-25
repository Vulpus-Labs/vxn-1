//! VXN1b CLAP plugin shell (clack) — params, state, MPE event routing (0204).
//!
//! Wires [`vxn1b_engine::Engine`] to CLAP with **host-generic knobs**: a stereo
//! output, a CLAP+MIDI note input, the flat param table (incl. the 16 matrix
//! slot depths), and `clap.state` save/restore. There is no faceplate yet — the
//! HTML editor + its controller land in E038; the E036 bar is "playable in a
//! DAW with the host's generic parameter UI".
//!
//! **Threading.** [`SharedParams`] is the lock-free crossing: the audio thread
//! writes host automation into it as events arrive; the main thread reads it for
//! the `params` extension (`get_value`) and serialises it for `state.save`. A
//! `state.load` writes the store and raises its reload flag, which the audio
//! thread observes at the top of `process` to re-sync the engine — the path for
//! a preset that lands while the plugin is active.
//!
//! **MPE.** The shared `vxn-core-clap` note dispatch is channel-agnostic, so the
//! event routing here is bespoke: note-on/off carry their MIDI channel into the
//! allocator, CLAP note-expression *pressure* and MIDI poly-key-pressure (0xA0)
//! become per-note pressure, and channel pressure (0xD0) broadcasts — the 0198
//! per-voice pressure spine. Pitch-bend drives the hardwired global bend
//! (ADR 0001 §3), not a matrix route.

mod gui;

use clack_extensions::gui::PluginGui;
use clack_extensions::state::{PluginState, PluginStateImpl};
use clack_extensions::timer::{HostTimer, PluginTimer, PluginTimerImpl, TimerId};
use clack_extensions::{audio_ports::*, note_ports::*, params::*};
use clack_plugin::events::event_types::NoteExpressionType;
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::events::{Match, UnknownEvent};
use clack_plugin::prelude::*;
use clack_plugin::stream::{InputStream, OutputStream};
use std::ffi::CStr;
use std::io::{Read, Write};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use vxn_core_app::{Controller, CorpusHandle, ParamId as AppParamId, ParamKind, ViewEvent};
use vxn_core_clap::{LocalParams, SharedStore, batch_range};
use vxn1b_engine::{
    Engine, EnginePresetStore, MeterBus, MeterFrame, ScopeBus, ScopeFrame, ScopeTap, SharedParams,
    TOTAL_PARAMS, clap_module, desc_for_clap_id,
};
use vxn1b_engine::sync::{rate_partner_clap_id, sync_aware_display};

/// Locks a poisoned mutex by extracting the inner value instead of unwrapping.
/// Plugin code unwinds on panic, so a panic during `tick` could poison the
/// controller's outer mutex; we don't want every subsequent flush to fail. The
/// guarded data is still valid (mid-write at worst). Used by [`gui`] too.
pub(crate) fn lock_mut<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A CLAP channel `Match` narrowed to a concrete MIDI channel; `All`/wildcard
/// (an omni or non-MPE host) folds to channel 0 — the degenerate single-channel
/// case the allocator already handles.
#[inline]
fn channel_of(m: Match<u16>) -> u8 {
    match m {
        Match::Specific(c) => c as u8,
        _ => 0,
    }
}

/// Adapts the engine's [`SharedParams`] to the shared
/// [`vxn_core_clap::SharedStore`] trait the generic [`LocalParams`] is written
/// against. Orphan rules forbid `impl SharedStore for SharedParams` here (both
/// are foreign to this crate), so the audio thread wraps a shared ref per call.
/// Forwards `get`/`set` plus the live UI-gesture flag so [`LocalParams::emit`]
/// brackets a knob drag into a single host automation edit.
struct StoreRef<'a>(&'a SharedParams);

impl SharedStore for StoreRef<'_> {
    #[inline]
    fn get(&self, id: usize) -> f32 {
        self.0.get(id)
    }
    #[inline]
    fn set(&self, id: usize, value: f32) {
        self.0.set(id, value)
    }
    #[inline]
    fn gesture(&self, id: usize) -> bool {
        self.0.gesture(id)
    }
}

/// Route one CLAP event onto the engine. Bespoke rather than the shared
/// `dispatch_event` because VXN1b threads MIDI channel + per-note pressure the
/// shared `EngineNotes` surface can't carry. `ParamValue` is *not* handled here
/// — the audio thread folds param writes through [`LocalParams`] instead.
fn dispatch(engine: &mut Engine, event: &UnknownEvent) {
    match event.as_core_event() {
        Some(CoreEventSpace::NoteOn(e)) => {
            if let Match::Specific(key) = e.key() {
                engine.note_on(channel_of(e.channel()), key as u8, e.velocity() as f32);
            }
        }
        Some(CoreEventSpace::NoteOff(e)) => {
            if let Match::Specific(key) = e.key() {
                engine.note_off(channel_of(e.channel()), key as u8);
            }
        }
        // Per-note (MPE) pressure → the matching voice (0198).
        Some(CoreEventSpace::NoteExpression(e))
            if e.expression_type() == Some(NoteExpressionType::Pressure) =>
        {
            if let Match::Specific(key) = e.key() {
                engine.poly_pressure(channel_of(e.channel()), key as u8, e.value() as f32);
            }
        }
        Some(CoreEventSpace::Midi(e)) => {
            let [status, d1, d2] = e.data();
            let ch = status & 0x0F;
            match status & 0xF0 {
                // 0x90 vel 0 is a note-off by MIDI convention. (note_on returns
                // the voice index — discarded, so every arm is `()`.)
                0x90 if d2 > 0 => {
                    engine.note_on(ch, d1, d2 as f32 / 127.0);
                }
                0x80 | 0x90 => engine.note_off(ch, d1),
                // Poly key pressure → per-note pressure.
                0xA0 => engine.poly_pressure(ch, d1, d2 as f32 / 127.0),
                // Channel pressure → broadcast to the channel's voices.
                0xD0 => engine.channel_pressure(ch, d1 as f32 / 127.0),
                0xE0 => {
                    // 14-bit bend, centre 8192 → normalised [-1, 1].
                    let raw = ((d2 as u16) << 7) | d1 as u16;
                    engine.set_pitch_bend((raw as f32 - 8192.0) / 8192.0);
                }
                0xB0 if d1 == 1 => {
                    // CC1 mod wheel; deadzone the bottom LSB.
                    let wheel = if d2 <= 1 { 0.0 } else { d2 as f32 / 127.0 };
                    engine.set_mod_wheel(wheel);
                }
                _ => {}
            }
        }
        _ => {}
    }
}

pub struct VxnPlugin;

impl Plugin for VxnPlugin {
    type AudioProcessor<'a> = VxnAudioProcessor<'a>;
    type Shared<'a> = VxnShared;
    type MainThread<'a> = VxnMainThread<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&VxnShared>) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginNotePorts>()
            .register::<PluginParams>()
            .register::<PluginState>()
            .register::<PluginGui>()
            .register::<PluginTimer>();
    }
}

impl DefaultPluginFactory for VxnPlugin {
    fn get_descriptor() -> PluginDescriptor {
        use clack_plugin::plugin::features::*;
        PluginDescriptor::new("labs.vulpus.vxn1b", "VXN-1b").with_features([
            INSTRUMENT,
            SYNTHESIZER,
            STEREO,
        ])
    }

    fn new_shared(_host: HostSharedHandle) -> Result<VxnShared, PluginError> {
        Ok(VxnShared {
            params: Arc::new(SharedParams::new()),
            meters: Arc::new(MeterBus::new()),
            scope: Arc::new(ScopeBus::new()),
        })
    }

    fn new_main_thread<'a>(
        host: HostMainThreadHandle<'a>,
        shared: &'a VxnShared,
    ) -> Result<VxnMainThread<'a>, PluginError> {
        // The editor's controller (E038) shares the same `Arc<SharedParams>` as
        // the audio path, so a UI edit and host automation land in one store.
        let (controller, view_rx, corpus) =
            Controller::new(shared.params.clone(), Box::new(EnginePresetStore::new()));
        Ok(VxnMainThread {
            host,
            shared,
            controller: Arc::new(Mutex::new(controller)),
            view_rx: Arc::new(Mutex::new(view_rx)),
            corpus,
            gui: None,
            timer: None,
            last_seen: vec![f32::NAN; TOTAL_PARAMS],
            last_matrix: None,
            last_key: None,
            meters_idle: true,
            scope_tick: 0,
            scope_idle: true,
        })
    }
}

/// State shared between the main and audio threads: the lock-free param store
/// behind an `Arc` so the (future) editor can hold a clone too, plus the meter
/// bus the audio thread publishes into and the editor timer drains.
pub struct VxnShared {
    params: Arc<SharedParams>,
    /// Meter bus (0240). Lives here rather than inside the `Engine` because it
    /// must outlive a deactivate/reactivate cycle — `activate` rebuilds the
    /// engine, and the main thread's drain must keep reading the same slots.
    meters: Arc<MeterBus>,
    /// Oscilloscope capture ring. Here for the same reason as `meters`, plus
    /// one of its own: the selected tap is written by the **main** thread (the
    /// faceplate's `set_scope_source`) and read by the audio thread, so the ring
    /// has to be reachable from both without going through the engine.
    scope: Arc<ScopeBus>,
}

impl PluginShared<'_> for VxnShared {}

/// Main-thread state. Beyond the shared param store it now owns the editor's
/// [`Controller`] (the sole non-audio model mutator), the view-event channel it
/// emits, the shared preset corpus, the live editor handle, and the host timer
/// that drives the view-event drain (E038).
pub struct VxnMainThread<'a> {
    /// Host main-thread handle. `gui::set_parent` / `on_timer` use it to
    /// register / unregister the periodic timer.
    host: HostMainThreadHandle<'a>,
    shared: &'a VxnShared,
    /// The editor's controller, wrapped so the timer drain and the params
    /// `flush` path share one instance without crossing thread boundaries
    /// (both are main-thread, so no real contention).
    controller: Arc<Mutex<Controller<SharedParams>>>,
    /// View-bound events the controller emits; the timer drains them into the
    /// WebView. Stay queued (bounded, drop-on-full) while the GUI is closed.
    view_rx: Arc<Mutex<Receiver<ViewEvent>>>,
    /// Shared snapshot of the preset corpus for the editor's browser panel;
    /// the controller republishes it after every disk op.
    corpus: CorpusHandle,
    /// The live editor window, while the GUI is open.
    gui: Option<vxn1b_ui_web::EditorHandle>,
    /// Editor's main-thread timer (the host's extension + id), driving the
    /// view-event drain. `None` when the GUI is closed or the host has no
    /// `timer-support`.
    timer: Option<(HostTimer, TimerId)>,
    /// Last param values seen by the diff pump. The audio thread writes
    /// [`SharedParams`] directly on host automation without routing through the
    /// controller, so the editor would otherwise never see it. Each tick diffs
    /// the store against this vector and pushes a `ParamChanged` for any drift.
    /// Seeded all-`NaN` so the first tick after open broadcasts the whole table.
    last_seen: Vec<f32>,
    /// Last matrix topology pushed to the page (0247). `None` until the first
    /// tick with the GUI open, which forces one push — the page was seeded at
    /// editor-open (0246), but a load landing between that splice and the
    /// page's first tick would otherwise slip through unnoticed.
    last_matrix: Option<[vxn1b_engine::MatrixTable; 2]>,
    /// Last keyboard state pushed to the page (0221). Same contract as
    /// `last_matrix`: `None` while the GUI is closed, so the next open
    /// re-pushes, and cleared again on `EditorReady` — the GUI handle exists
    /// from `set_parent`, but the page's `applyViewEvents` only exists from the
    /// `ready` handshake, so a push in between is dropped on the floor.
    last_key: Option<vxn1b_engine::KeyState>,
    /// Whether the previous tick's meter frame was all-zero (0240). Lets the
    /// drain push the *first* silent frame — the view needs that zero to start
    /// its decay — then go quiet until there is signal again, so an idle plugin
    /// costs nothing on the bridge.
    meters_idle: bool,
    /// Ticks since the last scope frame. The scope is pushed every other tick
    /// (~30 Hz): a trace is several hundred samples of JSON where a meter frame
    /// is eleven numbers, and 30 Hz is already past the point where a moving
    /// waveform reads as continuous.
    scope_tick: u8,
    /// Whether the previous scope frame was all-zero. Same idle suppression as
    /// `meters_idle`: one flat frame settles the trace on the centre line, then
    /// silence costs nothing.
    scope_idle: bool,
}

impl<'a> PluginMainThread<'a, VxnShared> for VxnMainThread<'a> {}

impl<'a> VxnMainThread<'a> {
    /// Drain the controller's view-event queue into the live WebView. No-op
    /// when there is no GUI.
    fn drain_view_events(&mut self) {
        let Some(handle) = self.gui.as_ref() else {
            return;
        };
        let rx = lock_mut(&self.view_rx);
        while let Ok(ev) = rx.try_recv() {
            handle.push_view_event(ev);
        }
    }

    /// Diff the shared store against `last_seen` and push a `ParamChanged` for
    /// any drift. This catches audio-thread automation: `process()` writes
    /// `SharedParams` directly, so the controller's view queue stays empty for
    /// those changes. NaN-aware (a NaN seed forces a full first broadcast).
    /// A sync toggle that flips also re-pushes its rate/time partner (0267): the
    /// partner's *value* hasn't changed, but its display flips Hz/s ↔
    /// subdivision, and the faceplate repaints from what it is sent.
    fn push_param_diffs(&mut self) {
        let Some(handle) = self.gui.as_ref() else {
            return;
        };
        let store = &*self.shared.params;
        for (id, seen) in self.last_seen.iter_mut().enumerate() {
            let plain = store.get(id);
            if plain == *seen {
                continue;
            }
            *seen = plain;
            handle.push_view_event(ViewEvent::ParamChanged {
                id: AppParamId::new(id),
                plain,
                norm: store.get_normalized(id),
                display: sync_aware_display(store, id, plain),
            });
            // Sync flag flipped: re-push the partnered rate/time so its readout
            // switches between Hz/seconds and the subdivision label.
            if let Some(rate_id) = rate_partner_clap_id(id) {
                let rate_plain = store.get(rate_id);
                handle.push_view_event(ViewEvent::ParamChanged {
                    id: AppParamId::new(rate_id),
                    plain: rate_plain,
                    norm: store.get_normalized(rate_id),
                    display: sync_aware_display(store, rate_id, rate_plain),
                });
            }
        }
    }

    /// Push the matrix topology to the page when it changes (0247).
    ///
    /// Topology is not a CLAP param, so none of the param machinery carries it:
    /// a preset load, a host `state.load`, or a host undo all rewrite the shared
    /// store's tables with nothing echoing to an open editor, leaving the
    /// source/dest combos showing the previous patch. This diffs the store
    /// against what the page was last told and pushes a `MatrixSnapshot` on any
    /// drift.
    ///
    /// Edits made *by* the page come back through here too. That is intended
    /// belt-and-braces — the combo is already showing the value it posted, so
    /// reflecting it is a no-op — and it keeps this a plain "store is the truth"
    /// diff rather than an origin-tracking scheme.
    ///
    /// Comparison is 32 slot structs; at the 60 Hz tick that is noise next to
    /// the param diff loop already running beside it.
    fn push_matrix_echo(&mut self) {
        let Some(handle) = self.gui.as_ref() else {
            // Editor closed: drop the memo so the next open re-pushes rather
            // than trusting a snapshot the new page never received.
            self.last_matrix = None;
            return;
        };
        let live = self.shared.params.matrix_snapshot();
        if self.last_matrix == Some(live) {
            return;
        }
        self.last_matrix = Some(live);
        handle.push_view_event(ViewEvent::Custom(Box::new(vxn1b_engine::MatrixSnapshot {
            layers: live,
        })));
    }

    /// Push the keyboard state to the page when it changes (0221).
    ///
    /// Exactly the topology echo's problem one type over: `KeyState` is not a
    /// CLAP param (ADR 0002 §3), so a preset load, a host `state.load`, or a
    /// host undo can turn Layer 2 on or move the split with nothing telling an
    /// open editor about it — the Layer 2 switch and the split row would keep
    /// showing the previous patch. Now that the blob carries the record, this
    /// diffs it against what the page was last told and pushes on any drift.
    ///
    /// Reads via `key_state` rather than `take_key_state`: the dirty flag
    /// belongs to the audio thread's engine re-sync, and consuming it here would
    /// silently drop a load's routing change whenever a tick beat `process`.
    fn push_key_echo(&mut self) {
        let Some(handle) = self.gui.as_ref() else {
            self.last_key = None;
            return;
        };
        let live = self.shared.params.key_state();
        if self.last_key == Some(live) {
            return;
        }
        self.last_key = Some(live);
        handle.push_view_event(ViewEvent::Custom(Box::new(live)));
    }

    /// Drain the meter bus into one `ViewEvent::Custom(MeterFrame)` (0240).
    ///
    /// Runs only with the GUI open, so a closed editor pays nothing — the audio
    /// thread's publish is a few atomics either way, but nothing is read,
    /// serialised, or pushed. Because the drain clears the bus, skipping it
    /// while closed also means the first frame after re-opening reports the
    /// interval since that open, not a stale peak from before it.
    ///
    /// Idle suppression: an all-zero frame is pushed **once** — the view needs
    /// that zero to start its decay — and then withheld until signal returns.
    /// Without this a silent plugin would ship 60 identical frames a second.
    fn push_meter_frame(&mut self) {
        let Some(handle) = self.gui.as_ref() else {
            // Editor closed: leave the bus alone. It self-limits (each slot
            // holds one peak), so there is nothing to drain for hygiene.
            return;
        };
        let frame = MeterFrame::drain(&self.shared.meters);
        let silent = frame.is_silent();
        if silent && self.meters_idle {
            return;
        }
        self.meters_idle = silent;
        handle.push_view_event(ViewEvent::Custom(Box::new(frame)));
    }

    /// Read one window out of the scope ring into a `ViewEvent::Custom`.
    ///
    /// Three gates, cheapest first: no editor, no tap selected (the FX/Global
    /// tab, or a page that never asked), and not this tick's turn. With the
    /// scope off screen the whole feature costs one atomic load per audio block
    /// and nothing at all here.
    fn push_scope_frame(&mut self) {
        let Some(handle) = self.gui.as_ref() else {
            // Editor closed: stop capturing. Re-opening re-points the ring from
            // the page's first tab sync, so nothing is lost, and a closed
            // editor leaves the audio thread's publish a single load-and-return.
            self.shared.scope.set_source(ScopeTap::Off.code());
            return;
        };
        if self.shared.scope.source() == ScopeTap::Off.code() {
            return;
        }
        self.scope_tick = self.scope_tick.wrapping_add(1);
        if self.scope_tick % SCOPE_TICK_DIVISOR != 0 {
            return;
        }
        // `None` while the ring is still filling — after an open, a tab flip, or
        // a sample-rate change. Nothing to say yet, so say nothing.
        let Some(frame) = ScopeFrame::read(&self.shared.scope) else {
            return;
        };
        let silent = frame.is_silent();
        if silent && self.scope_idle {
            return;
        }
        self.scope_idle = silent;
        handle.push_view_event(ViewEvent::Custom(Box::new(frame)));
    }
}

/// Push a scope frame every Nth timer tick. The timer runs at ~60 Hz, so 2 puts
/// the trace at ~30 Hz.
const SCOPE_TICK_DIVISOR: u8 = 2;

impl<'a> PluginTimerImpl for VxnMainThread<'a> {
    fn on_timer(&mut self, _id: TimerId) {
        // Pull UI-posted intents into the model first so the ViewEvents they
        // generate land in `view_rx` before we drain it — saves a tick of
        // round-trip latency on a knob drag. Custom UI ops (0219: the Layer 2
        // key-mode / split-point) are applied to the shared KeyState channel;
        // the audio thread re-syncs the engine from it on the next `process`.
        let sink = self.shared.params.clone();
        let scope = self.shared.scope.clone();
        let mut on_custom_ui = move |_ctrl: &mut _, payload: Box<dyn std::any::Any + Send>| {
            // Four vxn1b custom payloads share this hook: a KeyOp (Layer 2
            // enable / split), a MatrixEdit (topology), a PatchOp (bulk
            // patch duplication, 0265), or a ScopeOp (which signal the
            // oscilloscope captures). Try each; downcast hands the box back
            // on a miss.
            let payload = match payload.downcast::<vxn1b_engine::KeyOp>() {
                Ok(op) => return sink.apply_key_op(*op),
                Err(p) => p,
            };
            let payload = match payload.downcast::<vxn1b_engine::MatrixEdit>() {
                Ok(edit) => return sink.edit_matrix_slot(*edit),
                Err(p) => p,
            };
            let payload = match payload.downcast::<vxn1b_engine::PatchOp>() {
                Ok(op) => {
                    match *op {
                        vxn1b_engine::PatchOp::CopyLayer { from, to } => sink.copy_layer(from, to),
                    }
                    return;
                }
                Err(p) => p,
            };
            // The tap goes straight onto the shared ring rather than through
            // the param store: it is view state (which panel is on screen), so
            // it must not touch the patch, the state blob or the undo stack.
            if let Ok(op) = payload.downcast::<vxn1b_engine::ScopeOp>() {
                match *op {
                    vxn1b_engine::ScopeOp::SetTap(tap) => scope.set_source(tap.code()),
                }
            }
        };
        let editor_ready = {
            let mut ctrl = lock_mut(&self.controller);
            ctrl.tick(&mut on_custom_ui, &mut |_, _| {}, &mut |_| {});
            ctrl.take_editor_ready_flag()
        };
        // The page just (re)attached: its JS only exists from `ready` onwards,
        // so anything pushed between `set_parent` and that handshake went into
        // a WebView with no `applyViewEvents` yet and was lost. Params survive
        // it — `EditorReady` re-broadcasts the whole table — but the two
        // non-param echoes are diff-gated, so a dropped push would stay dropped
        // until the state changed again: a session reloaded with Layer 2 on
        // would play dual while the switch read dark. Clearing the memos makes
        // the pushes below unconditional for this tick.
        if editor_ready {
            self.last_matrix = None;
            self.last_key = None;
        }
        self.drain_view_events();
        // Then catch any audio-thread automation the controller never saw. The
        // two pushes can echo the same param twice in a tick; the WebView
        // dedupes ParamChanged by id in `flush_view_events`, so the overlap is
        // free on the wire.
        self.push_param_diffs();
        // Topology echo (0247) — cheap, and only pushes on a real change.
        self.push_matrix_echo();
        // Keyboard echo (0221) — the same, for the state the blob now carries.
        self.push_key_echo();
        // Meters (0240) join the same batch — no extra bridge call.
        self.push_meter_frame();
        // The scope trace rides it too, at half the tick rate.
        self.push_scope_frame();
        // One `evaluate_script` per tick: the pushes above only buffered into
        // the EditorHandle; this is the single bridge call.
        if let Some(handle) = self.gui.as_ref() {
            handle.flush_view_events();
        }
    }
}

/// Audio-thread processor: the engine + render scratch. Syncs its params +
/// topology from the shared store on activate and whenever a state load raises
/// the reload flag.
pub struct VxnAudioProcessor<'a> {
    engine: Engine,
    shared: &'a VxnShared,
    /// Per-block mirror of the param table: folds UI edits (via the shared
    /// store) and sub-block host automation, pushes the working values into the
    /// engine each block, and echoes UI edits back to the host.
    local: LocalParams<TOTAL_PARAMS>,
    scratch_l: Vec<f32>,
    scratch_r: Vec<f32>,
    /// Last known transport playing state (0267) — a stop→play edge realigns the
    /// synced LFO 2s to the bar grid.
    was_playing: bool,
}

impl<'a> PluginAudioProcessor<'a, VxnShared, VxnMainThread<'a>> for VxnAudioProcessor<'a> {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &mut VxnMainThread<'a>,
        shared: &'a VxnShared,
        audio_config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        let max = audio_config.max_frames_count as usize;
        let mut engine = Engine::new(audio_config.sample_rate as f32, max);
        // Publish meters into the plugin-lifetime bus (0240), not the engine's
        // own — this `Engine` is discarded on deactivate, and the editor's drain
        // handle must survive that.
        engine.set_meters(shared.meters.clone());
        // Same lifetime argument for the scope ring — and it also carries the
        // player's selected tap, which must survive a sample-rate change.
        engine.set_scope(shared.scope.clone());
        // Adopt whatever the store holds (factory default, or a state loaded
        // while the plugin was inactive). Clears any stale reload flag.
        shared.params.take_reload();
        engine.load_state(shared.params.engine_state());
        Ok(Self {
            engine,
            // Seed the mirror from the store *after* `load_state`, so it starts
            // aligned with the active patch (no spurious first-block echo).
            local: LocalParams::new(&StoreRef(&shared.params)),
            shared,
            scratch_l: vec![0.0; max],
            scratch_r: vec![0.0; max],
            was_playing: false,
        })
    }

    fn process(
        &mut self,
        process: Process,
        mut audio: Audio,
        events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        // Host transport → engine tempo for the synced LFO rates and delay time
        // (0267). Take the BPM only when the transport actually carries one;
        // otherwise the engine keeps its sane default and never sees a NaN. A
        // stop→play edge realigns a synced LFO 2 to the bar grid.
        let is_playing = process
            .transport
            .map(vxn_core_clap::playing_from_transport)
            .unwrap_or(false);
        if !self.was_playing && is_playing {
            self.engine.on_transport_restart();
        }
        self.was_playing = is_playing;
        if let Some(bpm) = process.transport.and_then(vxn_core_clap::tempo_from_transport) {
            self.engine.set_tempo(bpm as f32);
        }

        // A state/preset load that landed while active: re-sync the whole patch.
        if self.shared.params.take_reload() {
            self.engine.load_state(self.shared.params.engine_state());
        }
        // A Layer 2 key-mode / split edit from the UI (0219): apply the new
        // KeyState so the demux enables/bypasses synth 2 and routes note-ons.
        if let Some(key) = self.shared.params.take_key_state() {
            self.engine.set_key_state(key);
        }

        // Fold UI edits made since the last process into the local mirror, then
        // drive the engine from the working values (UI + last host state). This
        // is the path that makes faceplate knob edits audible.
        self.local.fetch_ui_changes(&StoreRef(&self.shared.params));
        {
            let engine = &mut self.engine;
            for (i, &v) in self.local.values().iter().enumerate() {
                engine.set_param(i, v);
            }
        }

        let mut output_port = audio
            .output_port(0)
            .ok_or(PluginError::Message("No output port"))?;
        let mut out = output_port
            .channels()?
            .into_f32()
            .ok_or(PluginError::Message("Expected f32 output"))?;

        let frames = (out.frames_count() as usize).min(self.scratch_l.len());
        let nch = out.channel_count() as usize;
        if nch == 0 {
            return Err(PluginError::Message("Expected ≥1 output channel"));
        }

        let engine = &mut self.engine;
        let local = &mut self.local;
        let l = &mut self.scratch_l[..frames];
        let r = &mut self.scratch_r[..frames];

        // Sub-block accurate: apply each event at its batch boundary, render the
        // segment up to the next one. ParamValue folds through the local mirror
        // (so host automation and UI edits reconcile); everything else routes
        // through the bespoke note/MIDI dispatcher.
        for event_batch in events.input.batch() {
            for event in event_batch.events() {
                if let Some(CoreEventSpace::ParamValue(_)) = event.as_core_event() {
                    if let Some((idx, value)) = local.apply_input(event) {
                        engine.set_param(idx, value);
                    }
                } else {
                    dispatch(engine, event);
                }
            }
            let (start, end) = batch_range(event_batch.sample_bounds(), frames);
            if start < end {
                engine.process_block(&mut l[start..end], &mut r[start..end]);
            }
        }

        if let Some(ch) = out.channel_mut(0) {
            let n = ch.len().min(frames);
            ch[..n].copy_from_slice(&self.scratch_l[..n]);
        }
        if nch >= 2 {
            if let Some(ch) = out.channel_mut(1) {
                let n = ch.len().min(frames);
                ch[..n].copy_from_slice(&self.scratch_r[..n]);
            }
        }

        // Fold host automation back into the shared store (so the UI/host see
        // it) and echo UI edits to the host as gesture-bracketed param events.
        self.local.publish(&StoreRef(&self.shared.params));
        self.local
            .emit(&StoreRef(&self.shared.params), events.output, frames as u32);

        Ok(ProcessStatus::Continue)
    }

    fn reset(&mut self) {
        self.engine.reset();
    }
}

// ── Audio / Note ports ──────────────────────────────────────────────────────

impl PluginAudioPortsImpl for VxnMainThread<'_> {
    fn count(&mut self, is_input: bool) -> u32 {
        if is_input { 0 } else { 1 }
    }

    fn get(&mut self, index: u32, is_input: bool, writer: &mut AudioPortInfoWriter) {
        if !is_input && index == 0 {
            writer.set(&AudioPortInfo {
                id: ClapId::new(1),
                name: b"main",
                channel_count: 2,
                flags: AudioPortFlags::IS_MAIN,
                port_type: Some(AudioPortType::STEREO),
                in_place_pair: None,
            });
        }
    }
}

impl PluginNotePortsImpl for VxnMainThread<'_> {
    fn count(&mut self, is_input: bool) -> u32 {
        if is_input { 1 } else { 0 }
    }

    fn get(&mut self, index: u32, is_input: bool, writer: &mut NotePortInfoWriter) {
        if is_input && index == 0 {
            writer.set(&NotePortInfo {
                id: ClapId::new(1),
                name: b"main",
                preferred_dialect: Some(NoteDialect::Clap),
                supported_dialects: NoteDialects::CLAP | NoteDialects::MIDI,
            });
        }
    }
}

// ── Parameters ──────────────────────────────────────────────────────────────

impl PluginMainThreadParams for VxnMainThread<'_> {
    fn count(&mut self) -> u32 {
        TOTAL_PARAMS as u32
    }

    fn get_info(&mut self, param_index: u32, info: &mut ParamInfoWriter) {
        let idx = param_index as usize;
        let Some(desc) = desc_for_clap_id(idx) else {
            return;
        };
        let mut flags = ParamInfoFlags::IS_AUTOMATABLE;
        // Enum/bool/int params are stepped; floats are continuous.
        if !matches!(desc.kind, ParamKind::Float { .. }) {
            flags |= ParamInfoFlags::IS_STEPPED;
        }
        info.set(&ParamInfo {
            id: ClapId::new(idx as u32),
            flags,
            cookie: Default::default(),
            name: desc.label.as_bytes(),
            // Two-layer surface (0216): Layer 1/2 share a label, so the host tells
            // them apart by module — "Upper"/"Lower" (globals stay ungrouped).
            module: clap_module(idx).as_bytes(),
            min_value: desc.min as f64,
            max_value: desc.max as f64,
            default_value: desc.default as f64,
        });
    }

    fn get_value(&mut self, param_id: ClapId) -> Option<f64> {
        let idx = param_id.get() as usize;
        (idx < TOTAL_PARAMS).then(|| self.shared.params.get(idx) as f64)
    }

    fn value_to_text(
        &mut self,
        param_id: ClapId,
        value: f64,
        writer: &mut ParamDisplayWriter,
    ) -> std::fmt::Result {
        use std::fmt::Write as _;
        let id = param_id.get() as usize;
        if desc_for_clap_id(id).is_none() {
            return Err(std::fmt::Error);
        }
        // Synced rate/time params read out as their subdivision label (0267), so
        // the host's value display matches the editor's popup. Same helper the
        // `ParamChanged` broadcast uses, so the two can't disagree.
        write!(writer, "{}", sync_aware_display(&self.shared.params, id, value as f32))
    }

    fn text_to_value(&mut self, param_id: ClapId, text: &CStr) -> Option<f64> {
        let desc = desc_for_clap_id(param_id.get() as usize)?;
        desc.parse(text.to_str().ok()?).map(|v| v as f64)
    }

    fn flush(&mut self, input: &InputEvents, _output: &mut OutputEvents) {
        // Main-thread flush (plugin inactive): fold host automation into the
        // store. The engine picks it up on the next `activate` (which always
        // re-syncs from the store), so no reload flag is needed here.
        for event in input {
            if let Some(CoreEventSpace::ParamValue(e)) = event.as_core_event() {
                if let Some(pid) = e.param_id() {
                    self.shared.params.set(pid.get() as usize, e.value() as f32);
                }
            }
        }
    }
}

impl PluginAudioProcessorParams for VxnAudioProcessor<'_> {
    fn flush(&mut self, input: &InputEvents, _output: &mut OutputEvents) {
        for event in input {
            // Param writes fold through the mirror; notes/MIDI fall through to
            // the bespoke dispatcher (`apply_input` returns None for them).
            if let Some((idx, value)) = self.local.apply_input(event) {
                self.engine.set_param(idx, value);
            } else {
                dispatch(&mut self.engine, event);
            }
        }
        self.local.publish(&StoreRef(&self.shared.params));
    }
}

// ── State save / restore ────────────────────────────────────────────────────

impl PluginStateImpl for VxnMainThread<'_> {
    fn save(&mut self, output: &mut OutputStream) -> Result<(), PluginError> {
        let blob = self.shared.params.snapshot_bytes();
        output
            .write_all(&blob)
            .map_err(|_| PluginError::Message("state save failed"))
    }

    fn load(&mut self, input: &mut InputStream) -> Result<(), PluginError> {
        let mut blob = Vec::new();
        input
            .read_to_end(&mut blob)
            .map_err(|_| PluginError::Message("state read failed"))?;
        // Empty / undecodable → report failure without touching the store
        // (clap-validator `state-invalid`, 0196).
        self.shared
            .params
            .restore_from_bytes(&blob)
            .map_err(|_| PluginError::Message("invalid state"))
    }
}

clack_export_entry!(SinglePluginEntry<VxnPlugin>);

// Keep the param count referenced so a thin-LTO cdylib never drops the table.
#[used]
static _PARAM_COUNT: usize = TOTAL_PARAMS;

#[cfg(test)]
mod tests {
    use super::*;
    use clack_plugin::events::Pckn;
    use clack_plugin::events::event_types::{NoteOnEvent, ParamValueEvent};
    use clack_plugin::utils::Cookie;
    use vxn1b_engine::{Layer, ParamId, clap_id_of};

    /// Layer-1 CLAP id for an inner param — the CLAP surface is the two-layer map
    /// (0216), so a test that means "layer 1's X" resolves it, not `X as usize`.
    fn l1(p: ParamId) -> usize {
        clap_id_of(Layer::L1, p)
    }

    fn param_event(id: ParamId, value: f64) -> ParamValueEvent {
        ParamValueEvent::new(
            0,
            ClapId::new(l1(id) as u32),
            Pckn::match_all(),
            value,
            Cookie::empty(),
        )
    }

    fn note_on(channel: u16, key: u16, vel: f64) -> NoteOnEvent {
        NoteOnEvent::new(
            0,
            Pckn::new(Match::All, Match::Specific(channel), Match::Specific(key), Match::All),
            vel,
        )
    }

    fn peak(engine: &mut Engine, frames: usize) -> f32 {
        let mut l = vec![0.0; frames];
        let mut r = vec![0.0; frames];
        engine.process_block(&mut l, &mut r);
        l.iter().chain(r.iter()).fold(0.0f32, |a, &s| a.max(s.abs()))
    }

    /// Fold a ParamValue event through the same `LocalParams` mirror the audio
    /// thread uses, then push the working values into the engine — the shape of
    /// `process`'s param path.
    fn apply_param(local: &mut LocalParams<TOTAL_PARAMS>, engine: &mut Engine, ev: &UnknownEvent) {
        if let Some((idx, value)) = local.apply_input(ev) {
            engine.set_param(idx, value);
        }
    }

    #[test]
    fn param_value_event_updates_store_and_engine() {
        let mut engine = Engine::new(48_000.0, 512);
        let shared = SharedParams::new();
        let mut local = LocalParams::<TOTAL_PARAMS>::new(&StoreRef(&shared));
        apply_param(&mut local, &mut engine, param_event(ParamId::Cutoff, 500.0).as_ref());
        // The mirror carries the host write; `publish` folds it into the store.
        local.publish(&StoreRef(&shared));
        assert_eq!(shared.get(l1(ParamId::Cutoff)), 500.0);
        assert_eq!(engine.param(l1(ParamId::Cutoff)), 500.0);
    }

    #[test]
    fn note_on_event_makes_sound() {
        let mut engine = Engine::new(48_000.0, 512);
        engine.set_param(l1(ParamId::Env2Attack), 0.001);
        dispatch(&mut engine, note_on(0, 60, 1.0).as_ref());
        assert!(peak(&mut engine, 512) > 0.0, "a dispatched note must sound");
    }

    #[test]
    fn layer1_sounds_through_the_full_process_flow() {
        // Repro for the "Layer 1 silent" report: mirror the exact audio-thread
        // flow — activate (load_state from the store), seed the mirror, then each
        // block push every one of the `TOTAL_PARAMS` CLAP values into the engine — and prove
        // synth 0 (Layer 1, single mode) still sounds.
        let shared = SharedParams::new();
        let mut engine = Engine::new(48_000.0, 512);
        engine.load_state(shared.engine_state());
        let local = LocalParams::<TOTAL_PARAMS>::new(&StoreRef(&shared));
        for (i, &v) in local.values().iter().enumerate() {
            engine.set_param(i, v);
        }
        dispatch(&mut engine, note_on(0, 60, 1.0).as_ref());
        assert!(peak(&mut engine, 2048) > 0.0, "Layer 1 must sound in single mode");
    }

    #[test]
    fn layer1_sounds_after_a_state_save_load_roundtrip() {
        // Some hosts save+restore the plugin's own state on a fresh instance.
        // Prove that round-trip preserves Layer 1's amp route (Env2→Amp slot 0).
        let src = SharedParams::new();
        let blob = src.snapshot_bytes();
        let dst = SharedParams::new();
        dst.restore_from_bytes(&blob).expect("restore factory blob");

        let mut engine = Engine::new(48_000.0, 512);
        engine.load_state(dst.engine_state());
        let local = LocalParams::<TOTAL_PARAMS>::new(&StoreRef(&dst));
        for (i, &v) in local.values().iter().enumerate() {
            engine.set_param(i, v);
        }
        dispatch(&mut engine, note_on(0, 60, 1.0).as_ref());
        assert!(peak(&mut engine, 2048) > 0.0, "Layer 1 must sound after a state round-trip");
    }

    #[test]
    fn slot_depth_param_event_moves_modulation_through_the_shell() {
        // 0204 acceptance: automating a slot depth through the CLAP param path
        // changes the sound. Zeroing the default Env2→Amp slot silences the note.
        let mut engine = Engine::new(48_000.0, 512);
        let shared = SharedParams::new();
        let mut local = LocalParams::<TOTAL_PARAMS>::new(&StoreRef(&shared));
        engine.set_param(l1(ParamId::Env2Attack), 0.001);
        apply_param(&mut local, &mut engine, param_event(ParamId::MatrixSlot0Depth, 0.0).as_ref());
        dispatch(&mut engine, note_on(0, 60, 1.0).as_ref());
        assert_eq!(peak(&mut engine, 512), 0.0, "zeroed amp-slot depth ⇒ silence");
    }

    /// The bug fix: a UI edit written into the shared store (no CLAP event)
    /// reaches the engine. `fetch_ui_changes` pulls the store into the mirror,
    /// then the mirror's working values drive the engine — mirroring `process`.
    #[test]
    fn ui_edit_via_shared_store_reaches_engine() {
        let mut engine = Engine::new(48_000.0, 512);
        let shared = SharedParams::new();
        let mut local = LocalParams::<TOTAL_PARAMS>::new(&StoreRef(&shared));
        // UI edit: controller writes the store directly, no CLAP event.
        shared.set(ParamId::Cutoff as usize, 777.0);
        assert!(local.fetch_ui_changes(&StoreRef(&shared)));
        for (i, &v) in local.values().iter().enumerate() {
            engine.set_param(i, v);
        }
        assert_eq!(engine.param(l1(ParamId::Cutoff)), 777.0);
    }

    #[test]
    fn state_round_trips_through_the_store() {
        let shared = SharedParams::new();
        shared.set(ParamId::Cutoff as usize, 1234.0);
        let blob = shared.snapshot_bytes();

        let restored = SharedParams::new();
        restored.restore_from_bytes(&blob).unwrap();
        assert_eq!(restored.get(ParamId::Cutoff as usize), 1234.0);
        // Empty blob is rejected (0196 contract).
        assert!(restored.restore_from_bytes(&[]).is_err());
    }
}
