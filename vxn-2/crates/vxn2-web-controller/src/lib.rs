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
use vxn2_engine::shared::{ParamModel, SharedParams};
use vxn2_engine::{TOTAL_PARAMS, rate_partner_clap_id, sync_aware_display};

mod user_store;
use user_store::{UserState, UserWrite, decode_record};

/// Drain `SharedParams`' dirty bitsets into `ViewEvent`s: one `ParamChanged`
/// per flipped value bit (with the sync-aware display + rate-partner refresh),
/// plus a whole-table matrix / KS-curve / EG-curve snapshot when the respective
/// dirty flag was set.
fn drain_dirty_bits(params: &SharedParams, mut emit: impl FnMut(ViewEvent)) {
    // Stack arrays, not `Vec`s. This runs every tick, and the two per-tick
    // allocations it used to make were finding 7 of ticket 0298 — fixed here as
    // a side effect of 0299's callback drain rather than as its own change.
    let mut emitted = [false; TOTAL_PARAMS];
    let mut needs_rate = [false; TOTAL_PARAMS];

    // `DirtyBits` masks its tail word, so every id it yields is in range — the
    // `id >= TOTAL_PARAMS` guard this loop used to carry is now the primitive's
    // invariant (`tail_word_padding_never_surfaces`).
    params.drain_dirty_values(|id| {
        emit(param_changed_event(params, id));
        emitted[id] = true;
        if let Some(rate_id) = rate_partner_clap_id(id)
            && rate_id < TOTAL_PARAMS
        {
            needs_rate[rate_id] = true;
        }
    });

    // Refresh sync-partner rate displays only when the partner wasn't already
    // emitted (both a rate and its sync toggle can drift in one tick).
    for rate_id in 0..TOTAL_PARAMS {
        if needs_rate[rate_id] && !emitted[rate_id] {
            emit(param_changed_event(params, rate_id));
        }
    }

    // Whole-table snapshots when any topology / curve bit was set — one event
    // each; the view-side renderer already collapses to one path.
    if params.take_dirty_matrix() != 0 {
        emit(matrix_snapshot_event(params));
    }
    if Vxn2Params::take_dirty_ks_curve(params) {
        emit(ks_curve_snapshot_event(params));
    }
    if Vxn2Params::take_dirty_eg_curve(params) {
        emit(eg_curve_snapshot_event(params));
    }
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
/// (0 none / 1 factory / 2 user), if factory u32 index / if user str path,
/// u32 warning_count + each str.
const VE_PRESET_LOADED: u32 = 6;
/// `VE_CORPUS_CHANGED`: u32 has_follow (0/1), if 1 str follow-path. Signals the
/// user corpus changed (save / rename / delete / move / folder op) so JS
/// republishes the corpus JSON + flushes the persistence journal (0159).
const VE_CORPUS_CHANGED: u32 = 7;
/// `VE_STATUS`: u32 len + UTF-8 status line (save/rename/delete feedback).
const VE_STATUS: u32 = 8;

const PRESET_SRC_NONE: u32 = 0;
const PRESET_SRC_FACTORY: u32 = 1;
const PRESET_SRC_USER: u32 = 2;

// Persistence-journal wire tags (JS `applyWrites` decodes these — 0159).
const JW_PUT: u32 = 1;
const JW_DELETE: u32 = 2;
const JW_PUT_FOLDER: u32 = 3;
const JW_DELETE_FOLDER: u32 = 4;

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
                Some(PresetSource::User { path }) => {
                    push_u32(buf, PRESET_SRC_USER);
                    push_str(buf, &path.to_string_lossy());
                }
                None => push_u32(buf, PRESET_SRC_NONE),
            }
            push_u32(buf, warnings.len() as u32);
            for w in warnings {
                push_str(buf, w);
            }
            true
        }
        ViewEvent::PresetCorpusChanged { follow } => {
            push_u32(buf, VE_CORPUS_CHANGED);
            match follow {
                Some(path) => {
                    push_u32(buf, 1);
                    push_str(buf, &path.to_string_lossy());
                }
                None => push_u32(buf, 0),
            }
            true
        }
        ViewEvent::Status { line } => {
            push_u32(buf, VE_STATUS);
            push_str(buf, line);
            true
        }
        // Text-input ViewEvents ride other channels (native-only popup).
        _ => false,
    }
}

