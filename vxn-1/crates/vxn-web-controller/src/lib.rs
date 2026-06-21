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
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};

use vxn_app::{
    Controller, CorpusHandle, FactoryEntry, KeyMode, Layer, ParamDesc, ParamId, ParamModel,
    PresetLoad, PresetMeta, PresetSource, PresetStore, TOTAL_PARAMS, UNCATEGORIZED, UiEvent,
    UserFolderEntry, ViewEvent, Vxn1Params, Vxn1UiCustom, corpus_snapshot_json, desc_for_clap_id,
    factory_asset, preset_record,
};

mod user_store;
use user_store::{UserState, UserWrite};

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

/// Web preset store. The factory bank is baked at build time and fetched as a
/// flat asset at boot (E019 / 0062), parsed into `factory` *after* the
/// controller is constructed — so the entries live behind a shared
/// `Arc<Mutex<…>>` the [`ControllerState`] also holds to fill them once the
/// asset arrives. (`Mutex` not `RefCell` only because `PresetStore: Send`;
/// the page is single-threaded.)
///
/// The user side is browser storage (IndexedDB; ADR 0009 addendum, E019 / 0063).
/// This holds the synchronous in-memory cache ([`UserState`]); the cache is
/// hydrated from IndexedDB at boot and its journal flushed back, both wired in
/// 0064. The cache is shared with [`ControllerState`] (same `Arc`) so 0064 can
/// drain the journal and seed hydrated records without re-plumbing the store.
#[derive(Default)]
struct WebPresetStore {
    factory: Arc<Mutex<Vec<FactoryEntry>>>,
    user: Arc<Mutex<UserState>>,
}

