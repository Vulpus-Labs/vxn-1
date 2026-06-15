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
use clack_extensions::timer::{HostTimer, PluginTimer, PluginTimerImpl, TimerId};
use clack_plugin::events::event_types::{TransportEvent, TransportFlags};
use clack_plugin::prelude::*;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use vxn_core_app::{Controller, CorpusHandle, ViewEvent};
use vxn3_app::{NullStore, Vxn3Model, Vxn3ViewCustom, tick_vxn3};
use vxn3_engine::io::{EngineIo, PlayheadState};
use vxn3_engine::{Engine, N_TRACKS, Transport};
use vxn_core_clap::tempo_from_transport;

pub mod gui;

/// Lock a mutex by extracting the inner value rather than unwrapping — a panic
/// under `panic = unwind` could poison it, but subsequent main-thread ticks
/// should still make progress.
pub(crate) fn lock_mut<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct VxnPlugin;

impl Plugin for VxnPlugin {
    type AudioProcessor<'a> = VxnAudioProcessor;
    type Shared<'a> = VxnShared;
    type MainThread<'a> = VxnMainThread<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&VxnShared>) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginGui>()
            .register::<PluginTimer>()
            .register::<PluginLatency>();
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

pub struct VxnAudioProcessor {
    engine: Engine,
    scratch_l: Vec<f32>,
    scratch_r: Vec<f32>,
}

impl<'a> PluginAudioProcessor<'a, VxnShared, VxnMainThread<'a>> for VxnAudioProcessor {
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
        Ok(Self {
            engine: Engine::with_io(sr, max, shared.io.clone()),
            scratch_l: vec![0.0; max],
            scratch_r: vec![0.0; max],
        })
    }

    fn process(
        &mut self,
        process: Process,
        mut audio: Audio,
        _events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        self.engine.set_transport(read_transport(process.transport));

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

        Ok(ProcessStatus::Continue)
    }

    fn reset(&mut self) {
        self.engine.reset();
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

    use vxn3_engine::Engine;

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
}
