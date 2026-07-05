//! VXN3 CLAP plugin shell (clack).
//!
//! Host-loadable plugin: stereo output port, host-transport sync, the audible
//! engine (0047/0048/0049), and the HTML faceplate (0052) — a `vxn3-ui-web`
//! webview driven through `vxn-core-app`'s `Controller`. UI edits flow to the
//! audio engine over the shared [`EngineIo`] command queue / swap mailboxes;
//! the playhead flows back to the page on the GUI timer.
//!
//! Structurally mirrors `vxn-2/crates/vxn2-clap`, but vxn-3 carries no flat
//! CLAP params (its UI state is the structured sequencer), so there is no
//! params/state extension or dirty-bitset pump — edits are custom events and
//! the only Model→View push is the playhead.

use clack_extensions::audio_ports::{
    AudioPortFlags, AudioPortInfo, AudioPortInfoWriter, AudioPortType, PluginAudioPorts,
    PluginAudioPortsImpl,
};
use clack_extensions::gui::PluginGui;
use clack_extensions::latency::{PluginLatency, PluginLatencyImpl};
use clack_extensions::note_ports::{
    NoteDialect, NoteDialects, NotePortInfo, NotePortInfoWriter, PluginNotePorts, PluginNotePortsImpl,
};
use clack_extensions::params::{
    ParamDisplayWriter, ParamInfo, ParamInfoFlags, ParamInfoWriter, PluginAudioProcessorParams,
    PluginMainThreadParams, PluginParams,
};
use clack_extensions::state::{PluginState, PluginStateImpl};
use clack_extensions::timer::{HostTimer, PluginTimer, PluginTimerImpl, TimerId};
use clack_plugin::events::event_types::{ParamValueEvent, TransportEvent, TransportFlags};
use clack_plugin::events::{Match, Pckn};
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::prelude::*;
use clack_plugin::stream::{InputStream, OutputStream};
use clack_plugin::utils::Cookie;
use std::ffi::CStr;
use std::fmt::Write as _;
use std::io::{Read as _, Write as _IoWrite};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use vxn_core_app::{Controller, CorpusHandle, ViewEvent};
use vxn3_app::{NullStore, Vxn3Model, Vxn3ViewCustom, tick_vxn3};
use vxn3_engine::io::{EngineIo, PlayheadState};
use vxn3_engine::{Engine, LockParam, N_TRACKS, Transport, make};
use vxn_core_clap::tempo_from_transport;

pub mod gui;
pub mod params;
pub mod state;

use params::{ParamCache, TOTAL_PARAMS};

/// Lock a mutex by extracting the inner value rather than unwrapping — a panic
/// under `panic = unwind` could poison it, but subsequent main-thread ticks
/// should still make progress.
pub(crate) fn lock_mut<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
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
            .register::<PluginGui>()
            .register::<PluginTimer>()
            .register::<PluginLatency>()
            .register::<PluginParams>()
            .register::<PluginState>();
    }
}

impl DefaultPluginFactory for VxnPlugin {
    fn get_descriptor() -> PluginDescriptor {
        use clack_plugin::plugin::features::*;
        PluginDescriptor::new("labs.vulpus.vxn3", "VXN3").with_features([
            INSTRUMENT,
            DRUM,
            STEREO,
        ])
    }

    fn new_shared(_host: HostSharedHandle) -> Result<VxnShared, PluginError> {
        Ok(VxnShared {
            io: EngineIo::new(),
            sample_rate: AtomicU32::new(48_000.0_f32.to_bits()),
            params: ParamCache::new(),
        })
    }

    fn new_main_thread<'a>(
        host: HostMainThreadHandle<'a>,
        shared: &'a VxnShared,
    ) -> Result<VxnMainThread<'a>, PluginError> {
        let (controller, view_rx, corpus) =
            Controller::new(Arc::new(Vxn3Model), Box::new(NullStore));
        Ok(VxnMainThread {
            shared,
            io: shared.io.clone(),
            controller: Arc::new(Mutex::new(controller)),
            view_rx: Arc::new(Mutex::new(view_rx)),
            corpus,
            gui: None,
            host: Some(host),
            timer: None,
        })
    }
}

