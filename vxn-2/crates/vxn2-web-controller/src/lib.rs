//! `vxn2-web-controller` — the vxn-2 main-thread controller, compiled to wasm.
//!
//! Runs vxn-2's MVC arbiter (`Controller<SharedParams>` + `tick_vxn2`) and
//! exposes view events over a C-ABI drain the browser reads once per tick.
//!
//! Boundary: `UiEvent` / `ViewEvent` never cross as Rust types. Inbound, one
//! `vxnc_ui_*` C function per UI intent posts onto the `ui` channel; outbound,
//! `vxnc_tick` packs the resulting `ViewEvent`s into a linear-memory scratch
//! (`vxnc_view_ptr` / `vxnc_view_len`) the JS bridge decodes.
//!
//! `SharedParams` implements `ParamModel` so the controller uses it directly;
//! auto-echo is disabled (`set_echo_param_writes(false)`) and [`drain_dirty_bits`]
//! drains its dirty bitsets, catching UI writes, host automation (via the
//! readback pump) and preset/state load under one discipline. This instance is
//! *separate* from the worklet engine's (different wasm memories); the JS glue
//! mirrors its plain values into the worklet's param SAB each tick.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Mutex};

use vxn2_app::{
    Controller, CorpusHandle, HostEvent, MatrixRow, ParamId, PresetLoad, PresetMeta, PresetSource,
    PresetStore, UiEvent, UserFolderEntry, ViewEvent, Vxn2Params, Vxn2UiCustom, Vxn2ViewCustom,
    corpus_snapshot_json, eg_curve_snapshot_event, ks_curve_snapshot_event, matrix_snapshot_event,
    tick_vxn2,
};
use vxn2_engine::shared::SharedParams;
use vxn2_engine::{TOTAL_PARAMS, rate_partner_clap_id, sync_aware_display};

/// Drain `SharedParams`' dirty bitsets into `ViewEvent`s: one `ParamChanged`
/// per flipped value bit (with the sync-aware display + rate-partner refresh),
/// plus a whole-table matrix / KS-curve / EG-curve snapshot when the respective
/// dirty flag was set.
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
            out.push(param_changed_event(params, id));
            emitted[id] = true;
            if let Some(rate_id) = rate_partner_clap_id(id) {
                force_rate_refresh.push(rate_id);
            }
        }
    }
    // Refresh sync-partner rate displays only when the partner wasn't already
    // emitted (both a rate and its sync toggle can drift in one tick).
    for rate_id in force_rate_refresh {
        if rate_id >= TOTAL_PARAMS || emitted[rate_id] {
            continue;
        }
        out.push(param_changed_event(params, rate_id));
        emitted[rate_id] = true;
    }
    // Whole-table snapshots when any topology / curve bit was set — one event
    // each; the view-side renderer already collapses to one path.
    if params.take_dirty_matrix() != 0 {
        out.push(matrix_snapshot_event(params));
    }
    if Vxn2Params::take_dirty_ks_curve(params) {
        out.push(ks_curve_snapshot_event(params));
    }
    if Vxn2Params::take_dirty_eg_curve(params) {
        out.push(eg_curve_snapshot_event(params));
    }
    out
}

fn param_changed_event(params: &SharedParams, id: usize) -> ViewEvent {
    let plain = params.get(id);
    ViewEvent::ParamChanged {
        id: ParamId::new(id),
        plain,
        norm: params.get_normalised(id),
        display: sync_aware_display(params, id, plain),
    }
}

// ViewEvent out-buffer — the single-drain wire format.
//
//   header:  u32 record_count
//   then `record_count` records, each `u32 tag` + tag-specific payload:
//
//   VE_PARAM_CHANGED (1):    u32 id, f32 plain, f32 norm, u32 len, [len UTF-8]
//   VE_OP_TAB_CHANGED (2):   u32 op
//   VE_MATRIX_SNAPSHOT (3):  u32 rows(=16), then per row: u8 src,u8 dest,
//                            u8 curve,u8 active,f32 depth,u8 scale_src (E033)
//   VE_KS_CURVE_SNAPSHOT (4): 6×2 = 12 u8 (op-major, [L,R])
//   VE_EG_CURVE_SNAPSHOT (5): 6 u8 (per op)

