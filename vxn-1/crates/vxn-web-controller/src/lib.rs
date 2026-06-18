//! VXN1 web main-thread controller (ticket 0044).
//!
//! The real promotion of the 0036 probe: a raw C-ABI `cdylib` that runs the
//! existing `vxn_app::Controller` (the sole off-audio model mutator, ADR 0007)
//! on the browser main thread, reused VERBATIM — one source of truth for model
//! mutation across native and web (ADR 0009 §1).
//!
//! Boundary contract: `UiEvent` / `ViewEvent` do NOT cross the JS↔wasm boundary
//! as Rust enums. The glue (`controller.mjs`) drives a narrow, explicit C-ABI
//! **opcode surface**:
//!
//! - hot path, alloc-free: [`vxnc_ui_set_param_norm`], [`vxnc_ui_set_param`],
//!   [`vxnc_ui_begin_gesture`], [`vxnc_ui_end_gesture`], [`vxnc_ui_editor_ready`]
//!   — 1:1 with `UiEvent::{SetParamNorm, SetParam, BeginGesture, EndGesture,
//!   EditorReady}`.
//! - per-synth custom (the `Box<dyn Any>` downcast stays inside wasm):
//!   [`vxnc_ui_set_key_mode`], [`vxnc_ui_set_split_point`],
//!   [`vxnc_ui_set_edit_layer`], [`vxnc_ui_reset_layer`].
//! - [`vxnc_tick`] drains the inbound queues into the model, then drains the
//!   resulting `ViewEvent`s into a linear-memory scratch buffer the JS side
//!   reads (the web analogue of the native `flush_view_events` single bridge
//!   call). `ViewEvent::ParamChanged{id, plain, norm, display}` is packed; the
//!   `display` string rides as a length-prefixed UTF-8 slice.
//!
//! Param SAB sharing (ADR 0009 §2): the controller owns the AUTHORITATIVE param
//! values in its own linear memory ([`WebModel`], one `AtomicU32` per CLAP id,
//! f32 bit-cast — the `SharedParams` shape). The controller wasm and the engine
//! wasm have separate linear memories, so neither can map the other's heap; the
//! JS glue mirrors this model's value array into the dedicated 0039 store SAB
//! the worklet reads, and feeds the worklet's readback region back in via the
//! diff pump. Two mirror exports make that cheap:
//!
//! - [`vxnc_param_values_ptr`] — pointer to the f32[TOTAL] current-value array;
//!   JS copies changed slots into the store SAB after each tick.
//! - [`vxnc_pump_readback`] — JS copies the readback SAB into
//!   [`vxnc_readback_in_ptr`] and calls this; the controller diffs it against
//!   an internal `last_seen[TOTAL]` mirror (NaN-seeded) and emits
//!   `ViewEvent::ParamChanged` for audio-thread drift — the port of
//!   `vxn-clap`'s `push_param_diffs`. The emitted records carry the correct
//!   `norm`/`display` from the param descriptor (the controller owns them),
//!   filling the TODO(E018) the JS `pollDiffs` stubbed.
//!
//! No wasm-bindgen — instantiated with a plain `WebAssembly.instantiate`, same
//! approach as the 0034 engine spike, so the module stays scope-clean.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};

use vxn_app::{
    Controller, KeyMode, Layer, ParamDesc, ParamId, ParamModel, PresetLoad, PresetMeta,
    PresetStore, TOTAL_PARAMS, UiEvent, UserFolderEntry, ViewEvent, Vxn1Params, Vxn1UiCustom,
    desc_for_clap_id,
};

// ===========================================================================
// WebModel — the controller-side param store (the native SharedParams shape)
// ===========================================================================
//
// One AtomicU32 per CLAP id holding the PLAIN f32 value bit-cast, exactly like
// vxn-engine's SharedParams and the 0036 probe's ProbeModel. This is the
// AUTHORITATIVE current value on the controller side; the JS glue mirrors it
// into the 0039 store SAB the worklet reads. KeyMode / split point are
// non-automatable shared state (ADR 0003 §3) and live OUTSIDE the 165.

