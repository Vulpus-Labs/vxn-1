//! VXN4 CLAP plugin shell (clack).
//!
//! Host-loadable plugin: stereo out, note in, eleven automatable parameters,
//! state save/restore. **No faceplate** — there is no `gui` or `timer`
//! extension here, and no webview. The synth is still an ear-driven sketch, and
//! a host's generic parameter UI is enough to turn eight knobs.
//!
//! Structurally the smallest of the four shells. vxn-2 and vxn-3 carry a
//! controller, a view-event pump and a dirty bitset because they have pages to
//! drive; vxn-4 has a param cache and an engine.
//!
//! ## Where parameters are applied
//!
//! Straight onto the engine on the audio thread, at the event's own position in
//! the block — [`vxn_core_clap::batch_range`] splits the block at every event
//! batch, so a macro sweep lands where the host put it rather than at the block
//! boundary. The engine's own control rate (32 samples) is the real resolution;
//! this makes sure the shell is not the coarser of the two.
//!
//! There is no echo pump. Nothing inside vxn-4 originates a parameter change —
//! no faceplate, no p-locks, no modulation writing back — so the host's value
//! is always the value, and an echo would be a loop with nothing to say.

use clack_extensions::audio_ports::{
    AudioPortFlags, AudioPortInfo, AudioPortInfoWriter, AudioPortType, PluginAudioPorts,
    PluginAudioPortsImpl,
};
use clack_extensions::latency::{PluginLatency, PluginLatencyImpl};
use clack_extensions::note_ports::{
    NoteDialect, NoteDialects, NotePortInfo, NotePortInfoWriter, PluginNotePorts,
    PluginNotePortsImpl,
};
use clack_extensions::params::{
    ParamDisplayWriter, ParamInfo, ParamInfoFlags, ParamInfoWriter, PluginAudioProcessorParams,
    PluginMainThreadParams, PluginParams,
};
use clack_extensions::state::{PluginState, PluginStateImpl};
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::prelude::*;
use clack_plugin::stream::{InputStream, OutputStream};
use std::ffi::CStr;
use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::atomic::{AtomicU32, Ordering};

use vxn_core_clap::{EngineNotes, batch_range, dispatch_notes};
use vxn4_engine::{Engine, HOST_LATENCY_SAMPLES};

pub mod params;
pub mod state;

use params::{ParamCache, Slot, TOTAL_PARAMS};

pub struct VxnPlugin;

impl Plugin for VxnPlugin {
    type AudioProcessor<'a> = VxnAudioProcessor<'a>;
    type Shared<'a> = VxnShared;
    type MainThread<'a> = VxnMainThread<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&VxnShared>) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginNotePorts>()
            .register::<PluginLatency>()
            .register::<PluginParams>()
            .register::<PluginState>();
    }
}

impl DefaultPluginFactory for VxnPlugin {
    fn get_descriptor() -> PluginDescriptor {
        use clack_plugin::plugin::features::*;
        PluginDescriptor::new("labs.vulpus.vxn4", "VXN4").with_features([
            INSTRUMENT,
            SYNTHESIZER,
            STEREO,
        ])
    }

    fn new_shared(_host: HostSharedHandle) -> Result<VxnShared, PluginError> {
        Ok(VxnShared {
            params: ParamCache::new(),
            sample_rate: AtomicU32::new(48_000.0_f32.to_bits()),
        })
    }

    fn new_main_thread<'a>(
        _host: HostMainThreadHandle<'a>,
        shared: &'a VxnShared,
    ) -> Result<VxnMainThread<'a>, PluginError> {
        Ok(VxnMainThread { shared })
    }
}

/// Cross-thread state: the host-facing parameter values, and the activated
/// sample rate.
///
/// The cache is the seam between threads. The audio thread writes it as
/// automation lands; the main thread reads it for `get_value` and state save,
/// and writes it on an inactive flush — which `activate` then replays into a
/// fresh engine, so automation set while the plugin was inactive is in effect
/// from the first block rather than the first event.
pub struct VxnShared {
    params: ParamCache,
    sample_rate: AtomicU32, // f32 bits
}