/// Cross-thread shared state: the main↔audio I/O bundle (edit queue, playhead,
/// swap mailboxes) and the activated sample rate (published by the audio thread,
/// read by the main thread when it builds a freshly selected engine).
pub struct VxnShared {
    io: EngineIo,
    sample_rate: AtomicU32, // f32 bits
    /// Fixed host-param value cache (0171): written as host automation lands on
    /// the audio thread, read by the main thread for `get_value`.
    params: ParamCache,
}

impl VxnShared {
    fn sample_rate(&self) -> f32 {
        f32::from_bits(self.sample_rate.load(Ordering::Relaxed))
    }
}

impl PluginShared<'_> for VxnShared {}

pub struct VxnMainThread<'a> {
    shared: &'a VxnShared,
    /// Clone of the shared I/O (same inner `Arc`s) so `tick_vxn3` can post edits.
    io: EngineIo,
    pub(crate) controller: Arc<Mutex<Controller<Vxn3Model>>>,
    pub(crate) view_rx: Arc<Mutex<Receiver<ViewEvent>>>,
    pub(crate) corpus: CorpusHandle,
    pub(crate) gui: Option<vxn3_ui_web::EditorHandle>,
    pub(crate) host: Option<HostMainThreadHandle<'a>>,
    pub(crate) timer: Option<(HostTimer, TimerId)>,
}

impl<'a> PluginMainThread<'a, VxnShared> for VxnMainThread<'a> {}

impl VxnMainThread<'_> {
    /// Drain the controller's view-event queue into the live WebView.
    fn drain_view_events(&mut self) {
        let Some(handle) = self.gui.as_ref() else {
            return;
        };
        let rx = lock_mut(&self.view_rx);
        while let Ok(ev) = rx.try_recv() {
            handle.push_view_event(ev);
        }
    }

    /// Push the current per-lane playhead to the page (the only Model→View push
    /// vxn-3 has — no flat params).
    fn push_playhead(&mut self) {
        let Some(handle) = self.gui.as_ref() else {
            return;
        };
        let mut steps = [PlayheadState::STOPPED; N_TRACKS];
        for (t, s) in steps.iter_mut().enumerate() {
            *s = self.io.playhead.step(t);
        }
        let playing = self.io.playhead.playing();
        handle.push_view_event(ViewEvent::Custom(Box::new(Vxn3ViewCustom::Playhead {
            steps,
            playing,
        })));
    }
}

impl PluginTimerImpl for VxnMainThread<'_> {
    fn on_timer(&mut self, _id: TimerId) {
        // Translate queued UI edits into engine commands / swaps.
        {
            let mut ctrl = lock_mut(&self.controller);
            tick_vxn3(&mut ctrl, &self.io, self.shared.sample_rate());
        }
        self.drain_view_events();
        self.push_playhead();
        if let Some(handle) = self.gui.as_ref() {
            handle.flush_view_events();
        }
    }
}

impl PluginLatencyImpl for VxnMainThread<'_> {
    /// The master limiter look-ahead — constant, always-on (PDC).
    fn get(&mut self) -> u32 {
        vxn3_engine::LIMITER_LOOKAHEAD
    }
}

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

// ── MIDI free-play note input (0186) ─────────────────────────────────────────

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

/// Default GM-drum-ish MIDI note → track map for free-play (0186). Standard drum
/// notes route to their voice track (the engine still receives the actual note, so a
/// Metal track's closed/open-hat note split keeps working); other notes fall through
/// chromatically so a keyboard plays the kit pitched across tracks. Explicit table —
/// user remapping is a later ticket.
fn note_to_track(note: u8) -> usize {
    match note {
        35 | 36 => 0,                     // acoustic / electric kick
        37..=40 => 2,                     // rim, snare, clap → snare/noise track
        42 | 44 | 46 => 1,                // closed / pedal / open hat — Metal note-split
        41 | 43 | 45 | 47 | 48 | 50 => 3, // toms
        49 | 55 | 57 => 4,                // crash / splash
        51 | 53 | 59 => 5,                // ride / bell
        _ => note as usize % N_TRACKS,    // chromatic fallback across the kit
    }
}

