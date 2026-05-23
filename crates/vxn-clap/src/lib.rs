//! VXN1 CLAP plugin shell (clack).
//!
//! Wires the framework-agnostic [`Synth`] engine to CLAP: a stereo output port,
//! a CLAP note input, the full parameter set, state save/restore, and the
//! `vxn-ui` Vizia editor via the `gui` extension. Parameters bridge the engine,
//! the host and the UI through `vxn_engine::SharedParams`; [`local::LocalParams`]
//! diffs that store to echo UI edits to the host without echoing host
//! automation back (see its module docs).

mod gui;
mod local;

use clack_extensions::gui::PluginGui;
use clack_extensions::state::{PluginState, PluginStateImpl};
use clack_extensions::{audio_ports::*, note_ports::*, params::*};
use clack_plugin::events::Match;
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::prelude::*;
use clack_plugin::stream::{InputStream, OutputStream};
use local::LocalParams;
use std::ffi::CStr;
use std::fmt::Write as _;
use std::io::{Read, Write as _};
use std::sync::Arc;
use vxn_engine::{PARAMS, ParamId, ParamKind, SharedParams, Synth};

/// Top-level plugin marker type.
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
            .register::<PluginGui>();
    }
}

impl DefaultPluginFactory for VxnPlugin {
    fn get_descriptor() -> PluginDescriptor {
        use clack_plugin::plugin::features::*;
        PluginDescriptor::new("labs.vulpus.vxn1", "VXN1").with_features([
            SYNTHESIZER,
            INSTRUMENT,
            STEREO,
        ])
    }

    fn new_shared(_host: HostSharedHandle) -> Result<VxnShared, PluginError> {
        Ok(VxnShared {
            params: Arc::new(SharedParams::new()),
        })
    }

    fn new_main_thread<'a>(
        _host: HostMainThreadHandle<'a>,
        shared: &'a VxnShared,
    ) -> Result<VxnMainThread<'a>, PluginError> {
        Ok(VxnMainThread {
            shared,
            params: LocalParams::new(&shared.params),
            gui: None,
        })
    }
}

/// Data shared between the main and audio threads. The parameter store lives
/// behind an `Arc` so the editor (created on the main thread) can hold a clone.
pub struct VxnShared {
    params: Arc<SharedParams>,
}

impl PluginShared<'_> for VxnShared {}

/// Main-thread state (parameter queries, state save/restore). Holds a local
/// parameter mirror used when the host flushes params while the plugin is
/// inactive.
pub struct VxnMainThread<'a> {
    shared: &'a VxnShared,
    params: LocalParams,
    /// The live editor window, while the GUI is open.
    gui: Option<vxn_ui::EditorHandle>,
}

impl<'a> PluginMainThread<'a, VxnShared> for VxnMainThread<'a> {}

/// Audio-thread processor: owns the synth engine, a local parameter mirror and
/// render scratch.
pub struct VxnAudioProcessor<'a> {
    synth: Synth,
    shared: &'a VxnShared,
    local: LocalParams,
    scratch_l: Vec<f32>,
    scratch_r: Vec<f32>,
}