struct WebModel {
    vals: Vec<AtomicU32>,
    gestures: Vec<AtomicBool>,
    key_mode: AtomicU32,
    split: AtomicU32,
}

impl WebModel {
    fn new() -> Self {
        let mut vals = Vec::with_capacity(TOTAL_PARAMS);
        let mut gestures = Vec::with_capacity(TOTAL_PARAMS);
        for i in 0..TOTAL_PARAMS {
            let d = desc_for_clap_id(i).map_or(0.0, |d| d.default);
            vals.push(AtomicU32::new(d.to_bits()));
            gestures.push(AtomicBool::new(false));
        }
        Self {
            vals,
            gestures,
            key_mode: AtomicU32::new(0),
            split: AtomicU32::new(vxn_app::DEFAULT_SPLIT_POINT as u32),
        }
    }
}

impl ParamModel for WebModel {
    fn total(&self) -> usize {
        TOTAL_PARAMS
    }
    fn get(&self, id: ParamId) -> f32 {
        f32::from_bits(self.vals[id.raw()].load(Ordering::Relaxed))
    }
    fn set(&self, id: ParamId, plain: f32) {
        self.vals[id.raw()].store(plain.to_bits(), Ordering::Relaxed);
    }
    fn get_normalized(&self, id: ParamId) -> f32 {
        self.descriptor(id).map_or(0.0, |d| d.to_fader(self.get(id)))
    }
    fn set_normalized(&self, id: ParamId, norm: f32) {
        let plain = self.descriptor(id).map_or(norm, |d| d.from_fader(norm));
        self.set(id, plain);
    }
    fn gesture(&self, id: ParamId) -> bool {
        self.gestures[id.raw()].load(Ordering::Relaxed)
    }
    fn set_gesture(&self, id: ParamId, on: bool) {
        self.gestures[id.raw()].store(on, Ordering::Relaxed);
    }
    fn descriptor(&self, id: ParamId) -> Option<&'static ParamDesc> {
        desc_for_clap_id(id.raw())
    }
    fn snapshot_bytes(&self) -> Vec<u8> {
        // Canonical VXN1 state blob via the shared `vxn-app` codec — the same
        // format native CLAP host state and factory presets use, so blobs
        // round-trip across native and wasm (E019 / 0062).
        vxn_app::write_state_bytes(self)
    }
    fn restore_from_bytes(&self, blob: &[u8]) -> Result<(), String> {
        vxn_app::read_state_into(self, blob)
    }
}

impl Vxn1Params for WebModel {
    fn key_mode(&self) -> KeyMode {
        match self.key_mode.load(Ordering::Relaxed) {
            1 => KeyMode::Dual,
            2 => KeyMode::Split,
            _ => KeyMode::Whole,
        }
    }
    fn set_key_mode(&self, mode: KeyMode) {
        self.key_mode.store(mode as u32, Ordering::Relaxed);
    }
    fn set_key_mode_seeded(&self, mode: KeyMode) {
        // Whole -> non-Whole copies Upper into Lower so the lower layer starts
        // equal to the upper before diverging (matches vxn-clap's LocalParams).
        if self.key_mode() == KeyMode::Whole && mode != KeyMode::Whole {
            for p in 0..vxn_app::PATCH_COUNT {
                let upper = self.vals[p].load(Ordering::Relaxed);
                self.vals[vxn_app::PATCH_COUNT + p].store(upper, Ordering::Relaxed);
            }
        }
        self.set_key_mode(mode);
    }
    fn split_point(&self) -> u8 {
        self.split.load(Ordering::Relaxed) as u8
    }
    fn set_split_point(&self, note: u8) {
        self.split.store(note as u32, Ordering::Relaxed);
    }
}