/// Map a CLAP transport event (or its absence) into the engine's clock.
fn read_transport(t: Option<&TransportEvent>) -> Transport {
    match t {
        Some(t) => Transport {
            playing: t.flags.contains(TransportFlags::IS_PLAYING),
            tempo_bpm: tempo_from_transport(t).unwrap_or(120.0),
            song_pos_beats: t
                .flags
                .contains(TransportFlags::HAS_BEATS_TIMELINE)
                .then(|| t.song_pos_beats.to_float()),
        },
        None => Transport::default(),
    }
}

/// Apply one host param write to the engine + the shared value cache. Shared by
/// the `process()` event loop and the audio-thread param `flush` (0171).
fn apply_host_param(engine: &mut Engine, shared: &VxnShared, id: usize, value: f32) {
    if id < TOTAL_PARAMS {
        shared.params.set(id, value);
        if let Some(cmd) = params::to_command(id, value) {
            engine.apply_command(cmd);
        }
    }
}

/// The engine's current *effective* value for a host param slot — the resolved
/// value the mix used this block (base, or a p-lock override for lockable lanes).
/// Read by the 0173 echo pump.
fn effective_value(engine: &Engine, slot: params::Slot) -> f32 {
    use params::Slot;
    match slot {
        Slot::MasterVolume => engine.master_volume(),
        Slot::DelayFeedback => engine.delay_feedback(),
        Slot::DelayTime => engine.delay_time_beats(),
        Slot::DelayReturn => engine.delay_return(),
        Slot::Level(t) => engine.track_effective(t as usize, LockParam::Gain),
        Slot::Pan(t) => engine.track_effective(t as usize, LockParam::Pan),
        Slot::Send(t) => engine.track_effective(t as usize, LockParam::Send),
        Slot::Mute(t) => {
            if engine.track_muted(t as usize) {
                1.0
            } else {
                0.0
            }
        }
        Slot::Macro(t, m) => {
            let p = match m {
                0 => LockParam::Decay,
                1 => LockParam::Tone,
                _ => LockParam::Pitch,
            };
            engine.track_effective(t as usize, p)
        }
    }
}

/// Diff every host-facing param's current effective value against the cache;
/// for each that changed, update the cache and call `emit(id, value)`. The core
/// of the 0173 echo pump — allocation-free, so `process` can push a
/// `ParamValueEvent` per change and tests can record them. Host writes pre-set
/// the cache, so an unchanged value never re-echoes (no feedback loop).
fn drain_param_echo(engine: &Engine, cache: &ParamCache, mut emit: impl FnMut(usize, f32)) {
    for id in 0..TOTAL_PARAMS {
        let Some(slot) = params::decode(id) else {
            continue;
        };
        let eff = effective_value(engine, slot);
        if eff != cache.get(id) {
            cache.set(id, eff);
            emit(id, eff);
        }
    }
}

pub struct VxnAudioProcessor<'a> {
    engine: Engine,
    shared: &'a VxnShared,
    scratch_l: Vec<f32>,
    scratch_r: Vec<f32>,
}