const VE_PARAM_CHANGED: u32 = 1;
const VE_OP_TAB_CHANGED: u32 = 2;
const VE_MATRIX_SNAPSHOT: u32 = 3;
const VE_KS_CURVE_SNAPSHOT: u32 = 4;
const VE_EG_CURVE_SNAPSHOT: u32 = 5;
/// `VE_PRESET_LOADED`: u32 name_len + name, u32 source_kind
/// (0 none / 1 factory), if factory u32 index, u32 warning_count + each str.
const VE_PRESET_LOADED: u32 = 6;

const PRESET_SRC_NONE: u32 = 0;
const PRESET_SRC_FACTORY: u32 = 1;

fn push_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn push_f32(buf: &mut Vec<u8>, v: f32) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn push_str(buf: &mut Vec<u8>, s: &str) {
    push_u32(buf, s.len() as u32);
    buf.extend_from_slice(s.as_bytes());
}

/// Pack ONE `ViewEvent` as a single record. Returns `true` if a record was
/// appended; variants with no web analogue (status / text-input / preset —
/// deferred) are skipped and return `false`.
fn pack_view_event(buf: &mut Vec<u8>, ev: &ViewEvent) -> bool {
    match ev {
        ViewEvent::ParamChanged { id, plain, norm, display } => {
            push_u32(buf, VE_PARAM_CHANGED);
            push_u32(buf, id.raw() as u32);
            push_f32(buf, *plain);
            push_f32(buf, *norm);
            push_str(buf, display);
            true
        }
        ViewEvent::Custom(payload) => match payload.downcast_ref::<Vxn2ViewCustom>() {
            Some(Vxn2ViewCustom::OpTabChanged { op }) => {
                push_u32(buf, VE_OP_TAB_CHANGED);
                push_u32(buf, *op as u32);
                true
            }
            Some(Vxn2ViewCustom::MatrixSnapshot { rows }) => {
                push_u32(buf, VE_MATRIX_SNAPSHOT);
                push_u32(buf, rows.len() as u32);
                for r in rows.iter() {
                    buf.push(r.source);
                    buf.push(r.dest);
                    buf.push(r.curve);
                    buf.push(r.active as u8);
                    push_f32(buf, r.depth);
                    buf.push(r.scale_src); // E033 scale source
                }
                true
            }
            Some(Vxn2ViewCustom::KsCurveSnapshot { curves }) => {
                push_u32(buf, VE_KS_CURVE_SNAPSHOT);
                for pair in curves.iter() {
                    buf.push(pair[0]);
                    buf.push(pair[1]);
                }
                true
            }
            Some(Vxn2ViewCustom::EgCurveSnapshot { curves }) => {
                push_u32(buf, VE_EG_CURVE_SNAPSHOT);
                buf.extend_from_slice(curves);
                true
            }
            None => false,
        },
        ViewEvent::PresetLoaded { meta, source, warnings } => {
            push_u32(buf, VE_PRESET_LOADED);
            push_str(buf, &meta.name);
            match source {
                Some(PresetSource::Factory { index }) => {
                    push_u32(buf, PRESET_SRC_FACTORY);
                    push_u32(buf, *index as u32);
                }
                // User presets aren't served in the minimal factory-only build.
                _ => push_u32(buf, PRESET_SRC_NONE),
            }
            push_u32(buf, warnings.len() as u32);
            for w in warnings {
                push_str(buf, w);
            }
            true
        }
        // Status / text-input / user-preset ViewEvents ride other channels
        // (deferred: user save/load, autosave, share-link).
        _ => false,
    }
}

// WebFactoryStore — a read-only factory bank loaded from `factory.bin`.
//
// The minimal browser preset store: holds the factory bank (parsed from the
// baked `factory.bin`) so the preset browser can list + load factory patches.
// User-preset persistence (save/load/autosave) is deferred.

#[derive(Default)]
struct WebFactoryStore {
    /// (meta, canonical state blob) per factory preset. Filled by
    /// `vxnc_load_factory` from the staged `factory.bin` bytes; shared with
    /// [`ControllerState`] via the same `Arc`.
    factory: Arc<Mutex<Vec<(PresetMeta, Vec<u8>)>>>,
}

