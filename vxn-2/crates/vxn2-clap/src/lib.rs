//! VXN2 CLAP plugin shell (clack).
//!
//! Scaffolds the host-loadable plugin: descriptor, audio + note ports,
//! parameter/state extension registration, and an empty `process()` that
//! returns silence. Later tickets in epic E002 attach the real param table
//! (0014/0015), the event + render loop (0016) and state save/restore
//! (0017). The GUI surface lives in its own epic.
//!
//! Structurally mirrors `vxn-1/crates/vxn-clap`: same `Shared` / `MainThread`
//! / `AudioProcessor` split, same `declare_extensions` shape — minus the
//! `Controller` / `ViewEvent` / GUI / timer machinery.

use clack_extensions::state::{PluginState, PluginStateImpl};
use clack_extensions::{audio_ports::*, note_ports::*, params::*};
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::prelude::*;
use clack_plugin::stream::{InputStream, OutputStream};
use std::ffi::CStr;
use std::fmt::Write as _;
use std::sync::Arc;
use vxn2_engine::engine::Engine;
use vxn2_engine::shared::SharedParams;
use vxn2_engine::{
    ParamDesc, ParamKind, ScopedFlushToZero, TOTAL_PARAMS, desc_for_clap_id, module_for_clap_id,
};

use crate::local::LocalParams;

pub mod local;

/// Engine control-block size in samples. The audio-thread loop in 0016 will
/// slice host buffers into chunks of at most this size before driving
/// `Engine::process_block`.
const CONTROL_BLOCK: usize = 32;

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
        PluginDescriptor::new("labs.vulpus.vxn2", "VXN2").with_features([
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
        Ok(VxnMainThread { shared })
    }
}

/// Data shared between the main and audio threads. The param store lives
/// behind an `Arc` so future editor / state code (main thread) can hold a
/// clone without reaching across the audio boundary.
pub struct VxnShared {
    params: Arc<SharedParams>,
}

impl PluginShared<'_> for VxnShared {}

pub struct VxnMainThread<'a> {
    shared: &'a VxnShared,
}

impl<'a> PluginMainThread<'a, VxnShared> for VxnMainThread<'a> {}

#[allow(dead_code)] // engine + scratch_r wired in 0016 (process loop)
pub struct VxnAudioProcessor<'a> {
    engine: Engine,
    shared: &'a VxnShared,
    /// Audio-thread parameter mirror. Host param events fold into it; 0016
    /// pushes the mirror into the engine at the top of each block.
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
        let max = audio_config.max_frames_count as usize;
        Ok(Self {
            engine: Engine::new(audio_config.sample_rate as f32, CONTROL_BLOCK),
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
        _events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        // FTZ for this block. Set per-process (not in `activate`) — the FP
        // control word is thread-local and the host may run `process` on a
        // different thread.
        let _ftz = ScopedFlushToZero::new();

        let mut output_port = audio
            .output_port(0)
            .ok_or(PluginError::Message("No output port"))?;
        let mut out = output_port
            .channels()?
            .into_f32()
            .ok_or(PluginError::Message("Expected f32 output"))?;

        let frames = (out.frames_count() as usize).min(self.scratch_l.len());
        let nch = out.channel_count() as usize;

        // 0016 will drive the engine and copy real samples here. For now,
        // zero the host's channels so the plugin loads cleanly and emits
        // silence rather than whatever garbage the host buffer arrived with.
        if let Some(ch) = out.channel_mut(0) {
            let n = ch.len().min(frames);
            ch[..n].fill(0.0);
        }
        if nch >= 2 {
            if let Some(ch) = out.channel_mut(1) {
                let n = ch.len().min(frames);
                ch[..n].fill(0.0);
            }
        }

        Ok(ProcessStatus::Continue)
    }

    fn reset(&mut self) {
        // Engine reset lands when the real driver does (0016).
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

/// Format `value` through the descriptor and write it via the host's display
/// writer. Sync-aware substitution (delay time as `1/8` when `delay_sync` is
/// on) slots in here in the UI epic — intercept before falling through to
/// `desc.display`.
fn format_value(
    desc: &ParamDesc,
    value: f64,
    writer: &mut ParamDisplayWriter,
) -> std::fmt::Result {
    write!(writer, "{}", desc.display(value as f32))
}

impl PluginMainThreadParams for VxnMainThread<'_> {
    fn count(&mut self) -> u32 {
        TOTAL_PARAMS as u32
    }

    fn get_info(&mut self, param_index: u32, info: &mut ParamInfoWriter) {
        let idx = param_index as usize;
        let Some(desc) = desc_for_clap_id(idx) else {
            return;
        };
        let stepped = !matches!(desc.kind, ParamKind::Float { .. });
        let mut flags = ParamInfoFlags::IS_AUTOMATABLE;
        if stepped {
            flags |= ParamInfoFlags::IS_STEPPED;
        }
        info.set(&ParamInfo {
            id: ClapId::new(idx as u32),
            flags,
            cookie: Default::default(),
            name: desc.name.as_bytes(),
            module: module_for_clap_id(idx).as_bytes(),
            min_value: desc.min as f64,
            max_value: desc.max as f64,
            default_value: desc.default as f64,
        });
    }

    fn get_value(&mut self, param_id: ClapId) -> Option<f64> {
        let idx = param_id.get() as usize;
        if idx < TOTAL_PARAMS {
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
        let id = param_id.get() as usize;
        let Some(desc) = desc_for_clap_id(id) else {
            return Err(std::fmt::Error);
        };
        format_value(desc, value, writer)
    }

    fn text_to_value(&mut self, _param_id: ClapId, text: &CStr) -> Option<f64> {
        let s = text.to_str().ok()?;
        // Take the leading numeric token; ignore unit suffix the host hands
        // back through this field (e.g. "-6.0 dB"). Matches VXN1.
        let num: String = s
            .trim()
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
            .collect();
        num.parse::<f64>().ok()
    }

    fn flush(&mut self, input: &InputEvents, _output: &mut OutputEvents) {
        // Main-thread / inactive-plugin flush. No Controller in E002, so
        // fold host param events straight into the shared store; the next
        // audio-thread activation rebuilds `LocalParams` from it.
        for event in input {
            if let Some(CoreEventSpace::ParamValue(e)) = event.as_core_event() {
                if let Some(pid) = e.param_id() {
                    let id = pid.get() as usize;
                    if id < TOTAL_PARAMS {
                        self.shared.params.set(id, e.value() as f32);
                    }
                }
            }
        }
    }
}

impl PluginAudioProcessorParams for VxnAudioProcessor<'_> {
    fn flush(&mut self, input: &InputEvents, _output: &mut OutputEvents) {
        // No render happens during `flush`; VXN2's engine refreshes from
        // `LocalParams` at the top of each `process()` (ticket 0016), so a
        // per-event engine setter (the VXN1 `synth.set_param` pattern) would
        // be redundant here. Fold into the mirror, then republish flagged
        // slots to the shared store so the host's next `get_value` poll
        // reflects the automation it just sent.
        for event in input {
            let _ = self.local.apply_input(event);
        }
        self.local.publish(&self.shared.params);
    }
}

// ── State (stub: 0017 wires the real save/restore) ──────────────────────────

impl PluginStateImpl for VxnMainThread<'_> {
    fn save(&mut self, _output: &mut OutputStream) -> Result<(), PluginError> {
        Ok(())
    }

    fn load(&mut self, _input: &mut InputStream) -> Result<(), PluginError> {
        Ok(())
    }
}