impl<'a> PluginAudioProcessor<'a, VxnShared, VxnMainThread<'a>> for VxnAudioProcessor<'a> {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &mut VxnMainThread<'a>,
        shared: &'a VxnShared,
        audio_config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        let max = audio_config.max_frames_count as usize;
        let sr = audio_config.sample_rate as f32;
        // Publish the rate so the main thread builds swapped-in engines at it.
        shared.sample_rate.store(sr.to_bits(), Ordering::Relaxed);
        let mut engine = Engine::with_io(sr, max, shared.io.clone());
        // Seed the fresh engine from the host-param cache so automation set while
        // inactive — or a just-restored project state (0174) — is in effect from
        // the first block (0171).
        for id in 0..TOTAL_PARAMS {
            if let Some(cmd) = params::to_command(id, shared.params.get(id)) {
                engine.apply_command(cmd);
            }
        }
        Ok(Self {
            engine,
            shared,
            scratch_l: vec![0.0; max],
            scratch_r: vec![0.0; max],
        })
    }

    fn process(
        &mut self,
        process: Process,
        mut audio: Audio,
        events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        self.engine.set_transport(read_transport(process.transport));

        // Fold host parameter automation into the engine (block-granular) + the
        // shared cache, before the engine drains the block. Applied straight to
        // the engine — the UI edit queue is strict SPSC, so host writes must not
        // become a second producer (0171).
        for batch in events.input.batch() {
            for event in batch.events() {
                match event.as_core_event() {
                    // Host param automation (block-granular, 0171).
                    Some(CoreEventSpace::ParamValue(e)) => {
                        if let Some(pid) = e.param_id() {
                            apply_host_param(&mut self.engine, self.shared, pid.get() as usize, e.value() as f32);
                        }
                    }
                    // MIDI free-play note-on (0186): typed CLAP note dialect. Map the
                    // note to a track + queue it sample-accurately at its event time.
                    Some(CoreEventSpace::NoteOn(e)) => {
                        if let Match::Specific(key) = e.key() {
                            let key = key as u8;
                            self.engine.queue_free_note(
                                note_to_track(key),
                                key as f32,
                                e.velocity() as f32,
                                event.header().time(),
                            );
                        }
                    }
                    // Raw-MIDI note-on (a MIDI-dialect host / the standalone): 0x90 with
                    // velocity > 0. Note-off / 0x80 is ignored — percussion is one-shot.
                    Some(CoreEventSpace::Midi(e)) => {
                        let [status, d1, d2] = e.data();
                        if status & 0xF0 == 0x90 && d2 > 0 {
                            self.engine.queue_free_note(
                                note_to_track(d1),
                                d1 as f32,
                                d2 as f32 / 127.0,
                                event.header().time(),
                            );
                        }
                    }
                    _ => {}
                }
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

        // Drains UI edits, installs swaps, sequences + renders + publishes the
        // playhead — all allocation-free.
        self.engine
            .process_block(&mut self.scratch_l[..frames], &mut self.scratch_r[..frames]);

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

        // Echo internally-originated param changes (faceplate edits + p-lock
        // resolved values) back to the host so its automation lanes / generic UI
        // track them (0173). Host writes above pre-set the cache, so an unchanged
        // effective value never re-echoes — no feedback loop. Allocation-free.
        let out_events = events.output;
        drain_param_echo(&self.engine, &self.shared.params, |id, eff| {
            let _ = out_events.try_push(ParamValueEvent::new(
                0,
                ClapId::new(id as u32),
                Pckn::match_all(),
                eff as f64,
                Cookie::empty(),
            ));
        });

        Ok(ProcessStatus::Continue)
    }

    fn reset(&mut self) {
        self.engine.reset();
    }
}

// ── Parameters (fixed host table, 0171) ──────────────────────────────────────

impl PluginMainThreadParams for VxnMainThread<'_> {
    fn count(&mut self) -> u32 {
        TOTAL_PARAMS as u32
    }

    fn get_info(&mut self, param_index: u32, info: &mut ParamInfoWriter) {
        let id = param_index as usize;
        let Some(slot) = params::decode(id) else {
            return;
        };
        let (min, max, default, stepped) = params::range(slot);
        let mut flags = ParamInfoFlags::IS_AUTOMATABLE;
        if stepped {
            flags |= ParamInfoFlags::IS_STEPPED;
        }
        let mut name = String::new();
        params::write_name(slot, &mut name);
        let mut module = String::new();
        params::write_module(slot, &mut module);
        info.set(&ParamInfo {
            id: ClapId::new(id as u32),
            flags,
            cookie: Default::default(),
            name: name.as_bytes(),
            module: module.as_bytes(),
            min_value: min as f64,
            max_value: max as f64,
            default_value: default as f64,
        });
    }

    fn get_value(&mut self, param_id: ClapId) -> Option<f64> {
        let id = param_id.get() as usize;
        (id < TOTAL_PARAMS).then(|| self.shared.params.get(id) as f64)
    }

    fn value_to_text(
        &mut self,
        param_id: ClapId,
        value: f64,
        writer: &mut ParamDisplayWriter,
    ) -> std::fmt::Result {
        let Some(slot) = params::decode(param_id.get() as usize) else {
            return Err(std::fmt::Error);
        };
        // A macro slot renders **flavour-aware** (0185): the assigned voice's macro name
        // (override / first-bound-param / "M<n>") + the knob position as a percent. Reads
        // the main-thread flavour store; falls back to the fixed engine map (0172) only if
        // the store is unavailable. Mix / master params render generically.
        if let params::Slot::Macro(t, m) = slot {
            let kind = self.io.kinds.get(t as usize);
            let meta = vxn3_engine::params_for(kind);
            let res = self.io.flavours.with(t as usize, |flav| {
                vxn3_engine::flavour::flavour_macro_text(meta, flav, m as usize, value as f32, writer)
            });
            return match res {
                Some(r) => r,
                None => vxn3_engine::macro_display(kind, m as usize, value as f32, writer),
            };
        }
        let mut s = String::new();
        params::write_value_text(slot, value as f32, &mut s);
        writer.write_str(&s)
    }

    fn text_to_value(&mut self, param_id: ClapId, text: &CStr) -> Option<f64> {
        // Slot-aware inverse of `value_to_text` so the transforms round-trip.
        let slot = params::decode(param_id.get() as usize)?;
        let s = text.to_str().ok()?;
        // Macros are "<name> <pct>%" (0185) — invert the percent, flavour-independent.
        if let params::Slot::Macro(_, _) = slot {
            return vxn3_engine::flavour::flavour_macro_parse(s).map(|v| v as f64);
        }
        params::parse_value(slot, s).map(|v| v as f64)
    }

    fn flush(&mut self, input: &InputEvents, _output: &mut OutputEvents) {
        // Main-thread / inactive flush: no engine here, so fold host writes into
        // the shared cache. `activate` replays the cache into the fresh engine.
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
        // Active flush (no render): apply straight to the engine + cache.
        for event in input {
            if let Some(CoreEventSpace::ParamValue(e)) = event.as_core_event() {
                if let Some(pid) = e.param_id() {
                    apply_host_param(&mut self.engine, self.shared, pid.get() as usize, e.value() as f32);
                }
            }
        }
    }
}