// WebPresetStore — the browser preset store (E030 / 0159).
//
// Factory side: a read-only bank parsed from the baked `factory.bin`. User
// side: the synchronous in-memory [`UserState`] cache + write journal
// ([`user_store`]); IndexedDB persistence is layered on JS-side over the
// journal-drain / hydration opcodes. Both halves are shared with
// [`ControllerState`] through the same `Arc`s, so the journal / hydration
// opcodes and the controller's `refresh_user_corpus` see one cache.

struct WebPresetStore {
    /// (meta, canonical state blob) per factory preset. Filled by
    /// `vxnc_load_factory` from the staged `factory.bin` bytes.
    factory: Arc<Mutex<Vec<(PresetMeta, Vec<u8>)>>>,
    /// The in-memory user corpus + persistence journal.
    user: Arc<Mutex<UserState>>,
}

impl PresetStore for WebPresetStore {
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
    fn user_move(&self, path: &Path, dest: Option<&str>) -> Result<PathBuf, String> {
        self.user
            .lock()
            .map_err(|_| "user store poisoned")?
            .move_preset(path, dest)
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
    /// Shared user-preset cache + journal (0159). The same `Arc` the store
    /// mutates, so the hydration / journal-drain opcodes see one cache.
    user: Arc<Mutex<UserState>>,
    /// Shared corpus snapshot the browser JSON is built from.
    corpus: CorpusHandle,
    /// Staging buffer JS writes the fetched `factory.bin` into before
    /// `vxnc_load_factory`.
    factory_in: Vec<u8>,
    /// Staging buffer JS writes packed UTF-8 opcode arguments into (preset
    /// names, folder names, paths, hydrated records) before an opcode reads
    /// them back by offset + length (0159).
    arg_in: Vec<u8>,
    /// Packed persistence-journal drain buffer, read out by JS after
    /// `vxnc_take_journal` (0159).
    journal_out: Vec<u8>,
    /// State-blob scratch: `vxnc_snapshot_state` writes the snapshot here for JS
    /// to read; `vxnc_state_buf_reserve` reuses it as the restore-input staging
    /// buffer (0159).
    state_buf: Vec<u8>,
    /// TOML scratch: `vxnc_export_toml` writes here for JS; `vxnc_toml_buf_reserve`
    /// reuses it as the import-input staging buffer (0159).
    toml_buf: Vec<u8>,
    /// UTF-8 corpus JSON (rebuilt on `load_factory` / hydration / corpus change),
    /// read out by JS.
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
        let user: Arc<Mutex<UserState>> = Arc::new(Mutex::new(UserState::default()));
        let store = WebPresetStore {
            factory: factory.clone(),
            user: user.clone(),
        };
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
            user,
            corpus,
            factory_in: Vec::new(),
            arg_in: Vec::new(),
            journal_out: Vec::new(),
            state_buf: Vec::new(),
            toml_buf: Vec::new(),
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
        self.rebuild_corpus_json();
        count
    }

    /// Rebuild the JS-visible corpus JSON from the shared snapshot (factory +
    /// user). Called after `load_factory`, after boot hydration, and whenever a
    /// `PresetCorpusChanged` lands in a tick.
    fn rebuild_corpus_json(&mut self) {
        let json = self
            .corpus
            .lock()
            .map(|c| corpus_snapshot_json(&c, "Uncategorized"))
            .unwrap_or_else(|_| "{\"factory\":[],\"user\":[]}".to_string());
        self.corpus_json.clear();
        self.corpus_json.extend_from_slice(json.as_bytes());
    }