/// No-op preset store. The web `PresetStore` (IndexedDB-backed) is E019; until
/// then the controller runs with this null store and preset ops are inert
/// (ADR 0009 §1). Identical to the probe's `NullStore`.
struct NullStore;

impl PresetStore for NullStore {
    fn factory_len(&self) -> usize {
        0
    }
    fn factory_load(&self, _index: usize) -> Result<PresetLoad, String> {
        Err("no presets".into())
    }
    fn factory_meta(&self, _index: usize) -> Option<PresetMeta> {
        None
    }
    fn user_load(&self, _path: &Path) -> Result<PresetLoad, String> {
        Err("no presets".into())
    }
    fn user_save(
        &self,
        _name: &str,
        _folder: Option<&str>,
        _meta: &PresetMeta,
        _blob: &[u8],
    ) -> Result<PathBuf, String> {
        Err("readonly".into())
    }
    fn user_delete(&self, _path: &Path) -> Result<(), String> {
        Err("readonly".into())
    }
    fn user_rename(&self, _path: &Path, _new_name: &str) -> Result<PathBuf, String> {
        Err("readonly".into())
    }
    fn user_move(&self, _path: &Path, _dest_folder: Option<&str>) -> Result<PathBuf, String> {
        Err("readonly".into())
    }
    fn user_create_folder(&self, _suggested: &str) -> Result<(PathBuf, String), String> {
        Err("readonly".into())
    }
    fn user_rename_folder(&self, _old: &str, _new: &str) -> Result<(PathBuf, String), String> {
        Err("readonly".into())
    }
    fn user_delete_folder(&self, _name: &str) -> Result<(), String> {
        Err("readonly".into())
    }
    fn list_user_tree(&self) -> Vec<UserFolderEntry> {
        Vec::new()
    }
}

// ===========================================================================
// ViewEvent out-buffer — the single-bridge-call drain
// ===========================================================================
//
// `tick()` drains `view_rx` into this scratch buffer; JS reads it out once per
// tick (the web analogue of the native `flush_view_events` one-call discipline).
// Packed little-endian, self-describing so JS needs no second wasm call to
// size it.
//
//   header:  u32 record_count
//   then `record_count` records, each:
//     u32 tag          VE_* below
//     ... tag-specific payload ...
//
//   VE_PARAM_CHANGED (1):
//     u32 id
//     f32 plain
//     f32 norm
//     u32 display_len   (UTF-8 byte length)
//     [display_len bytes UTF-8]    (display string)
//   VE_KEY_MODE_CHANGED (2):   u32 mode
//   VE_SPLIT_POINT_CHANGED (3): u32 note
//   VE_EDIT_LAYER_CHANGED (4):  u32 layer
//
// Out-of-scope ViewEvents (preset / status / text-input — E018/E019) are
// skipped here: this ticket delivers the param + non-param-shared-state
// transport + a smoke sink, not the full UI marshalling.

const VE_PARAM_CHANGED: u32 = 1;
const VE_KEY_MODE_CHANGED: u32 = 2;
const VE_SPLIT_POINT_CHANGED: u32 = 3;
const VE_EDIT_LAYER_CHANGED: u32 = 4;