// ── State save / restore (0174) ──────────────────────────────────────────────

impl PluginStateImpl for VxnMainThread<'_> {
    fn save(&mut self, output: &mut OutputStream) -> Result<(), PluginError> {
        let blob = state::save(&self.shared.params, &self.io.kinds, &self.io.flavours);
        output
            .write_all(&blob)
            .map_err(|_| PluginError::Message("state save failed"))
    }

    fn load(&mut self, input: &mut InputStream) -> Result<(), PluginError> {
        let mut blob = Vec::new();
        input
            .read_to_end(&mut blob)
            .map_err(|_| PluginError::Message("state read failed"))?;
        state::load(&blob, &self.shared.params, &self.io.kinds, &self.io.flavours)
            .map_err(|_| PluginError::Message("state parse failed"))?;
        // Rebuild each track's engine from the restored kind and apply its restored
        // flavour (0185) *before* handing it to the audio thread — the deep patch is the
        // base layer; the macro/mix cache replays over it (`invalidate_applied` on
        // install; cache re-applies on `activate`). The patched engine is queued into the
        // *shared* swap ring, so it survives an inactive load → `activate` too.
        let sr = self.shared.sample_rate();
        for t in 0..N_TRACKS {
            if let Some(swap) = self.io.swaps.get(t) {
                let mut engine = make(self.io.kinds.get(t), sr);
                engine.apply_flavour(self.io.flavours.get(t));
                let _ = swap.send(engine);
            }
        }
        Ok(())
    }
}

clack_export_entry!(SinglePluginEntry<VxnPlugin>);

#[cfg(test)]
mod tests {
    use super::*;

    // ── Allocation trap ──────────────────────────────────────────────────────
    // The workspace has no shared alloc-trap harness (vxn-1/vxn-2 rely on
    // pre-allocation + by-construction review). 0046 introduces one: a
    // thread-local counting allocator so RT-discipline tests can assert the
    // process path is allocation-free. `cfg(test)`-gated, so the shipped cdylib
    // keeps the system allocator.
    mod alloc_trap {
        use std::alloc::{GlobalAlloc, Layout, System};
        use std::cell::Cell;

        thread_local! {
            static ARMED: Cell<bool> = const { Cell::new(false) };
            static COUNT: Cell<usize> = const { Cell::new(0) };
        }

        struct TrapAlloc;