impl PresetStore for WebPresetStore {
    fn factory_len(&self) -> usize {
        self.factory.lock().map(|f| f.len()).unwrap_or(0)
    }
    fn factory_load(&self, index: usize) -> Result<PresetLoad, String> {
        let f = self.factory.lock().map_err(|_| "factory poisoned")?;
        let e = f.get(index).ok_or("factory index out of range")?;
        Ok(PresetLoad {
            meta: e.meta.clone(),
            blob: e.blob.clone(),
            warnings: Vec::new(),
        })
    }
    fn factory_meta(&self, index: usize) -> Option<PresetMeta> {
        self.factory.lock().ok()?.get(index).map(|e| e.meta.clone())
    }
    fn user_load(&self, path: &Path) -> Result<PresetLoad, String> {
        self.user.lock().map_err(|_| "user store poisoned")?.load(path)
    }
    fn user_save(
        &self,
        name: &str,
        folder: Option<&str>,
        meta: &PresetMeta,
        blob: &[u8],
    ) -> Result<PathBuf, String> {
        self.user
            .lock()
            .map_err(|_| "user store poisoned")?
            .save(name, folder, meta, blob)
    }
    fn user_delete(&self, path: &Path) -> Result<(), String> {
        self.user.lock().map_err(|_| "user store poisoned")?.delete(path)
    }
    fn user_rename(&self, path: &Path, new_name: &str) -> Result<PathBuf, String> {
        self.user
            .lock()
            .map_err(|_| "user store poisoned")?
            .rename(path, new_name)
    }
    fn user_move(&self, path: &Path, dest_folder: Option<&str>) -> Result<PathBuf, String> {
        self.user
            .lock()
            .map_err(|_| "user store poisoned")?
            .move_preset(path, dest_folder)
    }
    fn user_create_folder(&self, suggested: &str) -> Result<(PathBuf, String), String> {
        self.user
            .lock()
            .map_err(|_| "user store poisoned")?
            .create_folder(suggested)
    }
    fn user_rename_folder(&self, old: &str, new: &str) -> Result<(PathBuf, String), String> {
        self.user
            .lock()
            .map_err(|_| "user store poisoned")?
            .rename_folder(old, new)
    }
    fn user_delete_folder(&self, name: &str) -> Result<(), String> {
        self.user
            .lock()
            .map_err(|_| "user store poisoned")?
            .delete_folder(name)
    }
    fn list_user_tree(&self) -> Vec<UserFolderEntry> {
        self.user
            .lock()
            .map(|u| u.list_tree())
            .unwrap_or_default()
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
//   VE_PRESET_LOADED (5):       (E019 / 0062 — preset bar name + browser highlight)
//     u32 name_len ; [name_len bytes UTF-8]
//     u32 source_kind            0 = none, 1 = factory, 2 = user
//       if factory: u32 index
//       if user:    u32 path_len ; [path_len bytes UTF-8]
//     u32 warning_count ; per warning: u32 len ; [len bytes UTF-8]
//
// Remaining out-of-scope ViewEvents (status / text-input / corpus-changed) are
// skipped here; the corpus listing rides the separate `vxnc_corpus_json` channel
// (the web analogue of the native `applyPresetCorpus` push).

const VE_PARAM_CHANGED: u32 = 1;
const VE_KEY_MODE_CHANGED: u32 = 2;
const VE_SPLIT_POINT_CHANGED: u32 = 3;
const VE_EDIT_LAYER_CHANGED: u32 = 4;
const VE_PRESET_LOADED: u32 = 5;
// E019 / 0064: the user-preset corpus changed (save/rename/delete/move/folder
// op, or an EditorReady re-push). Carries an optional follow path the browser
// moves its cursor onto. The controller rebuilds `corpus_json` in the same
// drain, so JS re-reads it via `vxnc_corpus_json` and re-pushes `applyPresetCorpus`.
const VE_PRESET_CORPUS_CHANGED: u32 = 6;

const PRESET_SRC_NONE: u32 = 0;
const PRESET_SRC_FACTORY: u32 = 1;
const PRESET_SRC_USER: u32 = 2;

// Journal-op tags packed by `vxnc_take_journal` (the wasm UserWrite variants),
// decoded JS-side into `applyWrites` ops (preset-storage.mjs). MUST match the
// JS mirror in controller.mjs (JW_*).
const JW_PUT: u32 = 1;
const JW_DELETE: u32 = 2;
const JW_PUT_FOLDER: u32 = 3;
const JW_DELETE_FOLDER: u32 = 4;

// Sentinel `len` meaning `Option::None` for the string opcodes whose argument is
// `Option<String>` (save/move folder). A present-but-empty string is len 0.
const ARG_NONE: u32 = u32::MAX;

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
    /// Shared factory entries the store reads; filled by `vxnc_load_factory`
    /// once JS has fetched the baked asset (E019 / 0062).
    factory: Arc<Mutex<Vec<FactoryEntry>>>,
    /// Shared user-preset cache the store reads/writes (E019 / 0063). Held here
    /// so 0064 can hydrate it from IndexedDB and drain its write journal.
    user: Arc<Mutex<UserState>>,
    /// Shared corpus snapshot (factory + user listing) JS serializes for the
    /// preset browser via `vxnc_corpus_json`.
    corpus: CorpusHandle,
    /// Staging buffer JS writes the fetched factory asset into before
    /// `vxnc_load_factory`.
    factory_in: Vec<u8>,
    /// UTF-8 corpus JSON (rebuilt by `load_factory` / `hydrate_done` / a
    /// corpus-changing tick), read out by JS.
    corpus_json: Vec<u8>,
    /// Staging buffer JS writes UTF-8 opcode arguments (preset/folder names,
    /// paths) into before a string-taking user opcode reads them (E019 / 0064).
    /// Reused per call — opcodes copy what they need into the posted `UiEvent`.
    arg_in: Vec<u8>,
    /// Packed pending-write journal (UserWrite variants) JS drains via
    /// `vxnc_take_journal` and flushes to IndexedDB off the tick (E019 / 0064).
    journal_out: Vec<u8>,
    /// Full patch-state snapshot blob (the host-state-blob analogue) JS reads out
    /// for full-state autosave, and the staging buffer it writes a saved blob into
    /// before `vxnc_restore_state` (E019 / 0065). Reused per call.
    state_out: Vec<u8>,
}

impl ControllerState {
    fn new() -> Box<Self> {
        let model = Arc::new(WebModel::new());
        let factory: Arc<Mutex<Vec<FactoryEntry>>> = Arc::new(Mutex::new(Vec::new()));
        let user: Arc<Mutex<UserState>> = Arc::new(Mutex::new(UserState::default()));
        let store = WebPresetStore {
            factory: factory.clone(),
            user: user.clone(),
        };
        let (ctrl, view_rx, corpus) = Controller::new(model.clone(), Box::new(store));
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
            factory,
            user,
            corpus,
            factory_in: Vec::new(),
            corpus_json: Vec::new(),
            arg_in: Vec::new(),
            journal_out: Vec::with_capacity(4 * 1024),
            state_out: Vec::with_capacity(vxn_app::BLOB_LEN),
        })
    }

    /// Slice of the staged opcode-argument buffer (`arg_in[start..start+len]`),
    /// clamped to the buffer so a malformed length can't panic.
    fn arg_slice(&self, start: usize, len: usize) -> &[u8] {
        let n = self.arg_in.len();
        let s = start.min(n);
        let e = start.saturating_add(len).min(n);
        &self.arg_in[s..e]
    }

    /// `arg_slice` decoded as a UTF-8 `String` (lossy — the bytes came from a JS
    /// `TextEncoder`, so this is only defensive).
    fn arg_string(&self, start: usize, len: usize) -> String {
        String::from_utf8_lossy(self.arg_slice(start, len)).into_owned()
    }

    /// Drain the user store's pending-write journal into `journal_out` (the
    /// packed layout JS decodes into `applyWrites` ops). Returns the byte length.
    fn take_journal(&mut self) -> u32 {
        let ops = self
            .user
            .lock()
            .map(|mut u| u.take_journal())
            .unwrap_or_default();
        self.journal_out.clear();
        push_u32(&mut self.journal_out, ops.len() as u32);
        for op in &ops {
            match op {
                UserWrite::Put { key, bytes } => {
                    push_u32(&mut self.journal_out, JW_PUT);
                    push_str(&mut self.journal_out, key);
                    push_u32(&mut self.journal_out, bytes.len() as u32);
                    self.journal_out.extend_from_slice(bytes);
                }
                UserWrite::Delete { key } => {
                    push_u32(&mut self.journal_out, JW_DELETE);
                    push_str(&mut self.journal_out, key);
                }
                UserWrite::PutFolder { name } => {
                    push_u32(&mut self.journal_out, JW_PUT_FOLDER);
                    push_str(&mut self.journal_out, name);
                }
                UserWrite::DeleteFolder { name } => {
                    push_u32(&mut self.journal_out, JW_DELETE_FOLDER);
                    push_str(&mut self.journal_out, name);
                }
            }
        }
        self.journal_out.len() as u32
    }

    /// Parse the baked factory asset JS staged in `factory_in[..len]` into the
    /// shared store, republish the factory corpus, and rebuild `corpus_json`.
    /// Returns the entry count, or 0 on a bad/truncated asset.
    fn load_factory(&mut self, len: usize) -> u32 {
        let bytes = &self.factory_in[..len.min(self.factory_in.len())];
        let entries = match factory_asset::decode(bytes) {
            Ok(e) => e,
            Err(_) => return 0,
        };
        let count = entries.len() as u32;
        if let Ok(mut f) = self.factory.lock() {
            *f = entries;
        }
        self.ctrl.refresh_factory_corpus();
        self.rebuild_corpus_json();
        count
    }

    /// Rebuild `corpus_json` from the shared corpus snapshot (same projection
    /// the native wry editor pushes via `applyPresetCorpus`).
    fn rebuild_corpus_json(&mut self) {
        let json = self
            .corpus
            .lock()
            .map(|c| corpus_snapshot_json(&c, UNCATEGORIZED))
            .unwrap_or_else(|_| "{\"factory\":[],\"user\":[]}".to_string());
        self.corpus_json.clear();
        self.corpus_json.extend_from_slice(json.as_bytes());
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
        let mut corpus_dirty = false;
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
                ViewEvent::PresetLoaded {
                    meta,
                    source,
                    warnings,
                } => {
                    push_u32(&mut self.view_out, VE_PRESET_LOADED);
                    push_str(&mut self.view_out, &meta.name);
                    match source {
                        None => push_u32(&mut self.view_out, PRESET_SRC_NONE),
                        Some(PresetSource::Factory { index }) => {
                            push_u32(&mut self.view_out, PRESET_SRC_FACTORY);
                            push_u32(&mut self.view_out, index as u32);
                        }
                        Some(PresetSource::User { path }) => {
                            push_u32(&mut self.view_out, PRESET_SRC_USER);
                            push_str(&mut self.view_out, &path.display().to_string());
                        }
                    }
                    push_u32(&mut self.view_out, warnings.len() as u32);
                    for w in &warnings {
                        push_str(&mut self.view_out, w);
                    }
                    count += 1;
                }
                ViewEvent::PresetCorpusChanged { follow } => {
                    // The core controller already refreshed the shared corpus
                    // snapshot (save/rename/delete/move/folder op, or EditorReady).
                    // Pack the notice + flag a corpus_json rebuild so JS re-pushes
                    // `applyPresetCorpus` and flushes the write journal.
                    push_u32(&mut self.view_out, VE_PRESET_CORPUS_CHANGED);
                    match &follow {
                        Some(p) => {
                            push_u32(&mut self.view_out, 1);
                            push_str(&mut self.view_out, &p.display().to_string());
                        }
                        None => push_u32(&mut self.view_out, 0),
                    }
                    count += 1;
                    corpus_dirty = true;
                }
                // Status / text-input ViewEvents are skipped here; status /
                // text-input are E018-handled in JS.
                _ => {}
            }
        }
        // Backpatch the record count into the header.
        self.view_out[0..4].copy_from_slice(&count.to_le_bytes());
        // Rebuild the browser-facing corpus JSON once if anything changed it.
        if corpus_dirty {
            self.rebuild_corpus_json();
        }
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

