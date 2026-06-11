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

use clack_extensions::gui::PluginGui;
use clack_extensions::state::{PluginState, PluginStateImpl};
use clack_extensions::timer::{HostTimer, PluginTimer, PluginTimerImpl, TimerId};
use clack_extensions::{audio_ports::*, note_ports::*, params::*};
use clack_plugin::events::event_types::TransportFlags;
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::events::{Match, UnknownEvent};
use clack_plugin::prelude::*;
use clack_plugin::stream::{InputStream, OutputStream};
use std::ffi::CStr;
use std::fmt::Write as _;
use std::io::{Read, Write as _IoWrite};
use std::ops::Bound;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use vxn2_app::{NoopPresetStore, matrix_snapshot_event, tick_vxn2};
use vxn2_engine::engine::Engine;
use vxn2_engine::shared::SharedParams;
use vxn2_engine::{
    ParamKind, ParamModel, ScopedFlushToZero, TOTAL_PARAMS, desc_for_clap_id,
    module_for_clap_id, rate_partner_clap_id, sync_aware_display,
};
use vxn_core_app::{Controller, CorpusHandle, ParamId, ViewEvent};

use crate::local::LocalParams;

pub mod gui;
pub mod local;

/// Lock a mutex by extracting the inner value instead of unwrapping.
/// Plugin code runs with `panic = unwind`, so a panic during `tick` could
/// poison the controller mutex; we still want subsequent flushes to make
/// progress. The data is valid (the panic happened mid-write at worst).
pub(crate) fn lock_mut<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Engine control-block size in samples. The audio-thread loop slices host
/// buffers into chunks of at most this size before driving
/// `Engine::process_block` — block-rate state (LFO sampling, matrix eval,
/// EG ticks) must advance at this rate regardless of host buffer size, or
/// the 0074 level/pan ramps interpolate too coarsely and zipper returns at
/// large buffers (ticket 0075).
const CONTROL_BLOCK: usize = 32;

/// Subdivide `[start, end)` into chunks of at most [`CONTROL_BLOCK`]
/// samples. The tail chunk may be shorter; an empty range yields nothing.
fn control_chunks(start: usize, end: usize) -> impl Iterator<Item = (usize, usize)> {
    (start..end)
        .step_by(CONTROL_BLOCK)
        .map(move |p| (p, (p + CONTROL_BLOCK).min(end)))
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
        host: HostMainThreadHandle<'a>,
        shared: &'a VxnShared,
    ) -> Result<VxnMainThread<'a>, PluginError> {
        let (mut controller, view_rx, corpus) =
            Controller::new(shared.params.clone(), Box::new(NoopPresetStore));
        // Seed the preset bar with the synthetic "Init" label until the
        // preset epic (E007 lineage) ships a real factory bank.
        controller.set_init_preset_meta(Some(vxn_core_app::PresetMeta {
            name: "Init".into(),
            ..Default::default()
        }));
        Ok(VxnMainThread {
            shared,
            controller: Arc::new(Mutex::new(controller)),
            view_rx: Arc::new(Mutex::new(view_rx)),
            corpus,
            gui: None,
            host: Some(host),
            timer: None,
        })
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
    /// Wrapped in `Arc<Mutex<...>>` so the timer drain and the host
    /// `params::flush` paths share one controller without crossing
    /// threads. Both sites are main-thread, so there is no real
    /// contention.
    pub(crate) controller: Arc<Mutex<Controller<SharedParams>>>,
    /// View-bound events the controller emits. The timer drain consumes
    /// these; when the GUI is closed they stay queued and the bounded
    /// channel drops on full.
    pub(crate) view_rx: Arc<Mutex<Receiver<ViewEvent>>>,
    /// Shared snapshot of the preset corpus. Empty until the preset
    /// epic lands; the controller still publishes
    /// `ViewEvent::PresetCorpusChanged` events the page can ignore.
    pub(crate) corpus: CorpusHandle,
    /// Live editor handle while the GUI is open.
    pub(crate) gui: Option<vxn2_ui_web::EditorHandle>,
    /// Host main-thread handle. `gui::set_parent` uses it to register
    /// the host timer; `gui::destroy` to unregister. `None` only in
    /// unit-test contexts that construct a `VxnMainThread` without a
    /// real CLAP host — the GUI extension is then inert.
    pub(crate) host: Option<HostMainThreadHandle<'a>>,
    /// Editor's main-thread timer (id + the host's timer extension).
    /// `None` when the GUI is closed or the host doesn't support
    /// `timer-support`.
    pub(crate) timer: Option<(HostTimer, TimerId)>,
}

impl<'a> PluginMainThread<'a, VxnShared> for VxnMainThread<'a> {}

impl<'a> VxnMainThread<'a> {
    /// Drain the controller's view-event queue into the live WebView.
    /// No-op when the GUI is closed.
    fn drain_view_events(&mut self) {
        let Some(handle) = self.gui.as_ref() else {
            return;
        };
        let rx = lock_mut(&self.view_rx);
        while let Ok(ev) = rx.try_recv() {
            handle.push_view_event(ev);
        }
    }