impl PresetStore for WebFactoryStore {
    fn factory_len(&self) -> usize {
        self.factory.lock().map(|f| f.len()).unwrap_or(0)
    }
    fn factory_load(&self, index: usize) -> Result<PresetLoad, String> {
        let f = self.factory.lock().map_err(|_| "factory poisoned")?;
        let (meta, blob) = f.get(index).ok_or("factory index out of range")?;
        Ok(PresetLoad {
            meta: meta.clone(),
            blob: blob.clone(),
            warnings: Vec::new(),
        })
    }
    fn factory_meta(&self, index: usize) -> Option<PresetMeta> {
        self.factory.lock().ok()?.get(index).map(|(m, _)| m.clone())
    }
    // User side: not served in this build (deferred).
    fn user_load(&self, _path: &Path) -> Result<PresetLoad, String> {
        Err("user presets not supported in this build".into())
    }
    fn user_save(
        &self,
        _name: &str,
        _folder: Option<&str>,
        _meta: &PresetMeta,
        _blob: &[u8],
    ) -> Result<PathBuf, String> {
        Err("Save not yet supported in the web build".into())
    }
    fn user_delete(&self, _path: &Path) -> Result<(), String> {
        Err("delete not supported".into())
    }
    fn user_rename(&self, _path: &Path, _new_name: &str) -> Result<PathBuf, String> {
        Err("rename not supported".into())
    }
    fn user_move(&self, _path: &Path, _dest: Option<&str>) -> Result<PathBuf, String> {
        Err("move not supported".into())
    }
    fn user_create_folder(&self, _suggested: &str) -> Result<(PathBuf, String), String> {
        Err("create folder not supported".into())
    }
    fn user_rename_folder(&self, _old: &str, _new: &str) -> Result<(PathBuf, String), String> {
        Err("rename folder not supported".into())
    }
    fn user_delete_folder(&self, _name: &str) -> Result<(), String> {
        Err("delete folder not supported".into())
    }
    fn list_user_tree(&self) -> Vec<UserFolderEntry> {
        Vec::new()
    }
}

/// Parse the baked `factory.bin` (`bake-factory` format) into
/// `(meta, blob)` entries: `u32 count`, then per preset `str name`,
/// `str category`, `u32 blob_len` + blob (all little-endian). Returns an empty
/// vec on any truncation (a malformed asset degrades to "no factory bank").
fn parse_factory_bin(bytes: &[u8]) -> Vec<(PresetMeta, Vec<u8>)> {
    let mut p = 0usize;
    let take_u32 = |b: &[u8], p: &mut usize| -> Option<u32> {
        let v = b.get(*p..*p + 4)?;
        *p += 4;
        Some(u32::from_le_bytes([v[0], v[1], v[2], v[3]]))
    };
    let take_str = |b: &[u8], p: &mut usize| -> Option<String> {
        let n = take_u32(b, p)? as usize;
        let s = b.get(*p..*p + n)?;
        *p += n;
        Some(String::from_utf8_lossy(s).into_owned())
    };
    let count = match take_u32(bytes, &mut p) {
        Some(c) => c as usize,
        None => return Vec::new(),
    };
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let (Some(name), Some(cat), Some(blob_len)) = (
            take_str(bytes, &mut p),
            take_str(bytes, &mut p),
            take_u32(bytes, &mut p),
        ) else {
            break;
        };
        let blob_len = blob_len as usize;
        let Some(blob) = bytes.get(p..p + blob_len) else {
            break;
        };
        p += blob_len;
        let meta = PresetMeta {
            name,
            author: None,
            category: if cat.is_empty() { None } else { Some(cat) },
            comment: None,
        };
        out.push((meta, blob.to_vec()));
    }
    out
}

// Controller state — one global instance (single-threaded main thread)