// ---- presets: factory bank (E019 / 0062) ------------------------------------

/// Reserve `len` bytes in the factory staging buffer and return a pointer into
/// wasm memory JS writes the fetched baked asset into, before calling
/// [`vxnc_load_factory`]. The buffer is owned by the controller; the pointer is
/// valid until the next reserve (a `Vec` realloc can move it).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_factory_buf_reserve(len: u32) -> *mut u8 {
    let s = state();
    s.factory_in.clear();
    s.factory_in.resize(len as usize, 0);
    s.factory_in.as_mut_ptr()
}

/// Parse the `len` bytes JS staged via [`vxnc_factory_buf_reserve`] into the
/// factory bank, republish the corpus, and rebuild the corpus JSON. Returns the
/// preset count (0 on a malformed asset). After this, [`vxnc_corpus_json_ptr`] /
/// [`vxnc_corpus_json_len`] address the browser payload.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_load_factory(len: u32) -> u32 {
    state().load_factory(len as usize)
}

/// Pointer to the UTF-8 corpus JSON (built by [`vxnc_load_factory`]) — the same
/// shape the native wry editor feeds `window.__vxn.applyPresetCorpus`.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_corpus_json_ptr() -> *const u8 {
    state().corpus_json.as_ptr()
}

/// Byte length of the corpus JSON.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_corpus_json_len() -> u32 {
    state().corpus_json.len() as u32
}