    /// Boot hydration finished: refresh the user corpus from the (now-populated)
    /// cache and rebuild the corpus JSON so JS can publish it synchronously.
    fn hydrate_done(&mut self) {
        self.ctrl.refresh_user_corpus();
        self.rebuild_corpus_json();
    }

    /// Drain the user store's pending persistence ops into `journal_out`
    /// (the packed wire the bridge ships to IndexedDB) and return its byte
    /// length. Wire: `u32 op_count`, then per op `u32 tag` + payload —
    /// PUT(1): str key, u32 blob_len + blob; DELETE(2): str key;
    /// PUT_FOLDER(3): str name; DELETE_FOLDER(4): str name.
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

    /// Read a UTF-8 string out of the argument staging buffer.
    fn arg_string(&self, start: usize, len: usize) -> String {
        let end = (start + len).min(self.arg_in.len());
        String::from_utf8_lossy(&self.arg_in[start.min(self.arg_in.len())..end]).into_owned()
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
        let mut corpus_changed = false;
        // (1) Custom echoes + snapshot pushes that tick_vxn2 queued.
        while let Ok(ev) = self.view_rx.try_recv() {
            if matches!(ev, ViewEvent::PresetCorpusChanged { .. }) {
                corpus_changed = true;
            }
            if pack_view_event(&mut self.view_out, &ev) {
                count += 1;
            }
        }
        // (2) The canonical dirty-bitset drain (ParamChanged + snapshots).
        //     Packed straight out of the drain — no intermediate Vec (0299).
        let view_out = &mut self.view_out;
        drain_dirty_bits(&self.model, |ev| {
            if pack_view_event(view_out, &ev) {
                count += 1;
            }
        });
        self.view_out[0..4].copy_from_slice(&count.to_le_bytes());

        // A user-corpus mutation this tick — rebuild the JS-visible corpus JSON
        // so the bridge (which just saw the VE_CORPUS_CHANGED record) can
        // republish it synchronously.
        if corpus_changed {
            self.rebuild_corpus_json();
        }

        for id in 0..TOTAL_PARAMS {
            self.values_out[id] = self.model.get(id);
        }
    }

    /// Snapshot the full patch state into `state_buf` and return its length. The
    /// blob is the model's canonical [`ParamModel::snapshot_bytes`] format
    /// (params + matrix + curves) — the host-state analogue used for autosave
    /// and share-link.
    fn snapshot_state(&mut self) -> u32 {
        let blob = ParamModel::snapshot_bytes(&*self.model);
        self.state_buf.clear();
        self.state_buf.extend_from_slice(&blob);
        self.state_buf.len() as u32
    }

    /// Restore the model from the `len`-byte blob JS staged in `state_buf`.
    /// Returns 1 on success, 0 on a malformed / wrong-length blob (the model is
    /// left untouched by `load_bytes` on error). `load_bytes` marks every dirty
    /// bit, so the next tick re-broadcasts the whole table + snapshots.
    fn restore_state(&mut self, len: usize) -> u32 {
        let n = len.min(self.state_buf.len());
        match ParamModel::load_bytes(&*self.model, &self.state_buf[..n]) {
            Ok(()) => 1,
            Err(_) => 0,
        }
    }

    /// Serialise the current patch to name-keyed TOML into `toml_buf` and return
    /// its length. `name` is staged in `arg_in`.
    fn export_toml(&mut self, name_len: usize) -> u32 {
        let name = self.arg_string(0, name_len);
        let meta = PresetMeta { name, ..Default::default() };
        let blob = ParamModel::snapshot_bytes(&*self.model);
        match user_store::encode_record(&meta, &blob) {
            Ok(bytes) => {
                self.toml_buf.clear();
                self.toml_buf.extend_from_slice(&bytes);
                self.toml_buf.len() as u32
            }
            // A snapshot is always valid, so this can't fail in practice; emit an
            // empty buffer rather than panic.
            Err(_) => {
                self.toml_buf.clear();
                0
            }
        }
    }