fn push_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn push_f32(buf: &mut Vec<u8>, v: f32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

// ===========================================================================
// Controller state — one global instance (single-threaded main thread)
// ===========================================================================
//
// The native shell's `Arc<Mutex<Controller>>` collapses to a plain owned value
// here: the browser main thread is single-threaded and no second thread touches
// it (the `Mutex` is RefCell-discipline). The instance is a leaked Box; its
// pointer is the opaque handle every opcode passes back, like the engine wasm.

struct ControllerState {
    ctrl: Controller<WebModel>,
    model: Arc<WebModel>,
    view_rx: Receiver<ViewEvent>,
    ui_tx: SyncSender<UiEvent>,
    host_tx: SyncSender<vxn_app::HostEvent>,
    /// Snapshot of the model's plain values, exported to JS for SAB mirroring.
    values_out: Vec<f32>,
    /// Staging buffer JS copies the readback SAB into before `vxnc_pump_readback`.
    readback_in: Vec<f32>,
    /// Diff-pump mirror — port of vxn-clap `last_seen`. NaN-seeded so the first
    /// pump after open broadcasts the whole table (NaN != NaN).
    last_seen: Vec<f32>,
    /// Packed ViewEvent drain buffer JS reads after each tick.
    view_out: Vec<u8>,
}

impl ControllerState {
    fn new() -> Box<Self> {
        let model = Arc::new(WebModel::new());
        let (ctrl, view_rx, _corpus) = Controller::new(model.clone(), Box::new(NullStore));
        let ui_tx = ctrl.ui_sender();
        let host_tx = ctrl.host_sender();
        Box::new(Self {
            ctrl,
            model,
            view_rx,
            ui_tx,
            host_tx,
            values_out: vec![0.0; TOTAL_PARAMS],
            readback_in: vec![0.0; TOTAL_PARAMS],
            last_seen: vec![f32::NAN; TOTAL_PARAMS],
            view_out: Vec::with_capacity(8 * 1024),
        })
    }

    #[inline]
    fn post(&self, ev: UiEvent) {
        let _ = self.ui_tx.try_send(ev);
    }

    /// Drain inbound queues into the model, then pack the resulting ViewEvents
    /// into `view_out`. Also refreshes `values_out` so JS can mirror the model
    /// into the store SAB. The two pushes (controller emit + diff pump) can echo
    /// the same param twice; JS dedupes by id on consumption, as the native
    /// WebView does — costs nothing on the wire.
    fn tick(&mut self) {
        self.ctrl.tick();
        self.drain_view_events();
        // Refresh the JS-visible value snapshot AFTER the tick so a mirror pass
        // sees every model write this tick produced.
        for id in 0..TOTAL_PARAMS {
            self.values_out[id] = self.model.get(ParamId::new(id));
        }
    }

    fn drain_view_events(&mut self) {
        self.view_out.clear();
        push_u32(&mut self.view_out, 0); // count placeholder
        let mut count = 0u32;
        while let Ok(ev) = self.view_rx.try_recv() {
            match ev {
                ViewEvent::ParamChanged {
                    id,
                    plain,
                    norm,
                    display,
                } => {
                    push_u32(&mut self.view_out, VE_PARAM_CHANGED);
                    push_u32(&mut self.view_out, id.raw() as u32);
                    push_f32(&mut self.view_out, plain);
                    push_f32(&mut self.view_out, norm);
                    let bytes = display.as_bytes();
                    push_u32(&mut self.view_out, bytes.len() as u32);
                    self.view_out.extend_from_slice(bytes);
                    count += 1;
                }
                ViewEvent::Custom(payload) => {
                    // Downcast stays inside wasm (ADR 0009 §1); per-synth view
                    // state becomes a narrow opcode record, never Box<dyn Any>.
                    if let Ok(custom) = payload.downcast::<vxn_app::Vxn1ViewCustom>() {
                        match *custom {
                            vxn_app::Vxn1ViewCustom::KeyModeChanged { mode } => {
                                push_u32(&mut self.view_out, VE_KEY_MODE_CHANGED);
                                push_u32(&mut self.view_out, mode as u32);
                                count += 1;
                            }
                            vxn_app::Vxn1ViewCustom::SplitPointChanged { note } => {
                                push_u32(&mut self.view_out, VE_SPLIT_POINT_CHANGED);
                                push_u32(&mut self.view_out, note as u32);
                                count += 1;
                            }
                            vxn_app::Vxn1ViewCustom::EditLayerChanged { layer } => {
                                push_u32(&mut self.view_out, VE_EDIT_LAYER_CHANGED);
                                push_u32(&mut self.view_out, layer as u32);
                                count += 1;
                            }
                        }
                    }
                }
                // Preset / status / text-input ViewEvents are E018/E019; skipped
                // by this transport (smoke sink only).
                _ => {}
            }
        }
        // Backpatch the record count into the header.
        self.view_out[0..4].copy_from_slice(&count.to_le_bytes());
    }

    /// Port of vxn-clap `push_param_diffs`: diff `readback_in` (the values the
    /// worklet actually applied, copied from the readback SAB by JS) against
    /// `last_seen`, set the model + emit `ParamChanged` for any drift the
    /// controller never processed (host-automation echo / modulation). NaN-seed
    /// forces a full broadcast on the first pump. Routes through the controller
    /// as `HostEvent::ParamAutomation` so the gesture-suppression rule holds
    /// (a write to a param the user is dragging is swallowed, not yanked) — the
    /// emitted ParamChanged then lands in `view_rx` and is packed on the next
    /// `tick()`.
    fn pump_readback(&mut self) {
        for i in 0..TOTAL_PARAMS {
            let plain = self.readback_in[i];
            // NaN-aware compare, exactly like the native pump.
            if plain == self.last_seen[i] {
                continue;
            }
            self.last_seen[i] = plain;
            let _ = self.host_tx.try_send(vxn_app::HostEvent::ParamAutomation {
                id: ParamId::new(i),
                plain,
            });
        }
    }
}

/// Single global controller instance pointer. Set by [`vxnc_new`]; every opcode
/// dereferences it. One-instance-per-page (the browser main thread hosts one
/// controller), so a global handle is sufficient and keeps the JS glue flat.
static mut STATE: *mut ControllerState = core::ptr::null_mut();

#[inline]
fn state() -> &'static mut ControllerState {
    // SAFETY: single-threaded main thread; `vxnc_new` is called once before any
    // other opcode (the JS glue guarantees this), and no other thread touches
    // the pointer.
    unsafe { (*(&raw mut STATE)).as_mut().expect("vxnc_new not called") }
}