clack_export_entry!(SinglePluginEntry<VxnPlugin>);

#[cfg(test)]
mod tests {
    use super::*;
    use clack_plugin::events::Pckn;
    use clack_plugin::events::event_types::ParamValueEvent;
    use clack_plugin::events::io::EventBuffer;
    use clack_plugin::utils::Cookie;
    use vxn2_engine::params::id_of;

    fn mk_main<'a>(shared: &'a VxnShared) -> VxnMainThread<'a> {
        VxnMainThread { shared }
    }

    fn mk_audio<'a>(shared: &'a VxnShared) -> VxnAudioProcessor<'a> {
        VxnAudioProcessor {
            engine: Engine::new(48_000.0, CONTROL_BLOCK),
            local: LocalParams::new(&shared.params),
            shared,
            scratch_l: vec![0.0; 64],
            scratch_r: vec![0.0; 64],
        }
    }

    fn mk_shared() -> VxnShared {
        VxnShared {
            params: Arc::new(SharedParams::new()),
        }
    }

    /// Host `flush` on the main thread folds a `ParamValue` event straight
    /// into the shared store.
    #[test]
    fn main_thread_flush_writes_shared_store() {
        let shared = mk_shared();
        let mut main = mk_main(&shared);
        let vol = id_of("master-volume").unwrap();

        let mut buf = EventBuffer::with_capacity(2);
        buf.push(&ParamValueEvent::new(
            0,
            ClapId::new(vol as u32),
            Pckn::match_all(),
            -3.0,
            Cookie::empty(),
        ));
        let mut out = EventBuffer::with_capacity(0);
        main.flush(&buf.as_input(), &mut out.as_output());

        assert_eq!(shared.params.get(vol), -3.0);
    }

    /// Audio-thread `flush` folds events through the local mirror and
    /// publishes flagged slots to the shared store.
    #[test]
    fn audio_thread_flush_publishes_to_shared_store() {
        let shared = mk_shared();
        let mut audio = mk_audio(&shared);
        let decay = id_of("reverb-decay").unwrap();

        let mut buf = EventBuffer::with_capacity(2);
        buf.push(&ParamValueEvent::new(
            0,
            ClapId::new(decay as u32),
            Pckn::match_all(),
            4.5,
            Cookie::empty(),
        ));
        let mut out = EventBuffer::with_capacity(0);
        audio.flush(&buf.as_input(), &mut out.as_output());

        assert!((shared.params.get(decay) - 4.5).abs() < 1e-5);
    }

    /// `get_value` for known + unknown ids.
    #[test]
    fn main_thread_get_value() {
        let shared = mk_shared();
        let mut main = mk_main(&shared);
        let vol = id_of("master-volume").unwrap();

        let v = main.get_value(ClapId::new(vol as u32));
        assert_eq!(v, Some(-6.0));

        assert!(main.get_value(ClapId::new(TOTAL_PARAMS as u32)).is_none());
    }

    /// `text_to_value` strips a unit suffix the host hands back through the
    /// display field.
    #[test]
    fn text_to_value_ignores_unit_suffix() {
        let shared = mk_shared();
        let mut main = mk_main(&shared);
        let vol_id = ClapId::new(id_of("master-volume").unwrap() as u32);

        let s = std::ffi::CString::new("-6.0 dB").unwrap();
        let v = main.text_to_value(vol_id, s.as_c_str()).unwrap();
        assert!((v - (-6.0)).abs() < 1e-9);
    }

    /// `count` exposes the full table.
    #[test]
    fn main_thread_count_matches_total() {
        let shared = mk_shared();
        let mut main = mk_main(&shared);
        assert_eq!(
            PluginMainThreadParams::count(&mut main) as usize,
            TOTAL_PARAMS
        );
    }
}