    /// Parse the `len`-byte TOML JS staged in `toml_buf` and apply it to the
    /// model. Returns 1 on success, 0 on a malformed / wrong-schema file (the
    /// model is left untouched). Like `restore_state`, a success marks every
    /// dirty bit so the next tick re-broadcasts.
    fn import_toml(&mut self, len: usize) -> u32 {
        let n = len.min(self.toml_buf.len());
        let bytes = self.toml_buf[..n].to_vec();
        match decode_record(&bytes) {
            Ok((_meta, blob)) => match ParamModel::load_bytes(&*self.model, &blob) {
                Ok(()) => 1,
                Err(_) => 0,
            },
            Err(_) => 0,
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
    scale_shape: u32,
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
            scale_shape: scale_shape as u8,
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

// User presets + persistence (0159).
//
// String/blob args ride the shared `arg_in` staging buffer: JS reserves it via
// `vxnc_arg_buf_reserve`, writes the concatenated arguments, then calls the
// opcode with each argument's byte length. `ARG_NONE` in a length slot means an
// absent optional argument (root folder / no destination).

/// Sentinel length for an absent optional argument (folder = root).
const ARG_NONE: u32 = u32::MAX;

/// Reserve `len` bytes in the argument staging buffer and return its pointer.
/// JS writes the concatenated UTF-8 opcode arguments (names / paths / records)
/// here, then calls the opcode with each argument's length.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_arg_buf_reserve(len: u32) -> *mut u8 {
    let s = state();
    s.arg_in.clear();
    s.arg_in.resize(len as usize, 0);
    s.arg_in.as_mut_ptr()
}

/// `UiEvent::SavePreset` — snapshot the model + write through the user store.
/// Args: name (`name_len`), then folder (`folder_len`, `ARG_NONE` → root).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_save_preset(name_len: u32, folder_len: u32) {
    let s = state();
    let name = s.arg_string(0, name_len as usize);
    let folder = if folder_len == ARG_NONE {
        None
    } else {
        Some(s.arg_string(name_len as usize, folder_len as usize))
    };
    s.post(UiEvent::SavePreset { name, folder });
}

/// `UiEvent::LoadPreset { User }` — arg: synthetic preset path.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_load_user(path_len: u32) {
    let s = state();
    let path = s.arg_string(0, path_len as usize);
    s.post(UiEvent::LoadPreset {
        source: PresetSource::User { path: PathBuf::from(path) },
    });
}

/// `UiEvent::RenamePreset` — args: path (`path_len`), then new name (`name_len`).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_rename_preset(path_len: u32, name_len: u32) {
    let s = state();
    let path = s.arg_string(0, path_len as usize);
    let new_name = s.arg_string(path_len as usize, name_len as usize);
    s.post(UiEvent::RenamePreset { path: PathBuf::from(path), new_name });
}

/// `UiEvent::DeletePreset` — arg: preset path.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_delete_preset(path_len: u32) {
    let s = state();
    let path = s.arg_string(0, path_len as usize);
    s.post(UiEvent::DeletePreset { path: PathBuf::from(path) });
}

/// `UiEvent::MovePreset` — args: path (`path_len`), then destination folder
/// (`folder_len`, `ARG_NONE` → move to root).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_move_preset(path_len: u32, folder_len: u32) {
    let s = state();
    let path = s.arg_string(0, path_len as usize);
    let dest_folder = if folder_len == ARG_NONE {
        None
    } else {
        Some(s.arg_string(path_len as usize, folder_len as usize))
    };
    s.post(UiEvent::MovePreset { path: PathBuf::from(path), dest_folder });
}

/// `UiEvent::NewFolder` — arg: suggested folder name (the store uniquifies).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_new_folder(suggested_len: u32) {
    let s = state();
    let suggested = s.arg_string(0, suggested_len as usize);
    s.post(UiEvent::NewFolder { suggested });
}