// ===========================================================================
// C-ABI opcode surface
// ===========================================================================

/// Construct the controller. Mirrors the native `vxn-clap` setup path; idempotent
/// after the first call would leak, so JS calls it exactly once per page.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_new() {
    let boxed = ControllerState::new();
    unsafe {
        *(&raw mut STATE) = Box::into_raw(boxed);
    }
}

/// Tear down the controller (page teardown / re-init). Reclaims the leaked Box.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_destroy() {
    unsafe {
        let p = *(&raw mut STATE);
        if !p.is_null() {
            drop(Box::from_raw(p));
            *(&raw mut STATE) = core::ptr::null_mut();
        }
    }
}

// ---- param-addressing counts (read from vxn-app, never hard-coded) ----------

/// `PATCH_COUNT` (per-layer patch params). JS reads this rather than baking 69.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_patch_count() -> u32 {
    vxn_app::PATCH_COUNT as u32
}
/// `GLOBAL_COUNT` (global params). JS reads this rather than baking 27.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_global_count() -> u32 {
    vxn_app::GLOBAL_COUNT as u32
}
/// `TOTAL_PARAMS` (= 2*PATCH_COUNT + GLOBAL_COUNT). JS reads this rather than 165.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_total_params() -> u32 {
    TOTAL_PARAMS as u32
}

// ---- UiEvent hot path (1:1 with UiEvent variants) ---------------------------

/// `UiEvent::SetParamNorm` — set a param from a normalised fader position.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_set_param_norm(clap_id: u32, norm: f32) {
    state().post(UiEvent::SetParamNorm {
        id: ParamId::new(clap_id as usize),
        norm,
    });
}

/// `UiEvent::SetParam` — set a param from a plain value.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_set_param(clap_id: u32, plain: f32) {
    state().post(UiEvent::SetParam {
        id: ParamId::new(clap_id as usize),
        plain,
    });
}