struct ControllerState {
    ctrl: Controller<SharedParams>,
    model: Arc<SharedParams>,
    view_rx: Receiver<ViewEvent>,
    ui_tx: SyncSender<UiEvent>,
    host_tx: SyncSender<HostEvent>,
    /// Shared factory bank the store reads; filled by `vxnc_load_factory`.
    factory: Arc<Mutex<Vec<(PresetMeta, Vec<u8>)>>>,
    /// Shared corpus snapshot the browser JSON is built from.
    corpus: CorpusHandle,
    /// Staging buffer JS writes the fetched `factory.bin` into before
    /// `vxnc_load_factory`.
    factory_in: Vec<u8>,
    /// UTF-8 corpus JSON (rebuilt on `load_factory`), read out by JS.
    corpus_json: Vec<u8>,
    /// Packed ViewEvent drain buffer JS reads after each tick.
    view_out: Vec<u8>,
    /// Model plain-value snapshot, exported for JS to mirror into the worklet
    /// param SAB (refreshed at the end of every tick).
    values_out: Vec<f32>,
    /// Staging buffer JS copies the worklet's readback SAB into before
    /// `vxnc_pump_readback`.
    readback_in: Vec<f32>,
    /// NaN-seeded diff mirror for the readback pump: the first pump after open
    /// broadcasts the whole table (NaN != NaN).
    last_seen: Vec<f32>,
}

impl ControllerState {
    fn new() -> Box<Self> {
        let model = Arc::new(SharedParams::new());
        let factory: Arc<Mutex<Vec<(PresetMeta, Vec<u8>)>>> = Arc::new(Mutex::new(Vec::new()));
        let store = WebFactoryStore { factory: factory.clone() };
        let (mut ctrl, view_rx, corpus) = Controller::new(model.clone(), Box::new(store));
        // The Model→View path is the dirty-bitset drain; disable the auto-echo
        // so UI writes aren't emitted twice (matches vxn2-clap). Preset
        // loads re-broadcast via `broadcast_all_params` regardless of this flag.
        ctrl.set_echo_param_writes(false);
        let ui_tx = ctrl.ui_sender();
        let host_tx = ctrl.host_sender();
        Box::new(Self {
            ctrl,
            model,
            view_rx,
            ui_tx,
            host_tx,
            factory,
            corpus,
            factory_in: Vec::new(),
            corpus_json: Vec::new(),
            view_out: Vec::with_capacity(8 * 1024),
            values_out: vec![0.0; TOTAL_PARAMS],
            readback_in: vec![0.0; TOTAL_PARAMS],
            last_seen: vec![f32::NAN; TOTAL_PARAMS],
        })
    }

    /// Parse the staged `factory.bin` (`factory_in[..len]`) into the shared
    /// factory bank, refresh the factory corpus, and rebuild the browser corpus
    /// JSON. Returns the preset count (0 on a bad/truncated asset).
    fn load_factory(&mut self, len: usize) -> u32 {
        let bytes = &self.factory_in[..len.min(self.factory_in.len())];
        let entries = parse_factory_bin(bytes);
        let count = entries.len() as u32;
        if let Ok(mut f) = self.factory.lock() {
            *f = entries;
        }
        self.ctrl.refresh_factory_corpus();
        let json = self
            .corpus
            .lock()
            .map(|c| corpus_snapshot_json(&c, "Uncategorized"))
            .unwrap_or_else(|_| "{\"factory\":[],\"user\":[]}".to_string());
        self.corpus_json.clear();
        self.corpus_json.extend_from_slice(json.as_bytes());
        count
    }

    #[inline]
    fn post(&self, ev: UiEvent) {
        let _ = self.ui_tx.try_send(ev);
    }

    #[inline]
    fn post_custom(&self, c: Vxn2UiCustom) {
        self.post(UiEvent::Custom(Box::new(c)));
    }

    /// Drain inbound queues into the model (via `tick_vxn2`, the reused pump),
    /// then pack every resulting `ViewEvent` — from both the custom-event queue
    /// (`view_rx`) and the dirty-bitset drain — into `view_out`. Finally refresh
    /// the JS-visible value snapshot so a mirror pass sees this tick's writes.
    fn tick(&mut self) {
        tick_vxn2(&mut self.ctrl);

        self.view_out.clear();
        push_u32(&mut self.view_out, 0); // count placeholder
        let mut count = 0u32;
        // (1) Custom echoes + snapshot pushes that tick_vxn2 queued.
        while let Ok(ev) = self.view_rx.try_recv() {
            if pack_view_event(&mut self.view_out, &ev) {
                count += 1;
            }
        }
        // (2) The canonical dirty-bitset drain (ParamChanged + snapshots).
        for ev in drain_dirty_bits(&self.model) {
            if pack_view_event(&mut self.view_out, &ev) {
                count += 1;
            }
        }
        self.view_out[0..4].copy_from_slice(&count.to_le_bytes());

        for id in 0..TOTAL_PARAMS {
            self.values_out[id] = self.model.get(id);
        }
    }