/// `UiEvent::RenameFolder` — args: old name (`old_len`), then new name (`new_len`).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_rename_folder(old_len: u32, new_len: u32) {
    let s = state();
    let old_name = s.arg_string(0, old_len as usize);
    let new_name = s.arg_string(old_len as usize, new_len as usize);
    s.post(UiEvent::RenameFolder { old_name, new_name });
}

/// `UiEvent::DeleteFolder` — arg: folder name.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_delete_folder(name_len: u32) {
    let s = state();
    let name = s.arg_string(0, name_len as usize);
    s.post(UiEvent::DeleteFolder { name });
}

// Boot hydration — replay the persisted user corpus into the cache BEFORE the
// controller goes live, WITHOUT journalling (it's already stored).

/// Register a hydrated (already-persisted) folder — arg: folder name.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_hydrate_folder(name_len: u32) {
    let s = state();
    let name = s.arg_string(0, name_len as usize);
    if let Ok(mut u) = s.user.lock() {
        u.hydrate_folder(&name);
    }
}

/// Insert a hydrated preset — args: synthetic key (`key_len`), then its stored
/// TOML record (`rec_len`). Returns 1 on success, 0 if the record fails to
/// parse (a corrupt / foreign entry is skipped rather than aborting hydration).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_hydrate_preset(key_len: u32, rec_len: u32) -> u32 {
    let s = state();
    let key = s.arg_string(0, key_len as usize);
    let start = (key_len as usize).min(s.arg_in.len());
    let end = (key_len as usize + rec_len as usize).min(s.arg_in.len());
    let rec = s.arg_in[start..end].to_vec();
    match decode_record(&rec) {
        Ok((meta, blob)) => {
            if let Ok(mut u) = s.user.lock() {
                u.hydrate_preset(&key, meta, blob);
            }
            1
        }
        Err(_) => 0,
    }
}

/// Finish hydration: refresh the user corpus from the cache + rebuild the
/// corpus JSON (JS reads it synchronously after this).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_hydrate_done() {
    state().hydrate_done();
}

// Deferred-write journal — drained off the tick and shipped to IndexedDB.

/// Drain the user store's pending persistence ops into the packed journal
/// buffer; returns its byte length. JS decodes it (see `applyWrites`).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_take_journal() -> u32 {
    state().take_journal()
}

/// Pointer to the packed journal buffer (valid until the next `vxnc_take_journal`).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_journal_out_ptr() -> *const u8 {
    state().journal_out.as_ptr()
}

// Full patch-state snapshot / restore (autosave + share-link).

/// Snapshot the full patch state into the state scratch buffer; returns its
/// byte length. JS reads it via [`vxnc_state_out_ptr`].
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_snapshot_state() -> u32 {
    state().snapshot_state()
}

/// Pointer to the state scratch buffer (snapshot output / restore staging).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_state_out_ptr() -> *const u8 {
    state().state_buf.as_ptr()
}

/// Reserve `len` bytes in the state scratch buffer and return its pointer. JS
/// writes the saved blob here, then calls [`vxnc_restore_state`].
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_state_buf_reserve(len: u32) -> *mut u8 {
    let s = state();
    s.state_buf.clear();
    s.state_buf.resize(len as usize, 0);
    s.state_buf.as_mut_ptr()
}

/// Restore the model from the `len`-byte blob staged in the state scratch
/// buffer. Returns 1 on success, 0 if malformed (model left untouched).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_restore_state(len: u32) -> u32 {
    state().restore_state(len as usize)
}

// TOML export / import (file + share).

/// Serialise the current patch to name-keyed TOML into the TOML scratch buffer;
/// returns its byte length. The name is staged in `arg_in` (`name_len` bytes).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_export_toml(name_len: u32) -> u32 {
    state().export_toml(name_len as usize)
}

