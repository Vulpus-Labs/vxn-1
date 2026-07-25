//! VXN1b CLAP plugin shell (clack) — scaffold (0197).
//!
//! A host-loadable stub: one stereo output port, one note input port (CLAP +
//! MIDI dialects), and the silent [`vxn1b_engine::Engine`]. It carries VXN1b's
//! own stable plugin id (`labs.vulpus.vxn1b`), distinct from vxn-1/2/3
//! (ADR 0001 §1). Parameters, state save/restore, the GUI faceplate, and the
//! mod-matrix routing all land in later tickets (0200–0204); this crate exists
//! so the product bundles and loads in a host now.
//!
//! Structurally the leanest of the family shells (mirrors `vxn-3/crates/vxn3-clap`
//! minus params/state/gui/timer) — 0204 grows it into the full shell.

use clack_extensions::audio_ports::{
    AudioPortFlags, AudioPortInfo, AudioPortInfoWriter, AudioPortType, PluginAudioPorts,
    PluginAudioPortsImpl,
};
use clack_extensions::note_ports::{
    NoteDialect, NoteDialects, NotePortInfo, NotePortInfoWriter, PluginNotePorts,
    PluginNotePortsImpl,
};
use clack_plugin::prelude::*;
use vxn1b_engine::Engine;

pub struct VxnPlugin;

impl Plugin for VxnPlugin {
    type AudioProcessor<'a> = VxnAudioProcessor<'a>;
    type Shared<'a> = VxnShared;
    type MainThread<'a> = VxnMainThread<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&VxnShared>) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginNotePorts>();
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
        Ok(VxnShared)
    }

    fn new_main_thread<'a>(
        host: HostMainThreadHandle<'a>,
        _shared: &'a VxnShared,
    ) -> Result<VxnMainThread<'a>, PluginError> {
        Ok(VxnMainThread { _host: host })
    }
}

/// No cross-thread state yet — the param cache / engine I/O bundle lands with
/// the param table (0200) and matrix (0201/0202) tickets.
pub struct VxnShared;

impl PluginShared<'_> for VxnShared {}

pub struct VxnMainThread<'a> {
    _host: HostMainThreadHandle<'a>,
}

impl<'a> PluginMainThread<'a, VxnShared> for VxnMainThread<'a> {}

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

pub struct VxnAudioProcessor<'a> {
    engine: Engine,
    _shared: &'a VxnShared,
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
        Ok(Self {
            engine: Engine::new(sr, max),
            _shared: shared,
            scratch_l: vec![0.0; max],
            scratch_r: vec![0.0; max],
        })
    }

    fn process(
        &mut self,
        _process: Process,
        mut audio: Audio,
        _events: Events,
    ) -> Result<ProcessStatus, PluginError> {
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