    /// Drain `SharedParams`' dirty bitsets and push the resulting view
    /// events to the live editor handle. The bitset is the canonical
    /// Model → View change channel (ADR 0003): every write site —
    /// UI knob, host automation via `LocalParams::apply_input`, state
    /// load, preset load — flips a bit; this tick pops them.
    ///
    /// The pump is source-agnostic and dumb: it pushes every drift. The
    /// view's `bindGestureGated` helper (ticket 0060) drops echoes for
    /// widgets the user is currently dragging. `SharedParams.gestures`
    /// still drives plugin → host CLAP gesture brackets — different
    /// direction, separate concern. Sync flips re-emit their rate
    /// partner's display in a second pass.
    fn push_model_diffs(&mut self) {
        let Some(handle) = self.gui.as_ref() else {
            return;
        };
        for ev in drain_dirty_bits(&self.shared.params) {
            handle.push_view_event(ev);
        }
    }
}

/// Drain the dirty bitsets on `params` and return the view events the
/// editor handle should receive this tick (ADR 0003). One
/// `ParamChanged` per popped value bit, one `MatrixSnapshot` if any
/// matrix bit was set. The pump is source-agnostic — mid-gesture
/// suppression lives in the view's `bindGestureGated` helper (ticket
/// 0060). Sync flips re-emit their rate partner's display.
fn drain_dirty_bits(params: &SharedParams) -> Vec<ViewEvent> {
    let mut out: Vec<ViewEvent> = Vec::new();
    let value_bits = params.take_dirty_values();
    let mut emitted = vec![false; TOTAL_PARAMS];
    let mut force_rate_refresh: Vec<usize> = Vec::new();
    for (w, mut bits) in value_bits.iter().copied().enumerate() {
        while bits != 0 {
            let b = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let id = w * 64 + b;
            if id >= TOTAL_PARAMS {
                continue;
            }
            let plain = params.get(id);
            let norm = params.get_normalised(id);
            let display = sync_aware_display(params, id, plain);
            out.push(ViewEvent::ParamChanged {
                id: ParamId::new(id),
                plain,
                norm,
                display,
            });
            emitted[id] = true;
            if let Some(rate_id) = rate_partner_clap_id(id) {
                force_rate_refresh.push(rate_id);
            }
        }
    }
    // Refresh sync-partner rate displays only when the partner wasn't
    // already emitted in the main pass — both the rate and its sync
    // toggle can drift in the same tick (notably on the all-ones seed
    // first tick), and we don't want a duplicate ParamChanged for the
    // rate.
    for rate_id in force_rate_refresh {
        if rate_id >= TOTAL_PARAMS || emitted[rate_id] {
            continue;
        }
        let plain = params.get(rate_id);
        let norm = params.get_normalised(rate_id);
        let display = sync_aware_display(params, rate_id, plain);
        out.push(ViewEvent::ParamChanged {
            id: ParamId::new(rate_id),
            plain,
            norm,
            display,
        });
        emitted[rate_id] = true;
    }
    // Whole-table matrix snapshot when any slot bit was set. 16 rows
    // serialises cheaper than 16 row events and the view-side renderer
    // already collapses to one path.
    if params.take_dirty_matrix() != 0 {
        out.push(matrix_snapshot_event(params));
    }
    out
}

impl<'a> PluginTimerImpl for VxnMainThread<'a> {
    fn on_timer(&mut self, _id: TimerId) {
        // Pull UI-posted intents into the model first so the ViewEvents
        // they generate land in `view_rx` before we drain it — saves a
        // tick of round-trip latency on a knob drag.
        {
            let mut ctrl = lock_mut(&self.controller);
            tick_vxn2(&mut ctrl);
        }
        self.drain_view_events();
        // Drain SharedParams' dirty bitsets — the canonical Model → View
        // change channel (ADR 0003 / E005). Catches audio-thread
        // automation, host state load, preset load, and any UI write
        // that landed since the last tick under one discipline.
        self.push_model_diffs();
        // One `evaluate_script` per tick. Both pushes above only
        // buffered into the EditorHandle; this is the single bridge
        // call.
        if let Some(handle) = self.gui.as_ref() {
            handle.flush_view_events();
        }
    }
}

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
        process: Process,
        mut audio: Audio,
        events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        // FTZ for this block. Set per-process (not in `activate`) — the FP
        // control word is thread-local and the host may run `process` on a
        // different thread.
        let _ftz = ScopedFlushToZero::new();

        // Fold any UI edits made since the last block (no-op for E002 — the
        // UI write path lands in a later epic) into the local mirror, then
        // push the authoritative param set into the engine.
        self.local.fetch_ui_changes(&self.shared.params);
        self.local.write_to(self.engine.params_mut());
        self.engine.apply_block_params();

        // Host transport → engine tempo for LFO1 sync + delay sync. Read on
        // every block so BPM changes track without waiting for a reset.
        if let Some(t) = process.transport {
            if t.flags.contains(TransportFlags::HAS_TEMPO) {
                self.engine.set_tempo(t.tempo as f32);
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

        // Disjoint field borrows so the event dispatcher and the renderer can
        // coexist inside the batch loop.
        let engine = &mut self.engine;
        let local = &mut self.local;
        let shared = &self.shared.params;
        let l = &mut self.scratch_l[..frames];
        let r = &mut self.scratch_r[..frames];

        for event_batch in events.input.batch() {
            for event in event_batch.events() {
                dispatch_event(engine, local, shared, event);
            }
            let (start, end) = batch_range(event_batch.sample_bounds(), frames);
            for (a, b) in control_chunks(start, end) {
                engine.process_block(&mut l[a..b], &mut r[a..b]);
            }
        }

        // Copy stereo scratch into host channels. Mono hosts get L only —
        // an instrument port is stereo per ADR §1 / §9, so a 1-channel host
        // is out-of-spec; a naïve L+R downmix would peak.
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

        // Host automation already wrote through to the shared store inside
        // `dispatch_event` (one shared store + dirty-bit flip per event —
        // see [`LocalParams::apply_input`] / ADR 0003). Emit any UI edits
        // back to the host as ParamValue events.
        self.local
            .emit(&self.shared.params, events.output, frames as u32);

        Ok(ProcessStatus::Continue)
    }

    fn reset(&mut self) {
        self.engine.reset();
    }
}

