//! VXN3 CLAP plugin shell (clack).
//!
//! 0046 skeleton: a host-loadable CLAP plugin that reports a stereo output
//! port, reads the host transport each block, and renders silence. It reuses
//! `vxn-core-clap` for transport extraction and keeps its own `Plugin` impl
//! (per that crate's design note — generalising `clack_plugin::Plugin` over a
//! synth-specific engine isn't worth it).
//!
//! Structurally mirrors `vxn-2/crates/vxn2-clap` (same `Shared` / `MainThread`
//! / `AudioProcessor` split and `declare_extensions` shape) but stripped to the
//! empty vessel: no params, notes, GUI, state, or sequencer yet — those land in
//! later E021 slices.

use clack_extensions::audio_ports::{
    AudioPortFlags, AudioPortInfo, AudioPortInfoWriter, AudioPortType, PluginAudioPorts,
    PluginAudioPortsImpl,
};
use clack_plugin::events::event_types::{TransportEvent, TransportFlags};
use clack_plugin::prelude::*;
use vxn3_engine::{Engine, Transport};
use vxn_core_clap::tempo_from_transport;

pub struct VxnPlugin;

impl Plugin for VxnPlugin {
    type AudioProcessor<'a> = VxnAudioProcessor;
    type Shared<'a> = VxnShared;
    type MainThread<'a> = VxnMainThread;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&VxnShared>) {
        builder.register::<PluginAudioPorts>();
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
        Ok(VxnShared)
    }

    fn new_main_thread<'a>(
        _host: HostMainThreadHandle<'a>,
        _shared: &'a VxnShared,
    ) -> Result<VxnMainThread, PluginError> {
        Ok(VxnMainThread)
    }
}

/// Data shared between threads. Empty at 0046 — the lock-free param store lands
/// with the param table in a later slice.
pub struct VxnShared;
impl PluginShared<'_> for VxnShared {}

/// Main-thread state. Empty vessel for 0046 (no controller / GUI / timer yet).
pub struct VxnMainThread;
impl PluginMainThread<'_, VxnShared> for VxnMainThread {}

impl PluginAudioPortsImpl for VxnMainThread {
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

/// Map a CLAP transport event (or its absence) into the engine's clock. Pure so
/// it can be unit-tested without a host. The host supplies the transport as
/// `Option`: hosts may run `process` with no transport (offline render, freewheel).
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

impl<'a> PluginAudioProcessor<'a, VxnShared, VxnMainThread> for VxnAudioProcessor {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &mut VxnMainThread,
        _shared: &'a VxnShared,
        audio_config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        let max = audio_config.max_frames_count as usize;
        Ok(Self {
            engine: Engine::new(audio_config.sample_rate as f32, max),
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
        // Host transport → engine clock. Read every block so tempo / play-state
        // changes track without waiting for a reset. The sequencer (0048) will
        // consume it; 0046 just proves the clock reaches the engine layer.
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

        // 0046 renders silence into pre-allocated scratch (no per-block alloc);
        // later slices drive the sequencer + voice engines here.
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
