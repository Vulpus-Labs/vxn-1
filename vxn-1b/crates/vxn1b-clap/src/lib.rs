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

use clack_extensions::state::{PluginState, PluginStateImpl};
use clack_extensions::{audio_ports::*, note_ports::*, params::*};
use clack_plugin::events::event_types::NoteExpressionType;
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::events::{Match, UnknownEvent};
use clack_plugin::prelude::*;
use clack_plugin::stream::{InputStream, OutputStream};
use std::ffi::CStr;
use std::io::{Read, Write};
use std::sync::Arc;
use vxn_core_app::ParamKind;
use vxn_core_clap::batch_range;
use vxn1b_engine::{Engine, SharedParams, TOTAL_PARAMS, desc_for_clap_id};

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

/// Route one CLAP event onto the engine (and, for param writes, the shared
/// store so `get_value` reflects host automation). Bespoke rather than the
/// shared `dispatch_event` because VXN1b threads MIDI channel + per-note
/// pressure the shared `EngineNotes` surface can't carry.
fn dispatch(engine: &mut Engine, shared: &SharedParams, event: &UnknownEvent) {
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
        Some(CoreEventSpace::ParamValue(e)) => {
            if let Some(pid) = e.param_id() {
                let idx = pid.get() as usize;
                let value = e.value() as f32;
                // Store first (so a concurrent main-thread `get_value` sees the
                // new value), then drive the engine.
                shared.set(idx, value);
                engine.set_param(idx, value);
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
            .register::<PluginState>();
    }
}

impl DefaultPluginFactory for VxnPlugin {
    fn get_descriptor() -> PluginDescriptor {
        use clack_plugin::plugin::features::*;
        PluginDescriptor::new("labs.vulpus.vxn1b", "VXN1b").with_features([
            INSTRUMENT,
            SYNTHESIZER,
            STEREO,
        ])
    }

    fn new_shared(_host: HostSharedHandle) -> Result<VxnShared, PluginError> {
        Ok(VxnShared {
            params: Arc::new(SharedParams::new()),
        })
    }

    fn new_main_thread<'a>(
        host: HostMainThreadHandle<'a>,
        shared: &'a VxnShared,
    ) -> Result<VxnMainThread<'a>, PluginError> {
        Ok(VxnMainThread { _host: host, shared })
    }
}

/// State shared between the main and audio threads: the lock-free param store
/// behind an `Arc` so the (future) editor can hold a clone too.
pub struct VxnShared {
    params: Arc<SharedParams>,
}

impl PluginShared<'_> for VxnShared {}

pub struct VxnMainThread<'a> {
    _host: HostMainThreadHandle<'a>,
    shared: &'a VxnShared,
}

impl<'a> PluginMainThread<'a, VxnShared> for VxnMainThread<'a> {}

/// Audio-thread processor: the engine + render scratch. Syncs its params +
/// topology from the shared store on activate and whenever a state load raises
/// the reload flag.
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
        let mut engine = Engine::new(audio_config.sample_rate as f32, max);
        // Adopt whatever the store holds (factory default, or a state loaded
        // while the plugin was inactive). Clears any stale reload flag.
        shared.params.take_reload();
        engine.load_state(shared.params.engine_state());
        Ok(Self {
            engine,
            shared,
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
        // A state/preset load that landed while active: re-sync the whole patch.
        if self.shared.params.take_reload() {
            self.engine.load_state(self.shared.params.engine_state());
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
        let shared = &self.shared.params;
        let l = &mut self.scratch_l[..frames];
        let r = &mut self.scratch_r[..frames];

        // Sub-block accurate: apply each event at its batch boundary, render the
        // segment up to the next one.
        for event_batch in events.input.batch() {
            for event in event_batch.events() {
                dispatch(engine, shared, event);
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
            module: b"",
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
        let desc = desc_for_clap_id(param_id.get() as usize).ok_or(std::fmt::Error)?;
        write!(writer, "{}", desc.display(value as f32))
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
            dispatch(&mut self.engine, &self.shared.params, event);
        }
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
    use vxn1b_engine::ParamId;

    fn param_event(id: ParamId, value: f64) -> ParamValueEvent {
        ParamValueEvent::new(
            0,
            ClapId::new(id as u32),
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

    #[test]
    fn param_value_event_updates_store_and_engine() {
        let mut engine = Engine::new(48_000.0, 512);
        let shared = SharedParams::new();
        dispatch(&mut engine, &shared, param_event(ParamId::Cutoff, 500.0).as_ref());
        assert_eq!(shared.get(ParamId::Cutoff as usize), 500.0);
        assert_eq!(engine.param(ParamId::Cutoff as usize), 500.0);
    }

    #[test]
    fn note_on_event_makes_sound() {
        let mut engine = Engine::new(48_000.0, 512);
        let shared = SharedParams::new();
        engine.set_param(ParamId::Env2Attack as usize, 0.001);
        dispatch(&mut engine, &shared, note_on(0, 60, 1.0).as_ref());
        assert!(peak(&mut engine, 512) > 0.0, "a dispatched note must sound");
    }

    #[test]
    fn slot_depth_param_event_moves_modulation_through_the_shell() {
        // 0204 acceptance: automating a slot depth through the CLAP param path
        // changes the sound. Zeroing the default Env2→Amp slot silences the note.
        let mut engine = Engine::new(48_000.0, 512);
        let shared = SharedParams::new();
        engine.set_param(ParamId::Env2Attack as usize, 0.001);
        dispatch(&mut engine, &shared, param_event(ParamId::MatrixSlot0Depth, 0.0).as_ref());
        dispatch(&mut engine, &shared, note_on(0, 60, 1.0).as_ref());
        assert_eq!(peak(&mut engine, 512), 0.0, "zeroed amp-slot depth ⇒ silence");
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