/// Convert a clack event-batch `[start, end)` sample range into concrete
/// frame offsets, capped to the host's frame count. Mirrors VXN1's bound
/// extraction — `Unbounded` means "from start" / "to end" of the host block.
fn batch_range(bounds: (Bound<usize>, Bound<usize>), frames: usize) -> (usize, usize) {
    let (sb, eb) = bounds;
    let start = match sb {
        Bound::Included(n) => n,
        Bound::Excluded(n) => n + 1,
        Bound::Unbounded => 0,
    }
    .min(frames);
    let end = match eb {
        Bound::Included(n) => n + 1,
        Bound::Excluded(n) => n,
        Bound::Unbounded => frames,
    }
    .min(frames);
    (start, end)
}

/// Per-event dispatch: notes go straight to the engine, param-value events
/// fold into the local mirror AND write through to the shared store (so the
/// dirty bitset catches host automation), raw MIDI feeds bend / mod-wheel /
/// aftertouch.
fn dispatch_event(
    engine: &mut Engine,
    local: &mut LocalParams,
    shared: &SharedParams,
    event: &UnknownEvent,
) {
    match event.as_core_event() {
        Some(CoreEventSpace::NoteOn(e)) => {
            if let Match::Specific(key) = e.key() {
                let vel = ((e.velocity() * 127.0) as i32).clamp(1, 127) as u8;
                engine.note_on(key as u8, vel);
            }
        }
        Some(CoreEventSpace::NoteOff(e)) => {
            if let Match::Specific(key) = e.key() {
                engine.note_off(key as u8);
            }
        }
        Some(CoreEventSpace::ParamValue(_)) => {
            // Mirror + shared store: the engine re-snapshots from `LocalParams`
            // at the top of the next block, and the shared write flips a
            // dirty bit the main-thread tick will drain into a ParamChanged.
            // Sub-block accuracy at event boundaries lands with the UI epic
            // when `Engine::set_param` (per-id) is exposed.
            let _ = local.apply_input(shared, event);
        }
        Some(CoreEventSpace::Midi(e)) => {
            let [status, d1, d2] = e.data();
            match status & 0xF0 {
                0xE0 => {
                    // 14-bit bend, centre 8192 → normalised [-1, 1].
                    let raw = ((d2 as u16) << 7) | d1 as u16;
                    engine.set_pitch_bend((raw as f32 - 8192.0) / 8192.0);
                }
                0xB0 if d1 == 1 => {
                    // CC1 mod wheel. Deadzone the bottom LSB — hardware
                    // wheels rarely rest clean at 0 (mirrors VXN1).
                    let wheel = if d2 <= 1 { 0.0 } else { d2 as f32 / 127.0 };
                    engine.set_mod_wheel(wheel);
                }
                0xD0 => {
                    // Channel aftertouch: single data byte in [0, 127].
                    engine.set_aftertouch(d1 as f32 / 127.0);
                }
                _ => {}
            }
        }
        _ => {}
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
        // Same sync-aware path as the view pump (ticket 0066): a synced
        // rate shows its subdivision label in the host's automation lane,
        // matching the editor, instead of the raw Hz/ms value.
        let id = param_id.get() as usize;
        if desc_for_clap_id(id).is_none() {
            return Err(std::fmt::Error);
        }
        write!(
            writer,
            "{}",
            sync_aware_display(&self.shared.params, id, value as f32)
        )
    }

    fn text_to_value(&mut self, _param_id: ClapId, text: &CStr) -> Option<f64> {
        let s = text.to_str().ok()?;
        // Take the leading numeric token; ignore unit suffix the host hands
        // back through this field (e.g. "-6.0 dB"). Matches VXN1. Text input
        // stays Hz/ms-only: subdivision strings ("1/8") are not parsed —
        // "1/8" reads as 1.0 here, which is the documented trade-off
        // (ticket 0066); hosts use value_to_text for display and pass plain
        // values for edits.
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
        // be redundant here. `apply_input` writes through to the shared
        // store (ADR 0003), so the host's next `get_value` poll reflects
        // the automation without a separate publish pass.
        for event in input {
            let _ = self.local.apply_input(&self.shared.params, event);
        }
    }
}