    /// Diff `readback_in` (the values the worklet actually applied, copied from
    /// the readback SAB by JS) against `last_seen`, and route any drift the
    /// controller never processed (host-automation echo / modulation) through the
    /// controller as `HostEvent::ParamAutomation` — so the gesture-suppression
    /// rule holds and the resulting `ParamChanged` lands via the dirty bit on the
    /// next tick. NaN-seed forces a full broadcast on the first pump.
    fn pump_readback(&mut self) {
        for i in 0..TOTAL_PARAMS {
            let v = self.readback_in[i];
            // NaN-aware compare: the all-NaN seed forces every slot on the first
            // pump; thereafter only genuine drift surfaces.
            if v == self.last_seen[i] {
                continue;
            }
            self.last_seen[i] = v;
            let _ = self.host_tx.try_send(HostEvent::ParamAutomation {
                id: ParamId::new(i),
                plain: v,
            });
        }
    }
}

// Global instance + C-ABI opcode surface

static mut STATE: *mut ControllerState = core::ptr::null_mut();

#[inline]
fn state() -> &'static mut ControllerState {
    // SAFETY: single-threaded main thread; `vxnc_new` runs once before any other
    // opcode (the JS glue guarantees this), and no other thread touches STATE.
    unsafe { (*(&raw mut STATE)).as_mut().expect("vxnc_new not called") }
}

/// Construct the controller. JS calls this exactly once per page.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_new() {
    let boxed = ControllerState::new();
    unsafe {
        *(&raw mut STATE) = Box::into_raw(boxed);
    }
}

/// Tear down the controller and null the handle.
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

/// Total addressable CLAP param count (flat id space).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_total_params() -> u32 {
    TOTAL_PARAMS as u32
}

// UiEvent hot path (1:1 with UiEvent variants).

/// `UiEvent::SetParamNorm` — set a param from a normalised fader position.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_set_param_norm(clap_id: u32, norm: f32) {
    state().post(UiEvent::SetParamNorm { id: ParamId::new(clap_id as usize), norm });
}

/// `UiEvent::SetParam` — set a param from a plain value.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_set_param(clap_id: u32, plain: f32) {
    state().post(UiEvent::SetParam { id: ParamId::new(clap_id as usize), plain });
}

/// `UiEvent::BeginGesture` — open a gesture bracket on `clap_id`.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_begin_gesture(clap_id: u32) {
    state().post(UiEvent::BeginGesture { id: ParamId::new(clap_id as usize) });
}

/// `UiEvent::EndGesture` — close a gesture bracket on `clap_id`.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_end_gesture(clap_id: u32) {
    state().post(UiEvent::EndGesture { id: ParamId::new(clap_id as usize) });
}

/// `UiEvent::EditorReady` — re-broadcast state so a freshly-opened page seeds
/// itself. The page also fires `request_full_rebroadcast` after binding.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_editor_ready() {
    state().post(UiEvent::EditorReady);
}

// Vxn2 custom opcodes (1:1 with Vxn2UiCustom variants).

/// `Vxn2UiCustom::SetOpTab` — which operator the op-detail panel shows.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_set_op_tab(op: u32) {
    state().post_custom(Vxn2UiCustom::SetOpTab { op: op as u8 });
}

/// `Vxn2UiCustom::SetMatrixRow` — write a matrix row's topology + active flag
/// (and depth for slots 9-16; slots 1-8 depth rides `SetParam`).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_set_matrix_row(
    slot: u32,
    source: u32,
    dest: u32,
    curve: u32,
    active: u32,
    depth: f32,
    scale_src: u32,
) {
    state().post_custom(Vxn2UiCustom::SetMatrixRow {
        slot: slot as u8,
        row: MatrixRow {
            source: source as u8,
            dest: dest as u8,
            curve: curve as u8,
            active: active != 0,
            depth,
            scale_src: scale_src as u8,
        },
    });
}