/// Load factory preset `index` (`UiEvent::LoadPreset { Factory }`). The model
/// restore + `ParamChanged` fan-out + `PresetLoaded` land on the next
/// [`vxnc_tick`].
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_load_factory(index: u32) {
    state().post(UiEvent::LoadPreset {
        source: PresetSource::Factory {
            index: index as usize,
        },
    });
}

// ---- presets: user side (E019 / 0064) ---------------------------------------
//
// User-preset ops carry strings (names, paths, folders). The JS glue stages the
// UTF-8 bytes in the arg buffer via `vxnc_arg_buf_reserve`, then calls the
// opcode with the byte LENGTHS; the opcode slices the buffer (sequential layout)
// and posts the matching `UiEvent`. The core controller mutates the in-memory
// `UserState` cache + journals the persistence op + refreshes the corpus on the
// next `vxnc_tick`; JS drains the journal (`vxnc_take_journal`) and flushes it to
// IndexedDB off the tick. An `Option<String>` folder rides `len == ARG_NONE`.

/// Reserve `len` bytes in the opcode-argument staging buffer and return a
/// pointer JS writes the concatenated UTF-8 arguments into, before calling a
/// string-taking user opcode. Valid until the next reserve (a `Vec` realloc can
/// move it).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_arg_buf_reserve(len: u32) -> *mut u8 {
    let s = state();
    s.arg_in.clear();
    s.arg_in.resize(len as usize, 0);
    s.arg_in.as_mut_ptr()
}