// ── State save / restore ────────────────────────────────────────────────────

impl PluginStateImpl for VxnMainThread<'_> {
    fn save(&mut self, output: &mut OutputStream) -> Result<(), PluginError> {
        let blob = ParamModel::snapshot_bytes(&*self.shared.params);
        output
            .write_all(&blob)
            .map_err(|_| PluginError::Message("state save failed"))
    }

    fn load(&mut self, input: &mut InputStream) -> Result<(), PluginError> {
        let mut blob = Vec::new();
        input
            .read_to_end(&mut blob)
            .map_err(|_| PluginError::Message("state read failed"))?;
        ParamModel::load_bytes(&*self.shared.params, &blob)
            .map_err(|_| PluginError::Message("state read failed"))?;
        // `load_bytes` flips every dirty bit (values + matrix) so the
        // main-thread tick pumps a full re-broadcast — no bespoke push
        // from this site (ADR 0003 / E005).
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
        let (mut controller, view_rx, corpus) =
            Controller::new(shared.params.clone(), Box::new(NoopPresetStore));
        controller.set_init_preset_meta(Some(vxn_core_app::PresetMeta {
            name: "Init".into(),
            ..Default::default()
        }));
        VxnMainThread {
            shared,
            controller: Arc::new(Mutex::new(controller)),
            view_rx: Arc::new(Mutex::new(view_rx)),
            corpus,
            gui: None,
            host: None,
            timer: None,
        }
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

    // ── control_chunks ─────────────────────────────────────────────────────

    /// Render slicing (ticket 0075): every chunk is at most CONTROL_BLOCK
    /// long, chunks tile the range exactly, the ragged tail is preserved,
    /// and an empty batch range yields no chunks.
    #[test]
    fn control_chunks_tile_range_with_ragged_tail() {
        let chunks: Vec<_> = control_chunks(0, 512).collect();
        assert_eq!(chunks.len(), 512 / CONTROL_BLOCK);
        let mut expect = 0;
        for &(a, b) in &chunks {
            assert_eq!(a, expect);
            assert!(b - a <= CONTROL_BLOCK);
            expect = b;
        }
        assert_eq!(expect, 512);

        // Ragged: event at sample 100 splits the batch mid-chunk.
        let chunks: Vec<_> = control_chunks(100, 171).collect();
        assert_eq!(chunks, vec![(100, 132), (132, 164), (164, 171)]);

        assert_eq!(control_chunks(37, 37).count(), 0);
    }

    // ── batch_range ────────────────────────────────────────────────────────

    #[test]
    fn batch_range_unbounded_covers_full_block() {
        let (s, e) = batch_range((Bound::Unbounded, Bound::Unbounded), 256);
        assert_eq!((s, e), (0, 256));
    }

    #[test]
    fn batch_range_included_then_excluded_is_half_open() {
        let (s, e) = batch_range((Bound::Included(0), Bound::Excluded(200)), 256);
        assert_eq!((s, e), (0, 200));
    }

    #[test]
    fn batch_range_excluded_start_steps_one_sample_in() {
        let (s, _) = batch_range((Bound::Excluded(100), Bound::Unbounded), 256);
        assert_eq!(s, 101);
    }

    #[test]
    fn batch_range_included_end_is_inclusive() {
        let (_, e) = batch_range((Bound::Unbounded, Bound::Included(99)), 256);
        assert_eq!(e, 100);
    }

    #[test]
    fn batch_range_clamps_to_frame_count() {
        let (s, e) = batch_range((Bound::Included(300), Bound::Excluded(500)), 256);
        assert_eq!((s, e), (256, 256));
    }

    // ── dispatch_event ─────────────────────────────────────────────────────

    use clack_plugin::events::event_types::{MidiEvent, NoteOffEvent, NoteOnEvent};

    fn pckn_for(key: u16) -> Pckn {
        Pckn::new(0_u8, 0_u8, key as u8, 0_u8)
    }

    #[test]
    fn dispatch_note_on_starts_a_voice() {
        let shared = mk_shared();
        let mut audio = mk_audio(&shared);
        let buf = EventBuffer::with_capacity(2);
        // Borrow the event into the same `UnknownEvent` shape `process()` sees.
        let mut b = buf;
        b.push(&NoteOnEvent::new(0, pckn_for(60), 0.78));
        let evt = b.iter().next().unwrap();
        dispatch_event(&mut audio.engine, &mut audio.local, &shared.params, evt);
        let any_gated = audio.engine.alloc.stacks.iter().any(|s| s.gate);
        assert!(any_gated, "note-on did not gate a stack");
    }

    #[test]
    fn dispatch_note_off_releases_gated_voice() {
        let shared = mk_shared();
        let mut audio = mk_audio(&shared);
        audio.engine.note_on(60, 100);
        let mut b = EventBuffer::with_capacity(2);
        b.push(&NoteOffEvent::new(0, pckn_for(60), 0.0));
        dispatch_event(
            &mut audio.engine,
            &mut audio.local,
            &shared.params,
            b.iter().next().unwrap(),
        );
        // Stack stays in release tail but gate clears.
        assert!(!audio.engine.alloc.stacks.iter().any(|s| s.gate));
    }

    #[test]
    fn dispatch_midi_pitch_bend_updates_engine() {
        let shared = mk_shared();
        let mut audio = mk_audio(&shared);
        // Centre + 4096 → +0.5 of bend range (≈+1 st with ±2 default).
        let raw: u16 = 8192 + 4096;
        let d1 = (raw & 0x7F) as u8;
        let d2 = ((raw >> 7) & 0x7F) as u8;
        let mut b = EventBuffer::with_capacity(2);
        b.push(&MidiEvent::new(0, 0, [0xE0, d1, d2]));
        dispatch_event(
            &mut audio.engine,
            &mut audio.local,
            &shared.params,
            b.iter().next().unwrap(),
        );
        assert!((audio.engine.alloc.bend() - 1.0).abs() < 1e-3);
    }

    #[test]
    fn dispatch_midi_mod_wheel_and_aftertouch() {
        let shared = mk_shared();
        let mut audio = mk_audio(&shared);
        let mut b = EventBuffer::with_capacity(4);
        b.push(&MidiEvent::new(0, 0, [0xB0, 1, 64])); // CC1 = 64
        b.push(&MidiEvent::new(0, 0, [0xD0, 32, 0])); // aftertouch = 32
        for evt in b.iter() {
            dispatch_event(&mut audio.engine, &mut audio.local, &shared.params, evt);
        }
        assert!((audio.engine.mod_wheel - 64.0 / 127.0).abs() < 1e-6);
        assert!((audio.engine.aftertouch - 32.0 / 127.0).abs() < 1e-6);
    }

    #[test]
    fn dispatch_param_value_lands_in_mirror_and_shared() {
        let shared = mk_shared();
        let mut audio = mk_audio(&shared);
        let decay = id_of("reverb-decay").unwrap();
        let mut b = EventBuffer::with_capacity(2);
        b.push(&ParamValueEvent::new(
            0,
            ClapId::new(decay as u32),
            Pckn::match_all(),
            7.5,
            Cookie::empty(),
        ));
        dispatch_event(
            &mut audio.engine,
            &mut audio.local,
            &shared.params,
            b.iter().next().unwrap(),
        );
        assert!((audio.local.get(decay) - 7.5).abs() < 1e-5);
        // `apply_input` writes through to the shared store — no separate
        // publish pass is needed.
        assert!((shared.params.get(decay) - 7.5).abs() < 1e-5);
    }

    // ── process loop end-to-end ────────────────────────────────────────────

    /// Mirrors the ticket's integration scenario: note-on at sample 0,
    /// note-off at sample 200, in a 256-sample block. Drives the engine via
    /// the two split batches and checks finite + non-silent attack +
    /// decaying release tail.
    #[test]
    fn process_loop_two_batch_render_is_finite_and_non_silent() {
        let shared = mk_shared();
        let mut audio = mk_audio(&shared);
        let frames = 256;
        audio.scratch_l = vec![0.0; frames];
        audio.scratch_r = vec![0.0; frames];

        // Block N: note-on at 0, note-off at 200 (two batches: 0..200, 200..256).
        audio.engine.note_on(60, 100);
        audio.engine.process_block(
            &mut audio.scratch_l[0..200],
            &mut audio.scratch_r[0..200],
        );
        let attack_peak = audio.scratch_l[..200]
            .iter()
            .chain(audio.scratch_r[..200].iter())
            .fold(0.0_f32, |a, &x| a.max(x.abs()));
        assert!(
            attack_peak > 1e-3,
            "attack region silent: peak={attack_peak}"
        );
        for &v in audio.scratch_l[..200].iter().chain(&audio.scratch_r[..200]) {
            assert!(v.is_finite(), "non-finite attack sample");
        }

        audio.engine.note_off(60);
        audio.engine.process_block(
            &mut audio.scratch_l[200..frames],
            &mut audio.scratch_r[200..frames],
        );
        for &v in audio.scratch_l[200..frames]
            .iter()
            .chain(&audio.scratch_r[200..frames])
        {
            assert!(v.is_finite(), "non-finite release sample");
        }

        // Render ~1 second more to confirm the tail eventually decays.
        let blocks = 48_000 / CONTROL_BLOCK;
        let mut last_peak = 0.0_f32;
        let mut l = vec![0.0_f32; CONTROL_BLOCK];
        let mut r = vec![0.0_f32; CONTROL_BLOCK];
        for _ in 0..blocks {
            audio.engine.process_block(&mut l, &mut r);
        }
        for i in 0..CONTROL_BLOCK {
            last_peak = last_peak.max(l[i].abs()).max(r[i].abs());
        }
        assert!(last_peak < 0.05, "tail still audible: {last_peak}");
    }

    #[test]
    fn reset_silences_held_voice() {
        let shared = mk_shared();
        let mut audio = mk_audio(&shared);
        audio.engine.note_on(60, 100);
        // Render a bit so smoothers and FX wind up.
        audio.engine.process_block(&mut audio.scratch_l, &mut audio.scratch_r);
        audio.reset();
        // After reset: no gated stacks.
        assert!(!audio.engine.alloc.stacks.iter().any(|s| s.gate));
        // And one block of silence should now be near-zero.
        audio.engine.process_block(&mut audio.scratch_l, &mut audio.scratch_r);
        let peak = audio
            .scratch_l
            .iter()
            .chain(audio.scratch_r.iter())
            .fold(0.0_f32, |a, &x| a.max(x.abs()));
        assert!(peak < 1e-3, "reset left audible state: {peak}");
    }

    /// Host transport tempo flows into the engine via `set_tempo`.
    #[test]
    fn set_tempo_propagates_to_engine() {
        let shared = mk_shared();
        let mut audio = mk_audio(&shared);
        audio.engine.set_tempo(140.0);
        assert!((audio.engine.tempo_bpm - 140.0).abs() < 1e-6);
    }

    // ── State extension ────────────────────────────────────────────────────

    /// `save` → `load` on a fresh `SharedParams` reproduces every slot.
    #[test]
    fn plugin_state_save_load_round_trips_every_param() {
        let shared = mk_shared();
        // Spread non-default values across the table.
        for (name, v) in [
            ("op1-num", 3.0_f32),
            ("op6-level", 88.0),
            ("op4-pan", -0.7),
            ("mtx1-depth", 0.4),
            ("mtx8-depth", -0.7),
            ("master-volume", -3.0),
            ("reverb-decay", 4.5),
            ("delay-time", 250.0),
            ("assign-mode", 1.0),
            ("glide-time", 200.0),
        ] {
            let id = id_of(name).unwrap();
            shared.params.set(id, v);
        }

        let mut main = mk_main(&shared);
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut out = OutputStream::from_writer(&mut buf);
            main.save(&mut out).unwrap();
        }

        let restored = mk_shared();
        let mut restored_main = mk_main(&restored);
        let mut reader: &[u8] = &buf;
        {
            let mut input = InputStream::from_reader(&mut reader);
            restored_main.load(&mut input).unwrap();
        }
        for i in 0..TOTAL_PARAMS {
            assert_eq!(
                shared.params.get(i),
                restored.params.get(i),
                "slot {i} differs"
            );
        }
    }

    /// Two consecutive `save` calls on an unchanged plugin produce identical
    /// blobs — hosts rely on this for project-dirty detection.
    #[test]
    fn plugin_state_save_is_bit_identical_across_calls() {
        let shared = mk_shared();
        shared.params.set(id_of("master-volume").unwrap(), -3.0);
        let mut main = mk_main(&shared);

        let mut a: Vec<u8> = Vec::new();
        {
            let mut out = OutputStream::from_writer(&mut a);
            main.save(&mut out).unwrap();
        }
        let mut b: Vec<u8> = Vec::new();
        {
            let mut out = OutputStream::from_writer(&mut b);
            main.save(&mut out).unwrap();
        }
        assert_eq!(a, b);
    }

    /// A value written via host automation mid-block (folded into the audio
    /// mirror and republished to the shared store via `publish`) is visible
    /// to a subsequent `save` — no stale mirror.
    #[test]
    fn plugin_state_save_sees_post_publish_automation() {
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
        let mut sink = EventBuffer::with_capacity(0);
        audio.flush(&buf.as_input(), &mut sink.as_output());

        let mut main = mk_main(&shared);
        let mut blob: Vec<u8> = Vec::new();
        {
            let mut out = OutputStream::from_writer(&mut blob);
            main.save(&mut out).unwrap();
        }

        let restored = mk_shared();
        let mut restored_main = mk_main(&restored);
        let mut reader: &[u8] = &blob;
        {
            let mut input = InputStream::from_reader(&mut reader);
            restored_main.load(&mut input).unwrap();
        }
        assert!((restored.params.get(decay) - 4.5).abs() < 1e-5);
    }

    /// Corrupt blobs surface as `PluginError::Message("state read failed")`.
    #[test]
    fn plugin_state_load_rejects_bad_magic() {
        let shared = mk_shared();
        let mut main = mk_main(&shared);
        let blob = vec![b'X', b'X', b'X', b'X'];
        let mut reader: &[u8] = &blob;
        let mut input = InputStream::from_reader(&mut reader);
        let err = main.load(&mut input).unwrap_err();
        match err {
            PluginError::Message(m) => assert_eq!(m, "state read failed"),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    // ── drain_dirty_bits (E005 / ADR 0003) ────────────────────────────────

    fn changed_ids(evs: &[ViewEvent]) -> Vec<usize> {
        evs.iter()
            .filter_map(|e| match e {
                ViewEvent::ParamChanged { id, .. } => Some(id.raw()),
                _ => None,
            })
            .collect()
    }

    fn changed_for(evs: &[ViewEvent], target: usize) -> Option<&ViewEvent> {
        evs.iter().find(|e| match e {
            ViewEvent::ParamChanged { id, .. } => id.raw() == target,
            _ => false,
        })
    }

    fn matrix_snapshot_count(evs: &[ViewEvent]) -> usize {
        evs.iter()
            .filter(|e| match e {
                ViewEvent::Custom(b) => matches!(
                    b.downcast_ref::<vxn2_app::Vxn2ViewCustom>(),
                    Some(vxn2_app::Vxn2ViewCustom::MatrixSnapshot { .. })
                ),
                _ => false,
            })
            .count()
    }

    fn assert_drains_just(params: &SharedParams, expected: &[usize]) {
        let evs = drain_dirty_bits(params);
        let mut got = changed_ids(&evs);
        let mut want = expected.to_vec();
        got.sort();
        want.sort();
        assert_eq!(got, want, "drain ids mismatch");
    }

    /// First tick after open: dirty bitset is seeded all-ones, so the
    /// pump broadcasts the full table (every CLAP id) plus one matrix
    /// snapshot. Parallels the all-NaN `last_seen` seed of the prior
    /// polling pump.
    #[test]
    fn drain_full_broadcast_on_fresh_shared_params() {
        let shared = mk_shared();
        let evs = drain_dirty_bits(&shared.params);
        let ids = changed_ids(&evs);
        assert_eq!(ids.len(), TOTAL_PARAMS, "expected full-table broadcast");
        assert_eq!(matrix_snapshot_count(&evs), 1);
        // Second drain with no intervening writes: empty.
        let evs = drain_dirty_bits(&shared.params);
        assert!(evs.is_empty(), "expected no events after seed drain: {evs:?}");
    }

    /// A single audio-thread write surfaces as exactly one
    /// `ParamChanged`. Coalescing semantics: writing the same id five
    /// times between drains still emits one event carrying the latest
    /// value.
    #[test]
    fn drain_single_change_emits_one_event_coalescing() {
        let shared = mk_shared();
        let _ = drain_dirty_bits(&shared.params); // pop the seed

        let decay = id_of("reverb-decay").unwrap();
        for v in [1.0_f32, 2.0, 3.0, 4.0, 7.5] {
            shared.params.set(decay, v);
        }
        let evs = drain_dirty_bits(&shared.params);
        assert_eq!(changed_ids(&evs), vec![decay]);
        match &evs[0] {
            ViewEvent::ParamChanged { plain, display, .. } => {
                assert!((*plain - 7.5).abs() < 1e-5);
                assert!(display.contains("7.50"), "got display {display:?}");
            }
            other => panic!("unexpected event {other:?}"),
        }
    }

    /// Writing a matrix slot meta returns one `MatrixSnapshot`, not 16
    /// individual row events. (Slot 9 → extra-depth table only, so no
    /// CLAP `mtxN-depth` bit fires.)
    #[test]
    fn drain_matrix_write_emits_one_snapshot() {
        let shared = mk_shared();
        let _ = drain_dirty_bits(&shared.params); // pop the seed

        shared.params.set_matrix_row_raw(
            9,
            vxn2_engine::MatrixRowRaw {
                source: 4, dest: 2, curve: 0, active: true, depth: 0.3,
            },
        );
        let evs = drain_dirty_bits(&shared.params);
        assert_eq!(matrix_snapshot_count(&evs), 1);
        assert_drains_just(&shared.params, &[]); // already drained
    }

    /// Per ADR 0003 / ticket 0060 the pump is source-agnostic and does
    /// NOT consult `SharedParams.gestures` — mid-drag suppression lives
    /// in the view's `bindGestureGated` helper. Writes under gesture
    /// flow through the drain like any other write.
    #[test]
    fn drain_emits_ids_under_gesture_pump_is_source_agnostic() {
        let shared = mk_shared();
        let _ = drain_dirty_bits(&shared.params);

        let decay = id_of("reverb-decay").unwrap();
        shared.params.set_gesture(decay, true);
        shared.params.set(decay, 7.5);
        let evs = drain_dirty_bits(&shared.params);
        let r = changed_for(&evs, decay).expect("pump should emit even under gesture");
        match r {
            ViewEvent::ParamChanged { plain, .. } => {
                assert!((*plain - 7.5).abs() < 1e-5);
            }
            other => panic!("unexpected event {other:?}"),
        }
    }

    /// Flipping `lfo1-sync` re-emits `lfo1-rate` in the same tick even
    /// though the rate's plain value didn't change — the display label
    /// switches from Hz to a subdivision.
    #[test]
    fn drain_lfo1_sync_flip_refreshes_rate_partner() {
        let shared = mk_shared();
        let rate = id_of("lfo1-rate").unwrap();
        let sync = id_of("lfo1-sync").unwrap();
        shared.params.set(sync, 0.0);
        let _ = drain_dirty_bits(&shared.params);

        shared.params.set(sync, 1.0);
        let evs = drain_dirty_bits(&shared.params);
        let ids = changed_ids(&evs);
        assert!(ids.contains(&sync), "sync flag missing from drain: {ids:?}");
        assert!(ids.contains(&rate), "rate partner missing from drain: {ids:?}");

        match changed_for(&evs, rate).unwrap() {
            ViewEvent::ParamChanged { display, .. } => {
                assert!(display.contains('/'), "expected subdivision, got {display:?}");
                assert!(!display.contains("Hz"), "expected synced label, got {display:?}");
            }
            other => panic!("unexpected event {other:?}"),
        }

        shared.params.set(sync, 0.0);
        let evs = drain_dirty_bits(&shared.params);
        let r = changed_for(&evs, rate).expect("rate re-emit on sync off");
        match r {
            ViewEvent::ParamChanged { display, .. } => {
                assert!(display.contains("Hz"), "expected Hz label, got {display:?}");
            }
            other => panic!("unexpected event {other:?}"),
        }
    }

    /// Same pattern for the delay's sync flag.
    #[test]
    fn drain_delay_sync_flip_refreshes_time_partner() {
        let shared = mk_shared();
        let time = id_of("delay-time").unwrap();
        let sync = id_of("delay-sync").unwrap();
        shared.params.set(sync, 0.0);
        let _ = drain_dirty_bits(&shared.params);

        shared.params.set(sync, 1.0);
        let evs = drain_dirty_bits(&shared.params);
        let r = changed_for(&evs, time).expect("time partner re-emitted");
        match r {
            ViewEvent::ParamChanged { display, .. } => {
                assert!(display.contains('/'), "expected subdivision: {display:?}");
            }
            other => panic!("unexpected event {other:?}"),
        }
    }

    /// LFO2 sync flip behaves like LFO1 / delay — flipping the flag re-emits
    /// the rate fader so its display switches between Hz and a subdivision
    /// label.
    #[test]
    fn drain_lfo2_sync_flip_refreshes_rate_partner() {
        let shared = mk_shared();
        let rate = id_of("lfo2-rate").unwrap();
        let sync = id_of("lfo2-sync").unwrap();
        shared.params.set(sync, 0.0);
        let _ = drain_dirty_bits(&shared.params);

        shared.params.set(sync, 1.0);
        let evs = drain_dirty_bits(&shared.params);
        let r = changed_for(&evs, rate).expect("rate partner missing");
        match r {
            ViewEvent::ParamChanged { display, .. } => {
                assert!(display.contains('/'), "expected subdivision, got {display:?}");
            }
            other => panic!("unexpected event {other:?}"),
        }
    }

    /// `sync_aware_display` formats a synced rate as a subdivision label
    /// matching the fader-position → SUBDIVISIONS mapping the engine uses.
    #[test]
    fn sync_aware_display_picks_subdivision_when_synced() {
        let shared = mk_shared();
        let sync = id_of("lfo1-sync").unwrap();
        let rate = id_of("lfo1-rate").unwrap();
        // Sync off: plain Hz display.
        shared.params.set(sync, 0.0);
        let s = sync_aware_display(&shared.params, rate, 2.4);
        assert!(s.contains("Hz"), "got {s:?}");
        // Sync on: subdivision label at this fader position.
        shared.params.set(sync, 1.0);
        let s = sync_aware_display(&shared.params, rate, 2.4);
        assert!(s.contains('/'), "expected subdivision, got {s:?}");
        // Sanity: a non-sync-pairable id falls through to the descriptor's
        // own display regardless of any sync flags.
        let vol = id_of("master-volume").unwrap();
        let s = sync_aware_display(&shared.params, vol, -6.0);
        assert!(s.contains("dB"), "got {s:?}");
    }

    /// Diff pump is no-op when the GUI is closed (`gui = None`). The
    /// dirty bits stay set, so the next open's first tick broadcasts the
    /// full table (the all-ones seed semantics carry over).
    #[test]
    fn push_model_diffs_noop_when_gui_closed() {
        let shared = mk_shared();
        let mut main = mk_main(&shared);
        shared.params.set(id_of("reverb-decay").unwrap(), 7.5);
        // No panic, no mutation, no allocation we can observe.
        main.push_model_diffs();
        assert!(main.gui.is_none());
        // Dirty bits still present — next open's first tick will broadcast.
        let bits = shared.params.take_dirty_values();
        assert!(bits.iter().any(|w| *w != 0), "expected dirty bits to survive");
    }
}