/// `UiEvent::BeginGesture` — open a gesture bracket on `clap_id`.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_begin_gesture(clap_id: u32) {
    state().post(UiEvent::BeginGesture {
        id: ParamId::new(clap_id as usize),
    });
}

/// `UiEvent::EndGesture` — close a gesture bracket on `clap_id`.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_end_gesture(clap_id: u32) {
    state().post(UiEvent::EndGesture {
        id: ParamId::new(clap_id as usize),
    });
}

/// `UiEvent::EditorReady` — re-broadcast every param + non-param view state so a
/// freshly-attached faceplate is seeded.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_editor_ready() {
    state().post(UiEvent::EditorReady);
}

// ---- per-synth custom (downcast stays inside wasm) --------------------------

/// `Vxn1UiCustom::SetKeyMode` — 0 Whole, 1 Dual, 2 Split (out-of-band, not a param).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_set_key_mode(mode: u32) {
    let mode = match mode {
        1 => KeyMode::Dual,
        2 => KeyMode::Split,
        _ => KeyMode::Whole,
    };
    state().post(Vxn1UiCustom::SetKeyMode { mode }.into_event());
}

/// `Vxn1UiCustom::SetSplitPoint` — MIDI note (out-of-band, not a param).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_set_split_point(note: u32) {
    state().post(
        Vxn1UiCustom::SetSplitPoint {
            note: (note & 0xff) as u8,
        }
        .into_event(),
    );
}

/// `Vxn1UiCustom::SetEditLayer` — 0 Upper, 1 Lower (pure view state).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_set_edit_layer(layer: u32) {
    let layer = if layer == 0 { Layer::Upper } else { Layer::Lower };
    state().post(Vxn1UiCustom::SetEditLayer { layer }.into_event());
}

/// `Vxn1UiCustom::ResetLayer` — reset every per-patch param of a layer to default.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_reset_layer(layer: u32) {
    let layer = if layer == 0 { Layer::Upper } else { Layer::Lower };
    state().post(Vxn1UiCustom::ResetLayer { layer }.into_event());
}

// ---- tick + ViewEvent drain -------------------------------------------------

/// Drain inbound queues into the model and pack the resulting ViewEvents into
/// the out-buffer. After this call, [`vxnc_view_out_ptr`]/[`vxnc_view_out_len`]
/// address the packed records, and [`vxnc_param_values_ptr`] the refreshed
/// current-value snapshot. Returns the byte length of the view out-buffer.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_tick() -> u32 {
    let st = state();
    st.tick();
    st.view_out.len() as u32
}

/// Pointer to the packed ViewEvent out-buffer in linear memory (see the module
/// docs for the record layout). Valid until the next [`vxnc_tick`].
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_view_out_ptr() -> *const u8 {
    state().view_out.as_ptr()
}

/// Byte length of the packed ViewEvent out-buffer.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_view_out_len() -> u32 {
    state().view_out.len() as u32
}

// ---- param SAB mirroring ----------------------------------------------------

/// Pointer to the f32[TOTAL_PARAMS] current-value snapshot (refreshed each
/// [`vxnc_tick`]). JS copies changed slots into the 0039 store SAB the worklet
/// reads lock-free.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_param_values_ptr() -> *const f32 {
    state().values_out.as_ptr()
}

/// Pointer to the f32[TOTAL_PARAMS] readback staging buffer. JS copies the
/// readback SAB region (values the worklet actually applied) here, then calls
/// [`vxnc_pump_readback`].
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_readback_in_ptr() -> *mut f32 {
    state().readback_in.as_mut_ptr()
}

/// Run the diff pump over `readback_in` (port of `push_param_diffs`): route any
/// drift through the controller as `HostEvent::ParamAutomation`, which emits a
/// gesture-gated `ParamChanged`. Call [`vxnc_tick`] afterwards to pack the
/// emitted ViewEvents.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_pump_readback() {
    state().pump_readback();
}