/// `UiEvent::SavePreset` — args: `name` then `folder` (folder `len == ARG_NONE`
/// → root). Snapshots the model blob on the next tick and writes through the
/// user store.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_save_preset(name_len: u32, folder_len: u32) {
    let s = state();
    let name = s.arg_string(0, name_len as usize);
    let folder = (folder_len != ARG_NONE).then(|| s.arg_string(name_len as usize, folder_len as usize));
    s.post(UiEvent::SavePreset { name, folder });
}

/// `UiEvent::LoadPreset { User }` — arg: the synthetic preset path.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_load_user(path_len: u32) {
    let s = state();
    let path = s.arg_string(0, path_len as usize);
    s.post(UiEvent::LoadPreset {
        source: PresetSource::User {
            path: PathBuf::from(path),
        },
    });
}

/// `UiEvent::RenamePreset` — args: `path` then `new_name`.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_rename_preset(path_len: u32, name_len: u32) {
    let s = state();
    let path = s.arg_string(0, path_len as usize);
    let new_name = s.arg_string(path_len as usize, name_len as usize);
    s.post(UiEvent::RenamePreset {
        path: PathBuf::from(path),
        new_name,
    });
}

/// `UiEvent::DeletePreset` — arg: the preset path.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_delete_preset(path_len: u32) {
    let s = state();
    let path = s.arg_string(0, path_len as usize);
    s.post(UiEvent::DeletePreset {
        path: PathBuf::from(path),
    });
}

/// `UiEvent::MovePreset` — args: `path` then `dest_folder` (folder `len ==
/// ARG_NONE` → move to root).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_move_preset(path_len: u32, folder_len: u32) {
    let s = state();
    let path = s.arg_string(0, path_len as usize);
    let dest_folder =
        (folder_len != ARG_NONE).then(|| s.arg_string(path_len as usize, folder_len as usize));
    s.post(UiEvent::MovePreset {
        path: PathBuf::from(path),
        dest_folder,
    });
}

/// `UiEvent::RenameFolder` — args: `old_name` then `new_name`.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_rename_folder(old_len: u32, new_len: u32) {
    let s = state();
    let old_name = s.arg_string(0, old_len as usize);
    let new_name = s.arg_string(old_len as usize, new_len as usize);
    s.post(UiEvent::RenameFolder { old_name, new_name });
}

/// `UiEvent::DeleteFolder` — arg: the folder name.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_delete_folder(name_len: u32) {
    let s = state();
    let name = s.arg_string(0, name_len as usize);
    s.post(UiEvent::DeleteFolder { name });
}

/// `UiEvent::NewFolder` — arg: the suggested folder name (the store uniquifies).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_new_folder(suggested_len: u32) {
    let s = state();
    let suggested = s.arg_string(0, suggested_len as usize);
    s.post(UiEvent::NewFolder { suggested });
}

/// `UiEvent::StepPreset` — walk the combined factory+user list by `delta`.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_step_preset(delta: i32) {
    state().post(UiEvent::StepPreset { delta });
}

// ---- presets: write journal drain (deferred flush, E019 / 0064) -------------

/// Drain the user store's pending-write journal into the packed out-buffer (see
/// `JW_*`), returning the byte length. JS decodes it into `applyWrites` ops and
/// flushes them to IndexedDB OFF the tick. Empty on a tick with no writes.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_take_journal() -> u32 {
    state().take_journal()
}

/// Pointer to the packed journal out-buffer. Valid until the next
/// `vxnc_take_journal`.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_journal_out_ptr() -> *const u8 {
    state().journal_out.as_ptr()
}

// ---- presets: boot hydration (seed the cache from IndexedDB, E019 / 0064) ---
//
// At boot, before the controller goes live, JS reads the persisted user corpus
// out of IndexedDB and replays it into the in-memory cache WITHOUT journalling
// (it's already on disk). After replaying every folder + preset, JS calls
// `vxnc_hydrate_done` to refresh the corpus snapshot + rebuild the corpus JSON.