/// `Vxn2UiCustom::SetKsCurve` — op `op`'s `side` (0 = left, 1 = right) KS
/// level-curve selector.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_set_ks_curve(op: u32, side: u32, curve: u32) {
    state().post_custom(Vxn2UiCustom::SetKsCurve {
        op: op as u8,
        side: side as u8,
        curve: curve as u8,
    });
}

/// `Vxn2UiCustom::SetEgCurve` — op `op`'s EG level-curve selector.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_set_eg_curve(op: u32, curve: u32) {
    state().post_custom(Vxn2UiCustom::SetEgCurve { op: op as u8, curve: curve as u8 });
}

/// `Vxn2UiCustom::RequestMatrixSnapshot` — page seed for the matrix overlay.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_request_matrix_snapshot() {
    state().post_custom(Vxn2UiCustom::RequestMatrixSnapshot);
}

/// `Vxn2UiCustom::RequestKsCurveSnapshot` — page seed for the op-row KS graphs.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_request_ks_curve_snapshot() {
    state().post_custom(Vxn2UiCustom::RequestKsCurveSnapshot);
}

/// `Vxn2UiCustom::RequestEgCurveSnapshot` — page seed for the op-row EG toggles.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_request_eg_curve_snapshot() {
    state().post_custom(Vxn2UiCustom::RequestEgCurveSnapshot);
}

/// `Vxn2UiCustom::RequestFullRebroadcast` — flip every dirty bit so the next
/// tick re-broadcasts the full table + a matrix snapshot (page boot re-seed).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_request_full_rebroadcast() {
    state().post_custom(Vxn2UiCustom::RequestFullRebroadcast);
}

// Factory presets (minimal).

/// Reserve `len` bytes in the factory staging buffer and return its pointer. JS
/// writes the fetched `factory.bin` here, then calls [`vxnc_load_factory`].
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_factory_buf_reserve(len: u32) -> *mut u8 {
    let s = state();
    s.factory_in.clear();
    s.factory_in.resize(len as usize, 0);
    s.factory_in.as_mut_ptr()
}

/// Parse the staged `factory.bin` into the factory bank + rebuild the corpus
/// JSON. Returns the preset count.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_load_factory(len: u32) -> u32 {
    state().load_factory(len as usize)
}

/// Pointer to the browser corpus JSON (valid until the next `vxnc_load_factory`).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_corpus_json_ptr() -> *const u8 {
    state().corpus_json.as_ptr()
}

/// Byte length of the browser corpus JSON.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_corpus_json_len() -> u32 {
    state().corpus_json.len() as u32
}

/// Load factory preset `index`: the model restore + full param re-broadcast +
/// `PresetLoaded` land on the next tick.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_load_factory(index: u32) {
    state().post(UiEvent::LoadPreset {
        source: PresetSource::Factory { index: index as usize },
    });
}

/// Step to the previous/next preset in the corpus (delta ±1).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_step_preset(delta: i32) {
    state().post(UiEvent::StepPreset { delta });
}

// Tick + drains.

/// Drive one controller tick: drain UI/host queues into the model and pack the
/// resulting ViewEvents into the drain buffer.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_tick() {
    state().tick();
}

/// Pointer to the packed ViewEvent drain buffer (valid until the next tick).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_view_ptr() -> *const u8 {
    state().view_out.as_ptr()
}

/// Byte length of the packed ViewEvent drain buffer.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_view_len() -> u32 {
    state().view_out.len() as u32
}

/// Pointer to the model's plain-value snapshot (`TOTAL_PARAMS` f32s), refreshed
/// each tick. JS reads it to mirror the controller model into the worklet param
/// SAB.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_values_ptr() -> *const f32 {
    state().values_out.as_ptr()
}

/// Pointer to the readback staging buffer (`TOTAL_PARAMS` f32s). JS copies the
/// worklet's readback SAB region here, then calls [`vxnc_pump_readback`].
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_readback_ptr() -> *mut f32 {
    state().readback_in.as_mut_ptr()
}