impl VxnShared {
    pub fn sample_rate(&self) -> f32 {
        f32::from_bits(self.sample_rate.load(Ordering::Relaxed))
    }
}

impl PluginShared<'_> for VxnShared {}

pub struct VxnMainThread<'a> {
    shared: &'a VxnShared,
}

impl<'a> PluginMainThread<'a, VxnShared> for VxnMainThread<'a> {}

impl PluginLatencyImpl for VxnMainThread<'_> {
    /// Constant across the session, deliberately. See
    /// [`vxn4_engine::HOST_LATENCY_SAMPLES`] — quality is a live parameter and
    /// real latency differs by one sample between the two settings, so this
    /// reports the worst case rather than renegotiating the host's graph
    /// whenever a knob moves.
    fn get(&mut self) -> u32 {
        HOST_LATENCY_SAMPLES
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

/// Adapts vxn-4's [`Engine`] to [`EngineNotes`] so the shared [`dispatch_notes`]
/// drives the note and raw-MIDI arms.
///
/// Here rather than on `Engine` because the orphan rule blocks a foreign-trait
/// / foreign-type impl in this crate — the same reason vxn-2 has one. The only
/// mapping is velocity: CLAP's `[0, 1]` float to the engine's `1..=127`, with
/// the floor at 1 because a zero velocity is a note-*off* in the engine's
/// `note_on`, and a host that rounds a quiet note to zero would silence it
/// instead of playing it softly.
///
/// The expression arms — bend, wheel, aftertouch, sustain — keep the trait's
/// no-op defaults. vxn-4 has no pitch-bend range, no pedal and no per-note
/// expression yet, and a silently-dropped event is better than a wrong one.
struct EngineNotesAdapter<'a>(&'a mut Engine);

impl EngineNotes for EngineNotesAdapter<'_> {
    fn note_on(&mut self, key: u8, velocity: f32) {
        let vel = ((velocity * 127.0) as i32).clamp(1, 127) as u8;
        self.0.note_on(key, vel);
    }

    fn note_off(&mut self, key: u8) {
        self.0.note_off(key);
    }
}

/// Apply one host parameter write to the engine and the shared cache.
///
/// The cache is written **first and always**, even for a value the engine
/// clamps differently, so `get_value` and a state save report what the host
/// asked for rather than what the engine did with it. Both clamp; only the
/// cache's clamp is visible to the host.
fn apply_host_param(engine: &mut Engine, shared: &VxnShared, id: usize, value: f32) {
    let Some(slot) = params::decode(id) else {
        return;
    };
    let v = params::clamp(slot, value);
    shared.params.set(id, v);
    apply_to_engine(engine, slot, v);
}

/// Push one resolved parameter value into the engine.
fn apply_to_engine(engine: &mut Engine, slot: Slot, value: f32) {
    match slot {
        Slot::Patch => {
            let want = params::patch_from(value);
            // Guarded, because `set_patch` panics every sounding voice. An
            // unguarded write would make a host that re-sends its whole
            // parameter set each block — several do — a permanent all-notes-off.
            if want != engine.patch_index() {
                engine.set_patch(want);
            }
        }
        Slot::Quality => engine.set_quality(params::quality_from(value)),
        Slot::MasterGain => engine.set_master_gain(value),
        Slot::Macro(m) => engine.set_macro(m as usize, value),
    }
}