/// Register a hydrated (already-persisted) folder — arg: the folder name.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_hydrate_folder(name_len: u32) {
    let s = state();
    let name = s.arg_string(0, name_len as usize);
    if let Ok(mut u) = s.user.lock() {
        u.hydrate_folder(&name);
    }
}

/// Insert a hydrated preset — args: the synthetic `key` (path) then the
/// `preset_record` bytes. Returns 1 on success, 0 if the record fails to decode
/// (a corrupt/foreign blob is skipped, not fatal).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_hydrate_preset(key_len: u32, rec_len: u32) -> u32 {
    let s = state();
    let key = s.arg_string(0, key_len as usize);
    let rec_bytes = s.arg_slice(key_len as usize, rec_len as usize).to_vec();
    match preset_record::decode(&rec_bytes) {
        Ok(rec) => {
            if let Ok(mut u) = s.user.lock() {
                u.hydrate_preset(&key, rec);
            }
            1
        }
        Err(_) => 0,
    }
}

/// Finish hydration: refresh the user corpus snapshot from the now-seeded cache
/// and rebuild the corpus JSON so JS can push `applyPresetCorpus`.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_hydrate_done() {
    let s = state();
    s.ctrl.refresh_user_corpus();
    s.rebuild_corpus_json();
}

// ---- full patch-state autosave / restore (E019 / 0065) ----------------------
//
// On desktop the host persists the plugin-state blob; on the web there is no
// host, so the page autosaves the live patch and restores it on reload. The blob
// is the SAME canonical state codec native CLAP host state and factory presets
// use (`write_state_bytes` — params + key mode + split point), so a session blob
// round-trips across native and wasm. Distinct from user presets: this is the
// single "last session" patch, not a named corpus entry.

/// Snapshot the full patch state into `state_out` and return its byte length.
/// After this, [`vxnc_state_out_ptr`] addresses the blob (valid until the next
/// snapshot/restore). JS copies it out and writes it to browser storage off the
/// tick. The blob captures every param plus key mode + split point.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_snapshot_state() -> u32 {
    let s = state();
    let blob = s.model.snapshot_bytes();
    s.state_out.clear();
    s.state_out.extend_from_slice(&blob);
    s.state_out.len() as u32
}

/// Pointer to the snapshot blob staged by [`vxnc_snapshot_state`] (also the
/// staging buffer [`vxnc_restore_state`] reads). Valid until the next
/// snapshot/restore call.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_state_out_ptr() -> *const u8 {
    state().state_out.as_ptr()
}

/// Reserve `len` bytes in the state staging buffer and return a pointer JS writes
/// a saved state blob into, before calling [`vxnc_restore_state`]. Valid until
/// the next reserve (a `Vec` realloc can move it).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_state_buf_reserve(len: u32) -> *mut u8 {
    let s = state();
    s.state_out.clear();
    s.state_out.resize(len as usize, 0);
    s.state_out.as_mut_ptr()
}

/// Restore the model from the `len`-byte blob JS staged via
/// [`vxnc_state_buf_reserve`]. Returns 1 on success, 0 if the blob is malformed
/// or the wrong length (the codec rejects it and the model is left untouched, so
/// the caller falls back to defaults). Call before re-broadcasting `EditorReady`
/// so the broadcast seeds the UI + param SAB with the restored values; key mode
/// + split point ride the same blob and are republished on the post-tick poll.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_restore_state(len: u32) -> u32 {
    let s = state();
    let n = (len as usize).min(s.state_out.len());
    match s.model.restore_from_bytes(&s.state_out[..n]) {
        Ok(()) => 1,
        Err(_) => 0,
    }
}