impl<'a> PluginAudioProcessor<'a, VxnShared, VxnMainThread<'a>> for VxnAudioProcessor<'a> {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &mut VxnMainThread,
        shared: &'a VxnShared,
        audio_config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        vxn_engine::enable_flush_to_zero();
        let max = audio_config.max_frames_count as usize;
        Ok(Self {
            synth: Synth::new(audio_config.sample_rate as f32),
            local: LocalParams::new(&shared.params),
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
        // Fold UI edits made since the last process into the local mirror, then
        // drive the engine from the working values (UI + last host state).
        self.local.fetch_ui_changes(&self.shared.params);
        self.local.write_to(self.synth.params_mut());

        let mut output_port = audio
            .output_port(0)
            .ok_or(PluginError::Message("No output port"))?;
        let mut out = output_port
            .channels()?
            .into_f32()
            .ok_or(PluginError::Message("Expected f32 output"))?;

        let frames = (out.frames_count() as usize).min(self.scratch_l.len());
        let nch = out.channel_count() as usize;

        // Disjoint field borrows so event handling and rendering can coexist.
        let synth = &mut self.synth;
        let local = &mut self.local;
        let l = &mut self.scratch_l[..frames];
        let r = &mut self.scratch_r[..frames];

        for event_batch in events.input.batch() {
            for event in event_batch.events() {
                match event.as_core_event() {
                    Some(CoreEventSpace::NoteOn(e)) => {
                        if let Match::Specific(key) = e.key() {
                            synth.note_on(key as u8, e.velocity() as f32);
                        }
                    }
                    Some(CoreEventSpace::NoteOff(e)) => {
                        if let Match::Specific(key) = e.key() {
                            synth.note_off(key as u8);
                        }
                    }
                    Some(CoreEventSpace::ParamValue(_)) => {
                        // Host automation: fold into the mirror and the engine.
                        if let Some((idx, value)) = local.apply_input(event) {
                            synth.set_param(idx, value);
                        }
                    }
                    _ => {}
                }
            }
            let (sb, eb) = event_batch.sample_bounds();
            let start = match sb {
                std::ops::Bound::Included(n) => n,
                std::ops::Bound::Excluded(n) => n + 1,
                std::ops::Bound::Unbounded => 0,
            }
            .min(frames);
            let end = match eb {
                std::ops::Bound::Included(n) => n + 1,
                std::ops::Bound::Excluded(n) => n,
                std::ops::Bound::Unbounded => frames,
            }
            .min(frames);
            if start < end {
                synth.process(&mut l[start..end], &mut r[start..end]);
            }
        }

        // Copy the stereo scratch into the host's channels.
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

        // Fold host automation into the shared store (so the UI/host observe it)
        // and echo UI edits back to the host as gesture-bracketed param events.
        self.local.publish(&self.shared.params);
        self.local
            .emit(&self.shared.params, events.output, frames as u32);

        Ok(ProcessStatus::Continue)
    }

    fn reset(&mut self) {
        self.synth.reset();
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

// ── Parameters ────────────────────────────────────────────────────────────────

fn format_value(id: ParamId, value: f64, writer: &mut ParamDisplayWriter) -> std::fmt::Result {
    // Shared with the editor's value readouts so host and UI render identically.
    write!(writer, "{}", id.desc().display(value as f32))
}

impl PluginMainThreadParams for VxnMainThread<'_> {
    fn count(&mut self) -> u32 {
        ParamId::COUNT as u32
    }

    fn get_info(&mut self, param_index: u32, info: &mut ParamInfoWriter) {
        let Some(id) = ParamId::from_index(param_index as usize) else {
            return;
        };
        let desc = id.desc();
        let stepped = !matches!(desc.kind, ParamKind::Float { .. });
        let mut flags = ParamInfoFlags::IS_AUTOMATABLE;
        if stepped {
            flags |= ParamInfoFlags::IS_STEPPED;
        }
        info.set(&ParamInfo {
            id: ClapId::new(id.index() as u32),
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
        if idx < ParamId::COUNT {
            Some(self.shared.params.get(idx) as f64)
        } else {
            None
        }
    }

    fn value_to_text(
        &mut self,
        param_id: ClapId,
        value: f64,
        writer: &mut ParamDisplayWriter,
    ) -> std::fmt::Result {
        match ParamId::from_index(param_id.get() as usize) {
            Some(id) => format_value(id, value, writer),
            None => Err(std::fmt::Error),
        }
    }

    fn text_to_value(&mut self, _param_id: ClapId, text: &CStr) -> Option<f64> {
        let s = text.to_str().ok()?;
        // Take the leading numeric token (ignore any unit suffix).
        let num: String = s
            .trim()
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
            .collect();
        num.parse::<f64>().ok()
    }

    fn flush(&mut self, input: &InputEvents, _output: &mut OutputEvents) {
        // Inactive-plugin param flush (main thread): fold host changes into the
        // mirror and publish so `get_value`/the UI observe them.
        for event in input {
            self.params.apply_input(event);
        }
        self.params.publish(&self.shared.params);
    }
}

impl PluginAudioProcessorParams for VxnAudioProcessor<'_> {
    fn flush(&mut self, input: &InputEvents, _output: &mut OutputEvents) {
        for event in input {
            if let Some((idx, value)) = self.local.apply_input(event) {
                self.synth.set_param(idx, value);
            }
        }
        self.local.publish(&self.shared.params);
    }
}

// ── State save / restore ──────────────────────────────────────────────────────

impl PluginStateImpl for VxnMainThread<'_> {
    fn save(&mut self, output: &mut OutputStream) -> Result<(), PluginError> {
        for i in 0..ParamId::COUNT {
            output.write_all(&self.shared.params.get(i).to_le_bytes())?;
        }
        Ok(())
    }

    fn load(&mut self, input: &mut InputStream) -> Result<(), PluginError> {
        for i in 0..ParamId::COUNT {
            let mut buf = [0u8; 4];
            input.read_exact(&mut buf)?;
            self.shared.params.set(i, f32::from_le_bytes(buf));
        }
        Ok(())
    }
}

clack_export_entry!(SinglePluginEntry<VxnPlugin>);

// Keep the param table referenced so the linker never drops it in a thin-LTO
// cdylib build (defensive; also a compile-time check the import is used).
#[used]
static _PARAM_COUNT: usize = PARAMS.len();