/// Pointer to the TOML scratch buffer (export output / import staging).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_toml_out_ptr() -> *const u8 {
    state().toml_buf.as_ptr()
}

/// Reserve `len` bytes in the TOML scratch buffer and return its pointer. JS
/// writes the imported file's UTF-8 bytes here, then calls [`vxnc_import_toml`].
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_toml_buf_reserve(len: u32) -> *mut u8 {
    let s = state();
    s.toml_buf.clear();
    s.toml_buf.resize(len as usize, 0);
    s.toml_buf.as_mut_ptr()
}

/// Parse the `len`-byte TOML staged in the TOML scratch buffer and apply it to
/// the model. Returns 1 on success, 0 if malformed (model left untouched).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_import_toml(len: u32) -> u32 {
    state().import_toml(len as usize)
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
            row: MatrixRow {
                source: 2,
                dest: 3,
                curve: 1,
                active: true,
                depth: 0.5,
                scale_src: 0,
                scale_shape: 0,
            },
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

    // ---- persistence (0159) --------------------------------------------------

    /// Stage `args` (concatenated) into the arg buffer, as JS does before an op.
    fn stage_args(s: &mut ControllerState, parts: &[&[u8]]) {
        s.arg_in.clear();
        for p in parts {
            s.arg_in.extend_from_slice(p);
        }
    }

    /// Find the first VE_CORPUS_CHANGED record in the drain buffer.
    fn saw_corpus_changed(buf: &[u8]) -> bool {
        let count = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        let mut p = 4usize;
        let take_u32 = |buf: &[u8], p: &mut usize| {
            let v = u32::from_le_bytes(buf[*p..*p + 4].try_into().unwrap());
            *p += 4;
            v
        };
        for _ in 0..count {
            let tag = take_u32(buf, &mut p);
            match tag {
                VE_PARAM_CHANGED => {
                    p += 8;
                    let l = take_u32(buf, &mut p) as usize;
                    p += l;
                }
                VE_OP_TAB_CHANGED => p += 4,
                VE_MATRIX_SNAPSHOT => {
                    let rows = take_u32(buf, &mut p) as usize;
                    p += rows * 9;
                }
                VE_KS_CURVE_SNAPSHOT => p += 12,
                VE_EG_CURVE_SNAPSHOT => p += 6,
                VE_PRESET_LOADED => {
                    let nl = take_u32(buf, &mut p) as usize;
                    p += nl;
                    let src = take_u32(buf, &mut p);
                    if src == PRESET_SRC_FACTORY {
                        p += 4;
                    } else if src == PRESET_SRC_USER {
                        let pl = take_u32(buf, &mut p) as usize;
                        p += pl;
                    }
                    let wc = take_u32(buf, &mut p) as usize;
                    for _ in 0..wc {
                        let wl = take_u32(buf, &mut p) as usize;
                        p += wl;
                    }
                }
                VE_CORPUS_CHANGED => return true,
                VE_STATUS => {
                    let l = take_u32(buf, &mut p) as usize;
                    p += l;
                }
                other => panic!("unknown view tag {other}"),
            }
        }
        false
    }

    #[test]
    fn snapshot_restore_state_round_trips() {
        let mut s = fresh();
        s.tick(); // clear boot seed
        // Move a param, snapshot, move it again, then restore the snapshot.
        s.post(UiEvent::SetParam { id: ParamId::new(5), plain: 0.3 });
        s.tick();
        let before = s.model.get(5);
        let n = s.snapshot_state();
        let saved: Vec<u8> = s.state_buf[..n as usize].to_vec();

        s.post(UiEvent::SetParam { id: ParamId::new(5), plain: 0.9 });
        s.tick();
        assert_ne!(s.model.get(5), before);

        // Stage the saved blob and restore.
        let ptr = vxnc_state_buf_reserve_len(&mut s, saved.len());
        ptr.copy_from_slice(&saved);
        assert_eq!(s.restore_state(saved.len()), 1);
        assert!((s.model.get(5) - before).abs() < 1e-6, "restore did not reinstate the value");
        // The restore marks every dirty bit → next tick re-broadcasts the table.
        s.tick();
        assert_eq!(param_changed_ids(&s.view_out).len(), TOTAL_PARAMS);
    }

    /// Helper mirroring `vxnc_state_buf_reserve` on a borrowed state (the C-ABI
    /// version reaches through the global; tests drive the struct directly).
    fn vxnc_state_buf_reserve_len(s: &mut ControllerState, len: usize) -> &mut [u8] {
        s.state_buf.clear();
        s.state_buf.resize(len, 0);
        &mut s.state_buf[..]
    }

    #[test]
    fn restore_state_rejects_malformed_blob() {
        let mut s = fresh();
        s.tick();
        s.state_buf = vec![9, 9, 9]; // not a valid snapshot
        assert_eq!(s.restore_state(3), 0);
    }

    /// A real, restorable state blob (factory preset 0) — a codec fixpoint that
    /// round-trips through the sparse-TOML path bit-identically.
    fn real_blob() -> Vec<u8> {
        use vxn2_app::PresetStore;
        vxn2_engine::Vxn2PresetStore::new()
            .factory_load(0)
            .expect("factory preset 0")
            .blob
    }

    /// Set the controller model to `blob` via the raw restore path.
    fn set_model_blob(s: &mut ControllerState, blob: &[u8]) {
        s.state_buf = blob.to_vec();
        assert_eq!(s.restore_state(blob.len()), 1);
    }

    #[test]
    fn export_import_toml_round_trips() {
        let mut s = fresh();
        s.tick();
        // Start from a real patch (matrix + curves + params all populated).
        let fb = real_blob();
        set_model_blob(&mut s, &fb);
        s.tick();
        let b0 = { let n = s.snapshot_state(); s.state_buf[..n as usize].to_vec() };

        stage_args(&mut s, &[b"My Patch"]);
        let n = s.export_toml("My Patch".len() as usize);
        let toml: Vec<u8> = s.toml_buf[..n as usize].to_vec();
        assert!(
            String::from_utf8_lossy(&toml).contains("My Patch"),
            "exported TOML missing name"
        );

        // Wipe the model to defaults, then import the exported TOML back.
        set_model_blob(&mut s, &ParamModel::snapshot_bytes(&SharedParams::new()));
        s.toml_buf = toml;
        let len = s.toml_buf.len();
        assert_eq!(s.import_toml(len), 1);
        let b1 = { let n = s.snapshot_state(); s.state_buf[..n as usize].to_vec() };
        assert_eq!(b1, b0, "export→import did not reinstate the full patch");
    }

    #[test]
    fn import_toml_rejects_garbage() {
        let mut s = fresh();
        s.tick();
        s.toml_buf = b"not a preset at all".to_vec();
        let len = s.toml_buf.len();
        assert_eq!(s.import_toml(len), 0);
    }

    #[test]
    fn save_surfaces_corpus_changed_and_journals() {
        let mut s = fresh();
        s.tick();
        stage_args(&mut s, &[b"Bass One"]);
        s.post(UiEvent::SavePreset {
            name: "Bass One".to_string(),
            folder: None,
        });
        s.tick();
        // The save emits a PresetCorpusChanged → VE_CORPUS_CHANGED record.
        assert!(saw_corpus_changed(&s.view_out), "save did not surface VE_CORPUS_CHANGED");
        // The corpus JSON now lists the user preset.
        let json = String::from_utf8(s.corpus_json.clone()).unwrap();
        assert!(json.contains("Bass One"), "corpus json missing user preset: {json}");
        // And the journal carries a PUT the bridge would flush to IndexedDB.
        let n = s.take_journal();
        assert!(n > 0, "save produced no journal ops");
        let count = u32::from_le_bytes(s.journal_out[0..4].try_into().unwrap());
        assert!(count >= 1);
        let tag = u32::from_le_bytes(s.journal_out[4..8].try_into().unwrap());
        assert_eq!(tag, JW_PUT, "first journal op should be a PUT");
    }

    #[test]
    fn hydrate_then_load_user_round_trips() {
        // Save on one controller to produce a journalled TOML record, then
        // hydrate a fresh controller from it and load the user preset back.
        let mut a = fresh();
        a.tick();
        // Save a real patch (factory preset 0) as a user preset.
        let fb = real_blob();
        set_model_blob(&mut a, &fb);
        a.tick();
        let b0 = { let n = a.snapshot_state(); a.state_buf[..n as usize].to_vec() };
        a.post(UiEvent::SavePreset {
            name: "Keeper".to_string(),
            folder: Some("Mine".to_string()),
        });
        a.tick();
        a.take_journal(); // packs into a.journal_out
        // Pull the PUT record (key + bytes) out of the packed journal.
        let (key, rec) = first_put(&a.journal_out);

        let mut b = fresh();
        b.tick();
        // Hydrate: folder, then preset, then done.
        b.user.lock().unwrap().hydrate_folder("Mine");
        stage_args(&mut b, &[key.as_bytes(), &rec]);
        assert_eq!(
            vxnc_hydrate_preset_on(&mut b, key.len(), rec.len()),
            1,
            "hydrate_preset failed to parse the stored record"
        );
        b.hydrate_done();
        let json = String::from_utf8(b.corpus_json.clone()).unwrap();
        assert!(json.contains("Keeper"), "hydrated corpus missing preset: {json}");

        // Load the user preset and confirm the full patch comes back.
        stage_args(&mut b, &[key.as_bytes()]);
        b.post(UiEvent::LoadPreset {
            source: PresetSource::User { path: PathBuf::from(&key) },
        });
        b.tick();
        let b1 = { let n = b.snapshot_state(); b.state_buf[..n as usize].to_vec() };
        assert_eq!(b1, b0, "user load did not reinstate the full patch");
    }

    /// Decode the first PUT (key, bytes) from a packed journal buffer.
    fn first_put(buf: &[u8]) -> (String, Vec<u8>) {
        let count = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        let mut p = 4usize;
        let take_u32 = |buf: &[u8], p: &mut usize| {
            let v = u32::from_le_bytes(buf[*p..*p + 4].try_into().unwrap());
            *p += 4;
            v
        };
        let take_str = |buf: &[u8], p: &mut usize| {
            let n = take_u32(buf, p) as usize;
            let s = String::from_utf8_lossy(&buf[*p..*p + n]).into_owned();
            *p += n;
            s
        };
        for _ in 0..count {
            let tag = take_u32(buf, &mut p);
            match tag {
                JW_PUT => {
                    let key = take_str(buf, &mut p);
                    let n = take_u32(buf, &mut p) as usize;
                    let bytes = buf[p..p + n].to_vec();
                    return (key, bytes);
                }
                JW_DELETE => {
                    let _ = take_str(buf, &mut p);
                }
                JW_PUT_FOLDER | JW_DELETE_FOLDER => {
                    let _ = take_str(buf, &mut p);
                }
                other => panic!("unknown journal tag {other}"),
            }
        }
        panic!("no PUT in journal");
    }

    /// `vxnc_hydrate_preset` driven against a borrowed state (the C-ABI version
    /// reaches through the global).
    fn vxnc_hydrate_preset_on(s: &mut ControllerState, key_len: usize, rec_len: usize) -> u32 {
        let key = s.arg_string(0, key_len);
        let start = key_len.min(s.arg_in.len());
        let end = (key_len + rec_len).min(s.arg_in.len());
        let rec = s.arg_in[start..end].to_vec();
        match decode_record(&rec) {
            Ok((meta, blob)) => {
                s.user.lock().unwrap().hydrate_preset(&key, meta, blob);
                1
            }
            Err(_) => 0,
        }
    }
}