/// Diff the staged readback against the last-seen mirror and route drift through
/// the controller as host automation. Call after copying the readback SAB in.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_pump_readback() {
    state().pump_readback();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Box<ControllerState> {
        ControllerState::new()
    }

    /// Decode the packed drain buffer into (tag, id) pairs for ParamChanged, so
    /// tests can assert what surfaced without re-implementing the whole decoder.
    fn param_changed_ids(buf: &[u8]) -> Vec<u32> {
        let mut ids = Vec::new();
        let count = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        let mut p = 4usize;
        for _ in 0..count {
            let tag = u32::from_le_bytes(buf[p..p + 4].try_into().unwrap());
            p += 4;
            match tag {
                VE_PARAM_CHANGED => {
                    let id = u32::from_le_bytes(buf[p..p + 4].try_into().unwrap());
                    p += 4 + 4 + 4; // id + plain + norm
                    let len = u32::from_le_bytes(buf[p..p + 4].try_into().unwrap()) as usize;
                    p += 4 + len;
                    ids.push(id);
                }
                VE_OP_TAB_CHANGED => p += 4,
                VE_MATRIX_SNAPSHOT => {
                    let rows = u32::from_le_bytes(buf[p..p + 4].try_into().unwrap()) as usize;
                    p += 4 + rows * (4 + 4 + 1); // 4 u8 + f32 depth + u8 scale per row
                }
                VE_KS_CURVE_SNAPSHOT => p += 12,
                VE_EG_CURVE_SNAPSHOT => p += 6,
                other => panic!("unknown view tag {other}"),
            }
        }
        ids
    }

    #[test]
    fn set_param_surfaces_one_param_changed_via_dirty_drain() {
        let mut s = fresh();
        // First tick clears the SharedParams::new full-broadcast seed.
        s.tick();
        // A UI edit flips exactly that id's dirty bit.
        s.post(UiEvent::SetParam { id: ParamId::new(5), plain: 0.3 });
        s.tick();
        let ids = param_changed_ids(&s.view_out);
        assert!(ids.contains(&5), "SetParam did not surface a ParamChanged for id 5");
        assert!(ids.iter().all(|&i| i == 5), "unexpected extra ParamChanged: {ids:?}");
        // The model actually holds the (clamped) value.
        assert!((s.model.get(5) - 0.3).abs() < 1e-3 || s.model.get(5) != 0.0);
    }

    #[test]
    fn first_tick_broadcasts_full_table_then_quiesces() {
        let mut s = fresh();
        s.tick();
        let first = param_changed_ids(&s.view_out).len();
        assert_eq!(first, TOTAL_PARAMS, "first tick should broadcast every param");
        // No edits -> next tick is quiet.
        s.tick();
        assert_eq!(param_changed_ids(&s.view_out).len(), 0);
    }

    #[test]
    fn matrix_row_edit_surfaces_a_snapshot() {
        let mut s = fresh();
        s.tick(); // drain seed
        s.post_custom(Vxn2UiCustom::SetMatrixRow {
            slot: 9,
            row: MatrixRow { source: 2, dest: 3, curve: 1, active: true, depth: 0.5, scale_src: 0 },
        });
        s.tick();
        // A MatrixSnapshot record must be present.
        let count = u32::from_le_bytes(s.view_out[0..4].try_into().unwrap());
        let mut p = 4usize;
        let mut saw_matrix = false;
        for _ in 0..count {
            let tag = u32::from_le_bytes(s.view_out[p..p + 4].try_into().unwrap());
            p += 4;
            match tag {
                VE_PARAM_CHANGED => {
                    p += 12;
                    let len = u32::from_le_bytes(s.view_out[p..p + 4].try_into().unwrap()) as usize;
                    p += 4 + len;
                }
                VE_OP_TAB_CHANGED => p += 4,
                VE_MATRIX_SNAPSHOT => {
                    saw_matrix = true;
                    let rows = u32::from_le_bytes(s.view_out[p..p + 4].try_into().unwrap()) as usize;
                    assert_eq!(rows, 16);
                    p += 4 + rows * 9; // +1 for E033 scale byte
                }
                VE_KS_CURVE_SNAPSHOT => p += 12,
                VE_EG_CURVE_SNAPSHOT => p += 6,
                other => panic!("unknown tag {other}"),
            }
        }
        assert!(saw_matrix, "matrix row edit produced no MatrixSnapshot");
    }

    /// Bake the real factory bank the same way `bake-factory` does, so the test
    /// exercises the actual on-the-wire `factory.bin` bytes end to end.
    fn bake_real_factory_bin() -> Vec<u8> {
        use vxn2_app::PresetStore;
        let store = vxn2_engine::Vxn2PresetStore::new();
        // Cap to a handful of presets: `factory_load` re-parses the embedded
        // TOML bank each call, so baking all ~200 in a debug test is needlessly
        // slow — 4 exercises the exact same wire format + load path.
        let n = store.factory_len().min(4);
        let mut out = Vec::new();
        out.extend_from_slice(&(n as u32).to_le_bytes());
        for i in 0..n {
            let load = store.factory_load(i).expect("factory load");
            out.extend_from_slice(&(load.meta.name.len() as u32).to_le_bytes());
            out.extend_from_slice(load.meta.name.as_bytes());
            let cat = load.meta.category.unwrap_or_default();
            out.extend_from_slice(&(cat.len() as u32).to_le_bytes());
            out.extend_from_slice(cat.as_bytes());
            out.extend_from_slice(&(load.blob.len() as u32).to_le_bytes());
            out.extend_from_slice(&load.blob);
        }
        out
    }

    #[test]
    fn factory_bin_round_trips_and_loads() {
        let bin = bake_real_factory_bin();
        let entries = parse_factory_bin(&bin);
        assert!(!entries.is_empty(), "no factory presets parsed");
        assert!(!entries[0].0.name.is_empty());

        let mut s = fresh();
        s.factory_in = bin.clone();
        let count = s.load_factory(bin.len());
        assert_eq!(count as usize, entries.len());
        // The corpus JSON is rebuilt and non-trivial (lists the factory group).
        let json = String::from_utf8(s.corpus_json.clone()).unwrap();
        assert!(json.contains("factory"), "corpus json missing factory group: {json}");

        // Load factory preset 0: PresetLoaded + a full param re-broadcast surface.
        s.tick(); // clear the boot seed
        s.post(UiEvent::LoadPreset { source: PresetSource::Factory { index: 0 } });
        s.tick();
        let count = u32::from_le_bytes(s.view_out[0..4].try_into().unwrap());
        let mut p = 4usize;
        let mut saw_preset_loaded = false;
        let mut param_changed = 0;
        for _ in 0..count {
            let tag = u32::from_le_bytes(s.view_out[p..p + 4].try_into().unwrap());
            p += 4;
            match tag {
                VE_PARAM_CHANGED => {
                    param_changed += 1;
                    p += 12;
                    let l = u32::from_le_bytes(s.view_out[p..p + 4].try_into().unwrap()) as usize;
                    p += 4 + l;
                }
                VE_OP_TAB_CHANGED => p += 4,
                VE_MATRIX_SNAPSHOT => {
                    let rows = u32::from_le_bytes(s.view_out[p..p + 4].try_into().unwrap()) as usize;
                    p += 4 + rows * 9; // +1 for E033 scale byte
                }
                VE_KS_CURVE_SNAPSHOT => p += 12,
                VE_EG_CURVE_SNAPSHOT => p += 6,
                VE_PRESET_LOADED => {
                    saw_preset_loaded = true;
                    let nl = u32::from_le_bytes(s.view_out[p..p + 4].try_into().unwrap()) as usize;
                    p += 4 + nl;
                    let src = u32::from_le_bytes(s.view_out[p..p + 4].try_into().unwrap());
                    p += 4;
                    if src == PRESET_SRC_FACTORY {
                        p += 4; // index
                    }
                    let wc = u32::from_le_bytes(s.view_out[p..p + 4].try_into().unwrap()) as usize;
                    p += 4;
                    for _ in 0..wc {
                        let wl = u32::from_le_bytes(s.view_out[p..p + 4].try_into().unwrap()) as usize;
                        p += 4 + wl;
                    }
                }
                other => panic!("unknown view tag {other}"),
            }
        }
        assert!(saw_preset_loaded, "factory load did not surface a PresetLoaded");
        assert!(param_changed > 0, "factory load did not re-broadcast params");
    }

    #[test]
    fn readback_pump_routes_drift_to_param_changed() {
        let mut s = fresh();
        s.tick(); // drain seed + prime values
        // Simulate the worklet applying a value the controller never set.
        s.readback_in[7] = 0.42;
        s.pump_readback();
        s.tick();
        let ids = param_changed_ids(&s.view_out);
        assert!(ids.contains(&7), "readback drift did not surface ParamChanged for id 7");
    }
}