// ===========================================================================
// Tests (E019 / 0062) — the factory read path, host-run
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use vxn_app::PresetRecord;
    use vxn_app::factory_asset::{self, FactoryEntry};

    fn entry(name: &str, category: Option<&str>, blob: Vec<u8>) -> FactoryEntry {
        FactoryEntry {
            meta: PresetMeta {
                name: name.into(),
                author: None,
                category: category.map(Into::into),
                comment: None,
            },
            blob,
        }
    }

    // Acceptance (0062): the wasm-targeted store path reads a baked asset and
    // `factory_len()` > 0 — here through the full `vxnc_load_factory` core
    // (decode → store → corpus refresh → corpus JSON).
    #[test]
    fn load_factory_populates_store_and_corpus() {
        let asset = factory_asset::encode(&[
            entry("Init", Some("Bass"), vec![1, 2, 3]),
            entry("Lead", Some("Lead"), vec![4, 5]),
        ]);

        let mut state = ControllerState::new();
        state.factory_in = asset;
        let n = state.load_factory(state.factory_in.len());

        assert_eq!(n, 2);
        assert!(state.ctrl.preset_store().factory_len() > 0);
        assert_eq!(state.ctrl.preset_store().factory_len(), 2);
        assert_eq!(
            state.ctrl.preset_store().factory_meta(0).unwrap().name,
            "Init"
        );
        assert_eq!(state.ctrl.preset_store().factory_load(1).unwrap().blob, vec![4, 5]);

        let json = String::from_utf8(state.corpus_json.clone()).unwrap();
        assert!(json.contains("\"factory\""));
        assert!(json.contains("\"Init\""));
        assert!(json.contains("\"Lead\""));
    }

    // A malformed asset leaves the store empty and reports 0 (no panic).
    #[test]
    fn load_factory_rejects_garbage() {
        let mut state = ControllerState::new();
        state.factory_in = vec![0xde, 0xad, 0xbe, 0xef];
        assert_eq!(state.load_factory(4), 0);
        assert_eq!(state.ctrl.preset_store().factory_len(), 0);
    }

    // Decode the packed journal (vxnc_take_journal layout) into (tag, key/name)
    // pairs so the test can assert the flush payload without the JS decoder.
    fn decode_journal(buf: &[u8]) -> Vec<(u32, String)> {
        let mut off = 0usize;
        let rd_u32 = |b: &[u8], o: &mut usize| {
            let v = u32::from_le_bytes(b[*o..*o + 4].try_into().unwrap());
            *o += 4;
            v
        };
        let count = rd_u32(buf, &mut off);
        let mut out = Vec::new();
        for _ in 0..count {
            let tag = rd_u32(buf, &mut off);
            let nlen = rd_u32(buf, &mut off) as usize;
            let name = String::from_utf8(buf[off..off + nlen].to_vec()).unwrap();
            off += nlen;
            if tag == JW_PUT {
                let blen = rd_u32(buf, &mut off) as usize;
                off += blen; // skip the record bytes
            }
            out.push((tag, name));
        }
        out
    }

    // E019 / 0064: a user save mutates the cache, rebuilds the corpus JSON in the
    // same drain (PresetCorpusChanged), and journals the persistence ops for the
    // deferred flush.
    #[test]
    fn user_save_rebuilds_corpus_and_journals() {
        let mut st = ControllerState::new();
        st.post(UiEvent::NewFolder { suggested: "Leads".into() });
        st.post(UiEvent::SavePreset {
            name: "Hero".into(),
            folder: Some("Leads".into()),
        });
        st.tick();

        // Corpus JSON reflects the save synchronously (AC2).
        let json = String::from_utf8(st.corpus_json.clone()).unwrap();
        assert!(json.contains("\"Hero\""), "corpus lists the saved preset");
        assert!(json.contains("\"Leads\""), "corpus lists the folder");

        // The flush journal carries a PutFolder + a Put for the preset.
        let len = st.take_journal() as usize;
        let ops = decode_journal(&st.journal_out[..len]);
        assert!(
            ops.iter().any(|(t, n)| *t == JW_PUT_FOLDER && n == "Leads"),
            "journal has PutFolder(Leads): {ops:?}"
        );
        assert!(
            ops.iter().any(|(t, n)| *t == JW_PUT && n == "Leads/Hero.toml"),
            "journal has Put(Leads/Hero.toml): {ops:?}"
        );
        // Drained — a second take is empty.
        assert_eq!(st.take_journal(), 4, "empty journal packs just the u32 count");
    }

    // E019 / 0064: hydration replays persisted records WITHOUT journalling, then
    // hydrate_done refreshes the corpus snapshot + rebuilds the corpus JSON.
    #[test]
    fn hydrate_seeds_cache_without_journalling() {
        // Build a record the way the store persists one.
        let blob = vec![9u8, 8, 7];
        let rec = preset_record::encode(&PresetRecord {
            meta: PresetMeta {
                name: "Warm".into(),
                ..Default::default()
            },
            blob: blob.clone(),
        });

        let mut st = ControllerState::new();
        if let Ok(mut u) = st.user.lock() {
            u.hydrate_folder("Pads");
            u.hydrate_preset("Pads/Warm.toml", preset_record::decode(&rec).unwrap());
        }
        // Nothing journalled by hydration.
        assert_eq!(st.take_journal(), 4, "hydration does not journal");

        st.ctrl.refresh_user_corpus();
        st.rebuild_corpus_json();
        let json = String::from_utf8(st.corpus_json.clone()).unwrap();
        assert!(json.contains("\"Warm\""), "hydrated preset shows in corpus");
        assert!(json.contains("\"Pads\""), "hydrated folder shows in corpus");

        // The hydrated preset is loadable through the store.
        let load = st
            .ctrl
            .preset_store()
            .user_load(Path::new("Pads/Warm.toml"))
            .unwrap();
        assert_eq!(load.blob, blob);
    }

    // E019 / 0065: a full-state snapshot round-trips through restore — every
    // param plus the non-param key mode + split point (AC1).
    #[test]
    fn snapshot_state_round_trips_through_restore() {
        let mut st = ControllerState::new();
        // Edit a param, key mode, and split point away from defaults.
        st.model.set(ParamId::new(0), 0.321);
        st.model.set(ParamId::new(TOTAL_PARAMS - 1), 0.654);
        st.model.set_key_mode(KeyMode::Split);
        st.model.set_split_point(48);

        // Snapshot it (the autosave blob).
        let len = vxnc_snapshot_state_on(&mut st);
        let blob: Vec<u8> = st.state_out[..len].to_vec();
        assert_eq!(len, vxn_app::BLOB_LEN, "blob is the canonical state length");

        // A fresh controller at cold defaults restores from the blob.
        let mut fresh = ControllerState::new();
        assert_eq!(fresh.model.key_mode(), KeyMode::Whole, "cold default");
        fresh.state_out.clear();
        fresh.state_out.extend_from_slice(&blob);
        assert_eq!(restore_on(&mut fresh, blob.len()), 1, "restore succeeds");

        assert!((fresh.model.get(ParamId::new(0)) - 0.321).abs() < 1e-6);
        assert!((fresh.model.get(ParamId::new(TOTAL_PARAMS - 1)) - 0.654).abs() < 1e-6);
        assert_eq!(fresh.model.key_mode(), KeyMode::Split, "key mode restored");
        assert_eq!(fresh.model.split_point(), 48, "split point restored");
    }

    // E019 / 0065: a corrupt / wrong-length blob is rejected and leaves the model
    // untouched, so the caller falls back to defaults (AC3).
    #[test]
    fn restore_rejects_bad_blob_without_mutating() {
        let mut st = ControllerState::new();
        st.model.set(ParamId::new(0), 0.5);
        st.model.set_key_mode(KeyMode::Dual);

        // Wrong length.
        st.state_out = vec![0u8; 4];
        assert_eq!(restore_on(&mut st, 4), 0, "short blob rejected");
        // Right length, bad magic.
        st.state_out = vec![0u8; vxn_app::BLOB_LEN];
        assert_eq!(restore_on(&mut st, vxn_app::BLOB_LEN), 0, "bad magic rejected");

        // Model untouched.
        assert!((st.model.get(ParamId::new(0)) - 0.5).abs() < 1e-6);
        assert_eq!(st.model.key_mode(), KeyMode::Dual);
    }

    // Test shims around the global-state opcodes (the `extern "C"` entry points go
    // through the static STATE; these drive a borrowed instance directly).
    fn vxnc_snapshot_state_on(st: &mut ControllerState) -> usize {
        let blob = st.model.snapshot_bytes();
        st.state_out.clear();
        st.state_out.extend_from_slice(&blob);
        st.state_out.len()
    }
    fn restore_on(st: &mut ControllerState, len: usize) -> u32 {
        let n = len.min(st.state_out.len());
        match st.model.restore_from_bytes(&st.state_out[..n]) {
            Ok(()) => 1,
            Err(_) => 0,
        }
    }
}