        // SAFETY: forwards every call to the system allocator; the only added
        // work is a non-allocating thread-local counter bump while armed.
        unsafe impl GlobalAlloc for TrapAlloc {
            unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
                if ARMED.with(Cell::get) {
                    COUNT.with(|c| c.set(c.get() + 1));
                }
                unsafe { System.alloc(layout) }
            }
            unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
                unsafe { System.dealloc(ptr, layout) }
            }
            unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
                if ARMED.with(Cell::get) {
                    COUNT.with(|c| c.set(c.get() + 1));
                }
                unsafe { System.realloc(ptr, layout, new_size) }
            }
        }

        #[global_allocator]
        static GLOBAL: TrapAlloc = TrapAlloc;

        /// Count allocations made on *this thread* during `f`. Thread-local so
        /// it stays accurate under cargo's parallel test runner.
        pub fn count_allocs(f: impl FnOnce()) -> usize {
            COUNT.with(|c| c.set(0));
            ARMED.with(|a| a.set(true));
            f();
            ARMED.with(|a| a.set(false));
            COUNT.with(Cell::get)
        }
    }

    use vxn3_engine::track_engine::EngineKind;
    use vxn3_engine::{Engine, EngineCommand, EngineIo};

    #[test]
    fn silent_render_is_alloc_free_and_silent() {
        let mut engine = Engine::new(48_000.0, 512);
        // Pre-allocated, dirtied buffers: the render must zero them, in place.
        let mut l = vec![1.0_f32; 512];
        let mut r = vec![-1.0_f32; 512];

        let allocs = alloc_trap::count_allocs(|| {
            // One second at 48k / 512-frame blocks.
            for _ in 0..(48_000 / 512) {
                engine.process_block(&mut l, &mut r);
            }
        });

        assert_eq!(allocs, 0, "process_block allocated on the audio path");
        assert!(
            l.iter().chain(r.iter()).all(|&v| v == 0.0),
            "expected silence"
        );
    }

    #[test]
    fn absent_transport_yields_default_clock() {
        let t = read_transport(None);
        assert_eq!(t, Transport::default());
        assert!(!t.playing);
        assert_eq!(t.tempo_bpm, 120.0);
        assert_eq!(t.song_pos_beats, None);
    }

    #[test]
    fn transport_reaches_engine() {
        let mut engine = Engine::new(48_000.0, 512);
        let clock = Transport {
            playing: true,
            tempo_bpm: 174.0,
            song_pos_beats: Some(16.0),
        };
        engine.set_transport(clock);
        assert_eq!(engine.transport(), clock);
    }

    fn rms(b: &[f32]) -> f32 {
        (b.iter().map(|&x| x * x).sum::<f32>() / b.len().max(1) as f32).sqrt()
    }

    /// A track programmed to fire step 0 at beat 0, transport playing — one block
    /// renders an audible hit.
    fn primed_engine() -> Engine {
        let mut e = Engine::new(48_000.0, 512);
        e.apply_command(EngineCommand::SetStep { track: 0, step: 0, note: 36.0, velocity: 1.0 });
        e.set_transport(Transport { playing: true, tempo_bpm: 120.0, song_pos_beats: Some(0.0) });
        e
    }

    #[test]
    fn host_mute_reaches_engine() {
        // A host write to the track-0 mute param (via the fixed-table mapping)
        // silences the track; without it the same block is audible.
        let mute_id = params::N_MASTER + 2;
        let (mut l, mut r) = (vec![0.0_f32; 512], vec![0.0_f32; 512]);

        let mut loud = primed_engine();
        loud.process_block(&mut l, &mut r);
        assert!(rms(&l) > 1e-3, "unmuted track should sound, rms={}", rms(&l));

        let mut muted = primed_engine();
        muted.apply_command(params::to_command(mute_id, 1.0).unwrap());
        muted.process_block(&mut l, &mut r);
        assert!(rms(&l) < 1e-6, "host mute should silence, rms={}", rms(&l));
    }

    #[test]
    fn host_master_volume_reaches_engine() {
        // Master volume 0 (id 0) silences the whole mix.
        let (mut l, mut r) = (vec![0.0_f32; 512], vec![0.0_f32; 512]);
        let mut e = primed_engine();
        e.apply_command(params::to_command(0, 0.0).unwrap());
        e.process_block(&mut l, &mut r);
        assert!(rms(&l) < 1e-6, "master volume 0 should silence, rms={}", rms(&l));
    }

    fn collect_echo(engine: &Engine, cache: &ParamCache) -> Vec<(usize, f32)> {
        let mut out = Vec::new();
        drain_param_echo(engine, cache, |id, v| out.push((id, v)));
        out
    }

    #[test]
    fn echo_is_silent_when_nothing_changed() {
        // Fresh engine + default-seeded cache agree → no spurious echo (0173).
        let engine = Engine::new(48_000.0, 512);
        let cache = ParamCache::new();
        assert!(collect_echo(&engine, &cache).is_empty());
    }

    #[test]
    fn echo_reports_internal_edit_once() {
        // A faceplate edit (applied straight to the engine here) is echoed to the
        // host exactly once; a second drain is silent — the cache absorbs it, so
        // there is no feedback loop.
        let mut engine = Engine::new(48_000.0, 512);
        let cache = ParamCache::new();
        engine.apply_command(EngineCommand::SetGain { track: 1, gain: 0.3 });

        let level_id = params::N_MASTER + params::PER_TRACK; // track 1, level
        let first = collect_echo(&engine, &cache);
        assert_eq!(first, vec![(level_id, 0.3)], "one echo for the edited param");
        assert_eq!(cache.get(level_id), 0.3, "cache absorbed the value");
        assert!(collect_echo(&engine, &cache).is_empty(), "no re-echo (no feedback)");
    }

    #[test]
    fn echo_drain_is_alloc_free() {
        // The echo pass runs every block on the audio thread — it must not
        // allocate (0173). Emit into a non-allocating counter.
        let mut engine = Engine::new(48_000.0, 512);
        let cache = ParamCache::new();
        engine.apply_command(EngineCommand::SetGain { track: 2, gain: 0.7 });
        let mut n = 0usize;
        let allocs = alloc_trap::count_allocs(|| {
            for _ in 0..100 {
                drain_param_echo(&engine, &cache, |_, _| n += 1);
            }
        });
        assert_eq!(allocs, 0, "param echo allocated on the audio path");
        assert_eq!(n, 1, "changed once, echoed once");
    }

    #[test]
    fn restored_kind_mirror_rebuilds_engines() {
        // The state path restores each track's kind into the shared mirror; a
        // (re)activated engine builds from it — so a reloaded project comes up
        // on the saved engines, not all-KickTone (0174).
        let io = EngineIo::new();
        io.kinds.set(3, EngineKind::Noise);
        io.kinds.set(5, EngineKind::Metal);
        let engine = Engine::with_io(48_000.0, 512, io);
        assert_eq!(engine.track_kind(3), Some(EngineKind::Noise));
        assert_eq!(engine.track_kind(5), Some(EngineKind::Metal));
        assert_eq!(engine.track_kind(0), Some(EngineKind::KickTone)); // default
    }

    #[test]
    fn state_round_trips_through_cache_and_kinds() {
        // Full blob round-trip at the state-module boundary: a saved cache + kind
        // set reloads identically (mirrors clap-validator state-reproducibility).
        let cache = ParamCache::new();
        let kinds = vxn3_engine::TrackKinds::new();
        let flavours = vxn3_engine::io::FlavourStore::new();
        cache.set(0, 0.2);
        kinds.set(4, EngineKind::Metal);
        flavours.set(4, vxn3_engine::default_flavour_for(EngineKind::Metal));
        let blob = state::save(&cache, &kinds, &flavours);

        let cache2 = ParamCache::new();
        let kinds2 = vxn3_engine::TrackKinds::new();
        let flavours2 = vxn3_engine::io::FlavourStore::new();
        state::load(&blob, &cache2, &kinds2, &flavours2).unwrap();
        assert_eq!(cache2.get(0), 0.2);
        assert_eq!(kinds2.get(4), EngineKind::Metal);
        assert_eq!(blob, state::save(&cache2, &kinds2, &flavours2), "resave identical");
    }

    #[test]
    fn echo_skips_host_write_that_preset_cache() {
        // The host-write path (0171) sets the cache before the engine renders, so
        // the matching effective value is not echoed back — the no-feedback rule.
        let mut engine = Engine::new(48_000.0, 512);
        let cache = ParamCache::new();
        let level_id = params::N_MASTER; // track 0 level
        // Emulate apply_host_param: cache first, then engine.
        cache.set(level_id, 0.6);
        engine.apply_command(params::to_command(level_id, 0.6).unwrap());
        assert!(
            collect_echo(&engine, &cache).is_empty(),
            "host write should not echo back to the host"
        );
    }
}