/// Replay the whole cache into `engine`. Used on `activate`, so a fresh engine
/// starts from the host's state rather than the patch defaults.
fn seed_from_cache(engine: &mut Engine, shared: &VxnShared) {
    for id in 0..TOTAL_PARAMS {
        if let Some(slot) = params::decode(id) {
            apply_to_engine(engine, slot, shared.params.get(id));
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
        shared.sample_rate.store(sr.to_bits(), Ordering::Relaxed);
        let mut engine = Engine::new(sr);
        seed_from_cache(&mut engine, shared);
        Ok(Self {
            engine,
            shared,
            // The only allocation in the plugin's life, and it happens here
            // rather than in `process`. The engine itself is allocation-free
            // by construction — `CompiledRouting` is a fixed array precisely
            // so a patch change on the audio thread cannot reach an allocator.
            scratch_l: vec![0.0; max],
            scratch_r: vec![0.0; max],
        })
    }

    fn process(
        &mut self,
        _process: Process,
        mut audio: Audio,
        events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        let mut output_port = audio
            .output_port(0)
            .ok_or(PluginError::Message("No output port"))?;
        let mut out = output_port
            .channels()?
            .into_f32()
            .ok_or(PluginError::Message("Expected f32 output"))?;

        let nch = out.channel_count() as usize;
        if nch == 0 {
            return Err(PluginError::Message("Expected >= 1 output channel"));
        }
        let frames = (out.frames_count() as usize).min(self.scratch_l.len());

        // Split the block at every event batch, so a note or a macro move lands
        // at its own sample offset rather than at the block boundary.
        for batch in events.input.batch() {
            let (start, end) = batch_range(batch.sample_bounds(), frames);
            for event in batch.events() {
                match event.as_core_event() {
                    Some(CoreEventSpace::ParamValue(e)) => {
                        if let Some(pid) = e.param_id() {
                            apply_host_param(
                                &mut self.engine,
                                self.shared,
                                pid.get() as usize,
                                e.value() as f32,
                            );
                        }
                    }
                    _ => dispatch_notes(&mut EngineNotesAdapter(&mut self.engine), event),
                }
            }
            if end > start {
                self.engine.process(
                    &mut self.scratch_l[start..end],
                    &mut self.scratch_r[start..end],
                );
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

        Ok(ProcessStatus::Continue)
    }

    fn reset(&mut self) {
        self.engine.panic();
    }
}

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
        // Patch selection rebuilds the operator topology and panics every
        // voice, so it is not something a host may ramp through on its way to a
        // target value.
        if matches!(slot, Slot::Patch | Slot::Quality) {
            flags |= ParamInfoFlags::REQUIRES_PROCESS;
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
        let mut s = String::new();
        params::write_value_text(slot, value as f32, &mut s);
        writer.write_str(&s)
    }

    fn text_to_value(&mut self, param_id: ClapId, text: &CStr) -> Option<f64> {
        let slot = params::decode(param_id.get() as usize)?;
        params::parse_value(slot, text.to_str().ok()?).map(|v| v as f64)
    }

    fn flush(&mut self, input: &InputEvents, _output: &mut OutputEvents) {
        // Inactive flush: there is no engine on this thread, so host writes
        // land in the cache and `activate` replays them.
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
        // Active flush, no render: straight to the engine and the cache.
        for event in input {
            if let Some(CoreEventSpace::ParamValue(e)) = event.as_core_event() {
                if let Some(pid) = e.param_id() {
                    apply_host_param(
                        &mut self.engine,
                        self.shared,
                        pid.get() as usize,
                        e.value() as f32,
                    );
                }
            }
        }
    }
}

impl PluginStateImpl for VxnMainThread<'_> {
    fn save(&mut self, output: &mut OutputStream) -> Result<(), PluginError> {
        output
            .write_all(&state::save(&self.shared.params))
            .map_err(|_| PluginError::Message("state save failed"))
    }

    fn load(&mut self, input: &mut InputStream) -> Result<(), PluginError> {
        let mut blob = Vec::new();
        std::io::Read::read_to_end(input, &mut blob)
            .map_err(|_| PluginError::Message("state read failed"))?;
        state::load(&blob, &self.shared.params)
            .map_err(|_| PluginError::Message("state parse failed"))?;
        // The audio thread picks this up on its next `activate`. A load while
        // active leaves the running engine on the old values until then, which
        // is what every host does anyway — `clap.state` load is specified as a
        // deactivated-plugin operation.
        Ok(())
    }
}

clack_export_entry!(SinglePluginEntry<VxnPlugin>);

#[cfg(test)]
mod tests {
    use super::*;
    use vxn4_engine::N_MACROS;

    /// The macro block is the whole automation surface for modulation, and the
    /// brief is explicit that routes and depths are *not* exposed. A test,
    /// because the cheapest way for that to break is someone helpfully adding
    /// "just one" route param.
    #[test]
    fn the_host_sees_macros_and_nothing_else_about_modulation() {
        let mut macros = 0;
        for id in 0..TOTAL_PARAMS {
            match params::decode(id).expect("in range") {
                Slot::Macro(_) => macros += 1,
                Slot::Patch | Slot::Quality | Slot::MasterGain => {}
            }
        }
        assert_eq!(macros, N_MACROS);
        assert_eq!(TOTAL_PARAMS, params::N_FIXED + N_MACROS);
    }

    /// Seeding a fresh engine from the cache is what makes automation set while
    /// inactive take effect from the first block.
    #[test]
    fn a_fresh_engine_is_seeded_from_the_cache() {
        let shared = VxnShared {
            params: ParamCache::new(),
            sample_rate: AtomicU32::new(48_000.0_f32.to_bits()),
        };
        shared.params.set(0, 4.0); // web
        shared.params.set(2, 0.5);
        shared.params.set(params::N_FIXED, 0.75); // macro 1

        let mut engine = Engine::new(48_000.0);
        seed_from_cache(&mut engine, &shared);

        assert_eq!(engine.patch_index(), 4);
        assert_eq!(engine.master_gain(), 0.5);
        assert_eq!(engine.macro_value(0), 0.75);
    }

    /// Re-sending the current patch value must not silence the instrument.
    /// Hosts that push their whole parameter set every block exist, and an
    /// unguarded `set_patch` would make one a permanent all-notes-off.
    #[test]
    fn resending_the_same_patch_does_not_panic_the_voices() {
        let mut engine = Engine::new(48_000.0);
        engine.set_patch(2);
        engine.note_on(60, 100);
        assert_eq!(engine.active_voices(), 1);
        for _ in 0..8 {
            apply_to_engine(&mut engine, Slot::Patch, 2.0);
        }
        assert_eq!(
            engine.active_voices(),
            1,
            "a redundant write stole the voice"
        );

        // A real change still does panic them, which is the point of the guard
        // being a guard rather than a removal.
        apply_to_engine(&mut engine, Slot::Patch, 3.0);
        assert_eq!(engine.active_voices(), 0);
    }

    /// A host write outside the declared range must not reach the engine as an
    /// out-of-range index.
    #[test]
    fn a_hostile_param_write_is_clamped_before_the_engine() {
        let shared = VxnShared {
            params: ParamCache::new(),
            sample_rate: AtomicU32::new(48_000.0_f32.to_bits()),
        };
        let mut engine = Engine::new(48_000.0);
        apply_host_param(&mut engine, &shared, 0, 1e9);
        assert_eq!(engine.patch_index(), vxn4_engine::N_PATCHES - 1);
        apply_host_param(&mut engine, &shared, 2, -50.0);
        assert_eq!(engine.master_gain(), 0.0);
        // And an id past the table is a no-op rather than a panic.
        apply_host_param(&mut engine, &shared, TOTAL_PARAMS, 1.0);
        apply_host_param(&mut engine, &shared, usize::MAX, 1.0);
    }

    /// CLAP velocity is a float in `[0, 1]`; the engine reads `0` as note-off.
    /// A host that rounds a very quiet note to zero must not have it silenced.
    #[test]
    fn a_near_zero_velocity_still_sounds() {
        let mut engine = Engine::new(48_000.0);
        engine.set_patch(1);
        EngineNotesAdapter(&mut engine).note_on(60, 0.001);
        assert_eq!(
            engine.active_voices(),
            1,
            "a quiet note was read as note-off"
        );
    }

    #[test]
    fn notes_route_through_the_adapter_both_ways() {
        let mut engine = Engine::new(48_000.0);
        engine.set_patch(1);
        {
            let mut a = EngineNotesAdapter(&mut engine);
            a.note_on(60, 0.8);
            a.note_on(64, 0.8);
        }
        assert_eq!(engine.active_voices(), 2);
        EngineNotesAdapter(&mut engine).note_off(60);
        // Still allocated — it is releasing, not gone. What matters is that the
        // call reached the engine at all.
        let (mut l, mut r) = ([0.0; 256], [0.0; 256]);
        engine.process(&mut l, &mut r);
        assert!(l.iter().all(|s| s.is_finite()));
    }
}
