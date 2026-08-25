//! `vxn1b-web-controller` — the VXN1b main-thread controller, compiled to wasm
//! (ticket 0290, epic E045).
//!
//! Runs the **same** `vxn_core_app::Controller<SharedParams>` the native
//! [`vxn1b-clap`](../../vxn1b-clap/src/lib.rs) shell drives, so there is one
//! arbiter for off-audio model mutation across native and web rather than two
//! that can disagree. The engine wasm (`vxn1b-wasm`) renders in the
//! AudioWorklet; these are separate linear memories, and the JS glue mirrors
//! this model's plain values into the worklet's param SAB each tick.
//!
//! Boundary: `UiEvent` / `ViewEvent` never cross as Rust types. Inbound, one
//! `vxnc_*` C function per UI intent posts onto the `ui` channel; outbound,
//! [`vxnc_tick`] packs the resulting `ViewEvent`s into a linear-memory scratch
//! ([`vxnc_view_ptr`] / [`vxnc_view_len`]) the JS bridge decodes.
//!
//! # Change detection: echo on, no bitset drain
//!
//! `vxn2-web-controller` is the structural reference — both it and this crate
//! compose the shared `Controller` directly — but its *change detection* is not
//! portable. `vxn2-engine`'s `SharedParams` carries per-param dirty bitsets, so
//! its controller sets `echo_param_writes(false)` and drains those bits as the
//! single Model→View emitter. `vxn1b-engine`'s `SharedParams` has **no value
//! bitset at all** — only `key_dirty` and `reload`. So this controller leaves
//! `echo_param_writes` at its default `true`; copying vxn-2's setup would
//! compile and then emit nothing.
//!
//! Three consequences, each handled explicitly here because the echo alone does
//! not cover them:
//!
//! 1. **The echo's display string is wrong for synced params.**
//!    `Controller::emit_param_changed` uses `descriptor.display()`, which cannot
//!    know about a partner sync toggle. [`ControllerState::pack_param_changed`]
//!    recomputes it with [`sync_aware_display`] at pack time, so every source —
//!    UI write, preset load, re-broadcast — gets the right label from one place.
//! 2. **A sync flip does not change its rate partner's value**, only its label.
//!    [`ControllerState::drain`] synthesises a partner record for any emitted
//!    sync-flag id, the same rule the native shell applies in `push_param_diffs`.
//! 3. **Three paths write the model behind the Controller's back** — a state
//!    restore, a TOML import, and `PatchOp::CopyLayer` (which calls
//!    `SharedParams::copy_layer` directly, moving ~80 params). Each calls
//!    `broadcast_all_params()` explicitly. Under vxn-2's pump these would be
//!    caught by the bits the writes flip; here nothing would report them.
//!
//! Matrix topology and keyboard state are not CLAP params and have no view-side
//! change flag either, so they ride memo diffs
//! ([`ControllerState::push_matrix_echo`] / [`push_key_echo`]) — ports of the
//! native shell's, for exactly the same reason: a preset load moves them with
//! nothing in the param machinery to tell the page.
//!
//! [[E046]] would replace all six of those mechanisms with one dirty-bitset
//! pump; ticket 0303 is the follow-up that deletes them from this file.
//!
//! # No host, no readback
//!
//! vxn-1's web controller runs a NaN-seeded `last_seen` diff over a readback
//! region because its native shell needs one for CLAP host automation. The
//! browser has no host: the only writer of the param SAB is the coordinator,
//! and ticket 0297 removed the readback half of the SAB outright. There is no
//! `pump_readback` here and nothing for one to observe.
//!
//! [`sync_aware_display`]: vxn1b_engine::sync::sync_aware_display
//! [`push_key_echo`]: ControllerState::push_key_echo

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Mutex};

use vxn1b_engine::preset_io::EnginePresetStore;
use vxn1b_engine::shared::SharedParams;
use vxn1b_engine::sync::{rate_partner_clap_id, sync_aware_display};
use vxn1b_engine::{
    KeyOp, KeyState, Layer, MatrixEdit, MatrixField, MatrixTable, PatchOp, TOTAL_PARAMS,
};
use vxn_core_app::{
    Controller, CorpusHandle, ParamId, ParamModel, PresetLoad, PresetMeta, PresetSource,
    PresetStore, UiEvent, UserFolderEntry, ViewEvent, corpus_snapshot_json,
};

mod user_store;
use user_store::{UserState, UserWrite, decode_record};

/// Display label for the virtual root group of the user preset corpus. Matches
/// `vxn1b_ui_web`'s — the native page and the browser page must not disagree
/// about what the ungrouped presets are called (note the -s- spelling; vxn-2
/// uses the -z-).
const UNCATEGORISED: &str = "Uncategorised";

// ViewEvent out-buffer — the single-drain wire format.
//
//   header:  u32 record_count
//   then `record_count` records, each `u32 tag` + tag-specific payload:
//
//   VE_PARAM_CHANGED (1):    u32 id, f32 plain, f32 norm, u32 len, [len UTF-8]
//   VE_MATRIX_SNAPSHOT (2):  per layer (2), per slot (16): u8 source, u8 dest,
//                            u8 curve, u8 scale_src. Depths are params and ride
//                            VE_PARAM_CHANGED — same split as the native echo.
//   VE_KEY_STATE (3):        u8 mode (0 Single / 1 Dual / 2 Split), u8 split
//                            point, u8 lfo2_link
//   VE_PRESET_LOADED (4):    u32 name_len + name, u32 source_kind
//                            (0 none / 1 factory / 2 user), if factory u32 index
//                            / if user str path, u32 warning_count + each str
//   VE_CORPUS_CHANGED (5):   u32 has_follow (0/1), if 1 str follow-path
//   VE_STATUS (6):           u32 len + UTF-8 status line

const VE_PARAM_CHANGED: u32 = 1;
const VE_MATRIX_SNAPSHOT: u32 = 2;
const VE_KEY_STATE: u32 = 3;
const VE_PRESET_LOADED: u32 = 4;
const VE_CORPUS_CHANGED: u32 = 5;
const VE_STATUS: u32 = 6;

const PRESET_SRC_NONE: u32 = 0;
const PRESET_SRC_FACTORY: u32 = 1;
const PRESET_SRC_USER: u32 = 2;

// Persistence-journal wire tags. The shared `preset-persistence.mjs` (0284)
// drives the flush; the per-synth `controller.mjs` decodes these.
const JW_PUT: u32 = 1;
const JW_DELETE: u32 = 2;
const JW_PUT_FOLDER: u32 = 3;
const JW_DELETE_FOLDER: u32 = 4;

/// Sentinel length for an absent optional argument (folder = root).
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

// ── WebPresetStore ───────────────────────────────────────────────────────────
//
// Factory side: delegated straight to `EnginePresetStore`. Unlike vxn-1 and
// vxn-2 — which fetch a baked `factory.bin` at boot — nothing is baked here.
// vxn-1's reason was ADR 0009's "keep the DSP engine out of the lean controller
// wasm", and it does not apply: this crate already links `vxn1b-engine` for
// `SharedParams` and the param table, so the `include_dir!` bank comes with it.
// `EnginePresetStore`'s factory methods touch no filesystem (only `crate::factory`
// + serde), and VXN1b's bank is 8 presets / 32 KB against vxn-2's 206 / 828 KB.
// That buys back an opcode pair, a staging buffer, a second wire format and its
// parser, a boot fetch, and a bake step in the xtask.
//
// User side: the synchronous in-memory `UserState` cache + write journal
// ([`user_store`]); `EnginePresetStore`'s user half is `std::fs`, which on wasm
// compiles to stubs that fail silently rather than erroring. IndexedDB
// persistence is layered on JS-side over the journal-drain / hydration opcodes
// (ticket 0293).

struct WebPresetStore {
    /// Stateless unit struct; holds the embedded factory bank's read path.
    factory: EnginePresetStore,
    /// The in-memory user corpus + persistence journal, shared with
    /// [`ControllerState`] through the same `Arc` so the journal / hydration
    /// opcodes and the controller's `refresh_user_corpus` see one cache.
    user: Arc<Mutex<UserState>>,
}

impl PresetStore for WebPresetStore {
    fn factory_len(&self) -> usize {
        self.factory.factory_len()
    }
    fn factory_load(&self, index: usize) -> Result<PresetLoad, String> {
        self.factory.factory_load(index)
    }
    fn factory_meta(&self, index: usize) -> Option<PresetMeta> {
        self.factory.factory_meta(index)
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
        self.user.lock().map(|u| u.list_tree()).unwrap_or_default()
    }
}

// ── Custom UI ops ────────────────────────────────────────────────────────────

/// Apply one VXN1b `UiEvent::Custom` payload. Mirrors the downcast chain in
/// [`vxn1b-clap`'s timer](../../vxn1b-clap/src/lib.rs#L466) so native and web
/// interpret the same opcodes identically — with two deliberate differences:
///
/// - **No `ScopeOp` arm.** The oscilloscope tap is pure audio-thread state; on
///   the web it rides the event ring straight to the worklet and never reaches
///   the model. Same for the tempo. Routing them here would put view state into
///   the patch.
/// - **`CopyLayer` re-broadcasts.** `SharedParams::copy_layer` writes ~80 params
///   with `set`, rewrites the target layer's matrix and flips the key mode, all
///   without going through the Controller — so with echo-based change detection
///   nothing would tell the page. Natively the shell's `last_seen` poll catches
///   it. See the module docs; [[0303]] deletes this call.
fn apply_custom_ui(ctrl: &mut Controller<SharedParams>, payload: Box<dyn std::any::Any + Send>) {
    let model = ctrl.model().clone();
    let payload = match payload.downcast::<KeyOp>() {
        Ok(op) => return model.apply_key_op(*op),
        Err(p) => p,
    };
    let payload = match payload.downcast::<MatrixEdit>() {
        Ok(edit) => return model.edit_matrix_slot(*edit),
        Err(p) => p,
    };
    if let Ok(op) = payload.downcast::<PatchOp>() {
        match *op {
            PatchOp::CopyLayer { from, to } => model.copy_layer(from, to),
        }
        ctrl.broadcast_all_params();
    }
}

// ── Controller state ─────────────────────────────────────────────────────────
//
// One global instance (the browser main thread hosts exactly one controller).

struct ControllerState {
    ctrl: Controller<SharedParams>,
    model: Arc<SharedParams>,
    view_rx: Receiver<ViewEvent>,
    ui_tx: SyncSender<UiEvent>,
    /// Shared user-preset cache + journal — the same `Arc` the store mutates.
    user: Arc<Mutex<UserState>>,
    /// Shared corpus snapshot the browser JSON is built from.
    corpus: CorpusHandle,

    /// Staging buffer JS writes packed UTF-8 opcode arguments into (preset
    /// names, folder names, paths, hydrated records) before an opcode reads them
    /// back by offset + length.
    arg_in: Vec<u8>,
    /// Packed persistence-journal drain buffer, read out after `vxnc_take_journal`.
    journal_out: Vec<u8>,
    /// State-blob scratch: `vxnc_snapshot_state` writes the snapshot here for JS
    /// to read; `vxnc_state_buf_reserve` reuses it as the restore-input staging
    /// buffer.
    state_buf: Vec<u8>,
    /// TOML scratch: `vxnc_export_toml` writes here; `vxnc_toml_buf_reserve`
    /// reuses it as the import-input staging buffer.
    toml_buf: Vec<u8>,
    /// UTF-8 corpus JSON, rebuilt at most once per tick when dirtied.
    corpus_json: Vec<u8>,
    /// Packed ViewEvent drain buffer JS reads after each tick.
    view_out: Vec<u8>,
    /// Model plain-value snapshot, exported for JS to mirror into the worklet
    /// param SAB (refreshed at the end of every tick).
    values_out: Vec<f32>,

    /// Set by any corpus-mutating path; coalesces multiple corpus-changing
    /// events in one tick into a single JSON rebuild.
    corpus_json_dirty: bool,
    /// Per-tick scratch for the rate-partner refresh: which ids this tick
    /// already emitted. Reused across ticks — `partner_touched` records what to
    /// reset so the clear is O(changed), not O(TOTAL_PARAMS).
    emitted: Vec<bool>,
    partner_touched: Vec<usize>,
    /// Drained-but-not-yet-packed events. A reused buffer: the drain has to see
    /// every `ParamChanged` id before it can decide which rate partners to
    /// synthesise, so it cannot pack straight out of the channel.
    pending: Vec<ViewEvent>,
    /// Last matrix topology the page was told about (`None` = tell it next
    /// tick). Port of the native shell's `last_matrix`.
    last_matrix: Option<[MatrixTable; 2]>,
    /// Last keyboard state the page was told about. Port of `last_key`.
    last_key: Option<KeyState>,
}

impl ControllerState {
    fn new() -> Box<Self> {
        let model = Arc::new(SharedParams::new());
        let user: Arc<Mutex<UserState>> = Arc::new(Mutex::new(UserState::default()));
        let store = WebPresetStore {
            factory: EnginePresetStore::new(),
            user: user.clone(),
        };
        let (ctrl, view_rx, corpus) = Controller::new(model.clone(), Box::new(store));
        // NOTE: `echo_param_writes` is deliberately left at its default `true`.
        // See the module docs — VXN1b has no dirty bitsets to drain, so the echo
        // is the only Model→View emitter for controller-routed writes.
        // No `host_sender()` here: nothing in a browser originates a param
        // value outside this controller (the worklet only ever applies what it
        // was sent), so the host channel has no producer. The shared Controller
        // still owns one; it simply stays empty.
        let ui_tx = ctrl.ui_sender();
        let mut s = Box::new(Self {
            ctrl,
            model,
            view_rx,
            ui_tx,
            user,
            corpus,
            arg_in: Vec::new(),
            journal_out: Vec::with_capacity(4 * 1024),
            state_buf: Vec::new(),
            toml_buf: Vec::with_capacity(4 * 1024),
            corpus_json: Vec::new(),
            view_out: Vec::with_capacity(8 * 1024),
            values_out: vec![0.0; TOTAL_PARAMS],
            corpus_json_dirty: true,
            emitted: vec![false; TOTAL_PARAMS],
            partner_touched: Vec::with_capacity(32),
            pending: Vec::with_capacity(64),
            last_matrix: None,
            last_key: None,
        });
        // The factory bank is embedded, so the corpus is complete at
        // construction — unlike vxn-1/vxn-2, where it arrives with a fetched
        // asset. Publish it now so JS can read it before the first tick.
        s.flush_corpus_json();
        s
    }

    /// Rebuild the JS-visible corpus JSON from the shared snapshot if dirtied,
    /// then clear the flag. Idempotent — a clean flag is a no-op.
    fn flush_corpus_json(&mut self) {
        if !self.corpus_json_dirty {
            return;
        }
        self.corpus_json_dirty = false;
        let json = self
            .corpus
            .lock()
            .map(|c| corpus_snapshot_json(&c, UNCATEGORISED))
            .unwrap_or_else(|_| "{\"factory\":[],\"user\":[]}".to_string());
        self.corpus_json.clear();
        self.corpus_json.extend_from_slice(json.as_bytes());
    }

    /// Boot hydration finished: refresh the user corpus from the (now-populated)
    /// cache and rebuild the corpus JSON so JS can read it synchronously.
    fn hydrate_done(&mut self) {
        self.ctrl.refresh_user_corpus();
        self.corpus_json_dirty = true;
        self.flush_corpus_json();
    }

    /// Read a UTF-8 string out of the argument staging buffer, clamped to the
    /// buffer so a malformed length can't panic.
    fn arg_string(&self, start: usize, len: usize) -> String {
        let n = self.arg_in.len();
        let s = start.min(n);
        let e = start.saturating_add(len).min(n);
        String::from_utf8_lossy(&self.arg_in[s..e]).into_owned()
    }

    #[inline]
    fn post(&self, ev: UiEvent) {
        let _ = self.ui_tx.try_send(ev);
    }

    #[inline]
    fn post_custom<T: std::any::Any + Send>(&self, c: T) {
        self.post(UiEvent::Custom(Box::new(c)));
    }

    // ── Packing ─────────────────────────────────────────────────────────────

    /// Pack one `ParamChanged`. The `display` the Controller put on the event is
    /// **ignored**: it came from `descriptor.display()`, which is wrong for a
    /// tempo-synced rate (it prints Hz where the readout should show a
    /// subdivision). Recomputing here means every emitter — UI echo, preset
    /// re-broadcast, explicit broadcast — gets the same rule from one place.
    fn pack_param_changed(&mut self, id: usize) {
        let plain = self.model.get(id);
        push_u32(&mut self.view_out, VE_PARAM_CHANGED);
        push_u32(&mut self.view_out, id as u32);
        push_f32(&mut self.view_out, plain);
        push_f32(&mut self.view_out, self.model.get_normalized(id));
        let display = sync_aware_display(&self.model, id, plain);
        push_str(&mut self.view_out, &display);
    }

    /// Pack one non-param `ViewEvent`. Returns `true` if a record was appended;
    /// variants with no web analogue (the native text-input popup) are skipped.
    /// `ParamChanged` is handled by [`Self::pack_param_changed`] instead.
    fn pack_other(&mut self, ev: &ViewEvent) -> bool {
        match ev {
            ViewEvent::PresetLoaded {
                meta,
                source,
                warnings,
            } => {
                push_u32(&mut self.view_out, VE_PRESET_LOADED);
                push_str(&mut self.view_out, &meta.name);
                match source {
                    Some(PresetSource::Factory { index }) => {
                        push_u32(&mut self.view_out, PRESET_SRC_FACTORY);
                        push_u32(&mut self.view_out, *index as u32);
                    }
                    Some(PresetSource::User { path }) => {
                        push_u32(&mut self.view_out, PRESET_SRC_USER);
                        let p = path.to_string_lossy().into_owned();
                        push_str(&mut self.view_out, &p);
                    }
                    None => push_u32(&mut self.view_out, PRESET_SRC_NONE),
                }
                push_u32(&mut self.view_out, warnings.len() as u32);
                for w in warnings {
                    push_str(&mut self.view_out, w);
                }
                true
            }
            ViewEvent::PresetCorpusChanged { follow } => {
                push_u32(&mut self.view_out, VE_CORPUS_CHANGED);
                match follow {
                    Some(path) => {
                        push_u32(&mut self.view_out, 1);
                        let p = path.to_string_lossy().into_owned();
                        push_str(&mut self.view_out, &p);
                    }
                    None => push_u32(&mut self.view_out, 0),
                }
                true
            }
            ViewEvent::Status { line } => {
                push_u32(&mut self.view_out, VE_STATUS);
                push_str(&mut self.view_out, line);
                true
            }
            // Text-input ViewEvents are the native popup's channel; the page
            // prompts in-DOM.
            _ => false,
        }
    }

    /// Pack a whole-table matrix snapshot: both layers, 16 slots each. Depths
    /// stay out — they are CLAP params and ride `ParamChanged`, the same split
    /// the native echo uses.
    fn pack_matrix(&mut self, layers: &[MatrixTable; 2]) {
        push_u32(&mut self.view_out, VE_MATRIX_SNAPSHOT);
        for table in layers.iter() {
            for slot in table.slots.iter() {
                self.view_out.push(slot.source as u8);
                self.view_out.push(slot.dest as u8);
                self.view_out.push(slot.curve as u8);
                self.view_out.push(slot.scale_src as u8);
            }
        }
    }

    fn pack_key(&mut self, key: &KeyState) {
        push_u32(&mut self.view_out, VE_KEY_STATE);
        self.view_out.push(key.key_mode() as u8);
        self.view_out.push(key.split_point);
        self.view_out.push(key.lfo2_link as u8);
    }

    // ── Echoes ──────────────────────────────────────────────────────────────

    /// Push the matrix topology when it differs from what the page was last
    /// told. Port of the native shell's `push_matrix_echo` (0247): topology is
    /// not a CLAP param, so a preset load, a state restore or a layer copy moves
    /// it with nothing in the param machinery to report it.
    fn push_matrix_echo(&mut self) -> bool {
        let live = self.model.matrix_snapshot();
        if self.last_matrix == Some(live) {
            return false;
        }
        self.last_matrix = Some(live);
        self.pack_matrix(&live);
        true
    }

    /// The same, one type over, for the keyboard record (0221): key mode, split
    /// point and the LFO 2 link.
    ///
    /// Reads `key_state()` rather than `take_key_state()`. Natively that matters
    /// because the *audio thread* owns the dirty flag; here the worklet is a
    /// separate wasm fed by ring events and nothing else consumes it — but the
    /// memo is kept anyway so the two shells stay one idiom, and so this file
    /// does not silently depend on which side clears a flag.
    fn push_key_echo(&mut self) -> bool {
        let live = self.model.key_state();
        if self.last_key == Some(live) {
            return false;
        }
        self.last_key = Some(live);
        self.pack_key(&live);
        true
    }

    // ── Tick ────────────────────────────────────────────────────────────────

    /// Drive one controller tick: drain the UI/host queues into the model, then
    /// pack every resulting `ViewEvent` — plus the rate-partner refresh and the
    /// two non-param echoes — into `view_out`. Finally refresh the JS-visible
    /// value snapshot so a mirror pass sees this tick's writes.
    fn tick(&mut self) {
        self.ctrl.tick(
            &mut apply_custom_ui,
            // No host events originate in a browser; the channel exists because
            // the shared Controller has one.
            &mut |_, _| {},
            // The post-load hook is unused: the matrix / key echoes below run
            // every tick and catch a load's non-param drift on their own.
            &mut |_| {},
        );

        // A re-attached page has no state; re-seed the non-param echoes so the
        // next push is unconditional. Params re-broadcast via `EditorReady`.
        if self.ctrl.take_editor_ready_flag() {
            self.last_matrix = None;
            self.last_key = None;
        }

        self.view_out.clear();
        push_u32(&mut self.view_out, 0); // count placeholder
        let mut count = 0u32;

        // (1) Drain the channel first — the partner refresh has to know every
        // ParamChanged id in this batch before it can decide what to synthesise.
        while let Ok(ev) = self.view_rx.try_recv() {
            if matches!(ev, ViewEvent::PresetCorpusChanged { .. }) {
                self.corpus_json_dirty = true;
            }
            self.pending.push(ev);
        }

        // (2) Pack, recording which param ids were emitted.
        for i in 0..self.pending.len() {
            match &self.pending[i] {
                ViewEvent::ParamChanged { id, .. } => {
                    let id = id.raw();
                    if id >= TOTAL_PARAMS || self.emitted[id] {
                        continue;
                    }
                    self.emitted[id] = true;
                    self.partner_touched.push(id);
                    self.pack_param_changed(id);
                    count += 1;
                }
                _ => {
                    // Clone out of `pending` to satisfy the borrow checker —
                    // non-param events are a handful per tick, never a burst.
                    let ev = std::mem::replace(&mut self.pending[i], ViewEvent::Status {
                        line: String::new(),
                    });
                    if self.pack_other(&ev) {
                        count += 1;
                    }
                }
            }
        }
        self.pending.clear();

        // (3) Rate-partner refresh: a sync toggle's flip does not change its
        // rate param's value, but it swaps the readout between Hz/seconds and a
        // subdivision label, and the faceplate repaints only from what it is
        // sent. Same rule as the native shell's `push_param_diffs`.
        for i in 0..self.partner_touched.len() {
            let id = self.partner_touched[i];
            let Some(rate_id) = rate_partner_clap_id(id) else {
                continue;
            };
            if rate_id >= TOTAL_PARAMS || self.emitted[rate_id] {
                continue;
            }
            self.emitted[rate_id] = true;
            self.partner_touched.push(rate_id);
            self.pack_param_changed(rate_id);
            count += 1;
        }
        for id in self.partner_touched.drain(..) {
            self.emitted[id] = false;
        }

        // (4) The two non-param echoes.
        if self.push_matrix_echo() {
            count += 1;
        }
        if self.push_key_echo() {
            count += 1;
        }

        self.view_out[0..4].copy_from_slice(&count.to_le_bytes());

        // Rebuild the browser-facing corpus JSON at most once if this tick
        // dirtied it, so the bridge (which just saw VE_CORPUS_CHANGED) can
        // republish synchronously.
        self.flush_corpus_json();

        for id in 0..TOTAL_PARAMS {
            self.values_out[id] = self.model.get(id);
        }
    }

    // ── Journal ─────────────────────────────────────────────────────────────

    /// Drain the user store's pending persistence ops into `journal_out` (the
    /// packed wire the bridge ships to IndexedDB) and return its byte length.
    /// Wire: `u32 op_count`, then per op `u32 tag` + payload — PUT(1): str key,
    /// u32 blob_len + blob; DELETE(2): str key; PUT_FOLDER(3): str name;
    /// DELETE_FOLDER(4): str name.
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

    // ── State + TOML ────────────────────────────────────────────────────────

    /// Snapshot the full patch state into `state_buf`, returning its length. The
    /// blob is the model's canonical `PluginState` format (both layers' params +
    /// matrices + the keyboard record) — the host-state analogue used for
    /// autosave and the share link.
    fn snapshot_state(&mut self) -> u32 {
        let blob = ParamModel::snapshot_bytes(&*self.model);
        self.state_buf.clear();
        self.state_buf.extend_from_slice(&blob);
        self.state_buf.len() as u32
    }

    /// Restore the model from the `len`-byte blob staged in `state_buf`. Returns
    /// 1 on success, 0 on a malformed blob (the model is left untouched).
    ///
    /// `restore_from_bytes` writes the model directly, so it raises `reload` for
    /// the audio thread but produces no `ParamChanged` — hence the explicit
    /// re-broadcast. The matrix / key echoes catch the non-param half.
    fn restore_state(&mut self, len: usize) -> u32 {
        let n = len.min(self.state_buf.len());
        match ParamModel::restore_from_bytes(&*self.model, &self.state_buf[..n]) {
            Ok(()) => {
                self.ctrl.broadcast_all_params();
                1
            }
            Err(_) => 0,
        }
    }

    /// Serialise the current patch to sparse TOML into `toml_buf`, returning its
    /// length. `name` is staged in `arg_in`.
    fn export_toml(&mut self, name_len: usize) -> u32 {
        let name = self.arg_string(0, name_len);
        let meta = PresetMeta {
            name,
            ..Default::default()
        };
        let blob = ParamModel::snapshot_bytes(&*self.model);
        self.toml_buf.clear();
        match user_store::encode_record(&meta, &blob) {
            Ok(bytes) => {
                self.toml_buf.extend_from_slice(&bytes);
                self.toml_buf.len() as u32
            }
            // A live snapshot is always a valid PluginState, so this cannot fail
            // in practice; emit an empty buffer rather than panic.
            Err(_) => 0,
        }
    }

    /// Parse the `len`-byte TOML staged in `toml_buf` and apply it. Returns 1 on
    /// success, 0 on a malformed / wrong-schema file (model left untouched).
    /// Re-broadcasts for the same reason [`Self::restore_state`] does.
    fn import_toml(&mut self, len: usize) -> u32 {
        let n = len.min(self.toml_buf.len());
        let bytes = self.toml_buf[..n].to_vec();
        let Ok((_meta, blob, _warnings)) = decode_record(&bytes) else {
            return 0;
        };
        match ParamModel::restore_from_bytes(&*self.model, &blob) {
            Ok(()) => {
                self.ctrl.broadcast_all_params();
                1
            }
            Err(_) => 0,
        }
    }
}

// ── Global instance + C-ABI opcode surface ───────────────────────────────────

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

/// Total addressable CLAP param count (the flat two-layer id space).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_total_params() -> u32 {
    TOTAL_PARAMS as u32
}

/// Per-layer patch param count — the JS side needs the layer split back to map
/// a flat id onto (layer, inner) (vxn-2 dropped this; VXN1b is two-layer).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_patch_count() -> u32 {
    vxn1b_engine::PATCH_COUNT as u32
}

// UiEvent hot path (1:1 with UiEvent variants).

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

/// `UiEvent::EditorReady` — re-broadcast the whole table so a freshly-opened
/// page seeds itself, and re-arm the matrix / key echoes.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_editor_ready() {
    state().post(UiEvent::EditorReady);
}

// VXN1b custom opcodes.
//
// Three of these are "both" ops: the model owns them for state + UI echo, and
// the *engine* needs them too — and it is a separate wasm, so the bridge (0291)
// also puts them on the event ring. Native gets that free because one
// `SharedParams` is visible to both threads. `copy_layer` is controller-only:
// its results reach the engine as ordinary param writes and matrix edits. The
// scope tap and the tempo are ring-only and have no opcode here at all.

/// `KeyOp::SetKeyMode` — 0 Single / 1 Dual / 2 Split. **Also goes on the ring.**
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_set_key_mode(mode: u32) {
    state().post_custom(KeyOp::SetKeyMode(mode as u8));
}

/// `KeyOp::SetSplitPoint` — MIDI note. **Also goes on the ring.**
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_set_split_point(note: u32) {
    state().post_custom(KeyOp::SetSplitPoint(note as u8));
}

/// `KeyOp::SetLfo2Link` — cross-layer LFO 2 link. **Also goes on the ring.**
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_set_lfo2_link(on: u32) {
    state().post_custom(KeyOp::SetLfo2Link(on != 0));
}

/// `MatrixEdit` — one topology field of one slot of one layer. Depth is a normal
/// CLAP param and rides `vxnc_ui_set_param`. `layer`: 0 = L1 (upper), 1 = L2
/// (lower). `field`: 0 source, 1 dest, 2 curve, 3 scale. **Also goes on the
/// ring.** An unknown `field` is dropped rather than guessed.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_set_matrix(layer: u32, slot: u32, field: u32, value: u32) {
    let Some(field) = matrix_field_from_wire(field) else {
        return;
    };
    state().post_custom(MatrixEdit {
        layer: if layer == 0 { Layer::L1 } else { Layer::L2 },
        slot: slot as u8,
        field,
        value: value as u8,
    });
}

/// Decode the wire `field` selector of [`vxnc_ui_set_matrix`]. `None` for an
/// unknown value, which the opcode drops rather than guessing at — a bad
/// selector must not silently rewrite the wrong field.
fn matrix_field_from_wire(field: u32) -> Option<MatrixField> {
    Some(match field {
        0 => MatrixField::Source,
        1 => MatrixField::Dest,
        2 => MatrixField::Curve,
        3 => MatrixField::ScaleSrc,
        _ => return None,
    })
}

/// `PatchOp::CopyLayer` — duplicate one layer's patch + topology onto the other.
/// Controller-only: the engine sees the result as ordinary param writes.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_copy_layer(from: u32, to: u32) {
    let side = |v: u32| if v == 0 { Layer::L1 } else { Layer::L2 };
    state().post_custom(PatchOp::CopyLayer {
        from: side(from),
        to: side(to),
    });
}

// Presets — factory bank is embedded, so there is no load-the-asset opcode.

/// Pointer to the browser corpus JSON (valid until the next tick).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_corpus_json_ptr() -> *const u8 {
    state().corpus_json.as_ptr()
}

/// Byte length of the browser corpus JSON.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_corpus_json_len() -> u32 {
    state().corpus_json.len() as u32
}

/// Number of embedded factory presets.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_factory_len() -> u32 {
    state().ctrl.preset_store().factory_len() as u32
}

/// Load factory preset `index`: the model restore + full param re-broadcast +
/// `PresetLoaded` land on the next tick.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_load_factory(index: u32) {
    state().post(UiEvent::LoadPreset {
        source: PresetSource::Factory {
            index: index as usize,
        },
    });
}

/// Step to the previous/next preset in the corpus (delta ±1).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_step_preset(delta: i32) {
    state().post(UiEvent::StepPreset { delta });
}

// User presets + persistence.
//
// String/blob args ride the shared `arg_in` staging buffer: JS reserves it via
// `vxnc_arg_buf_reserve`, writes the concatenated arguments, then calls the
// opcode with each argument's byte length. `ARG_NONE` in a length slot means an
// absent optional argument (root folder / no destination).

/// Reserve `len` bytes in the argument staging buffer and return its pointer.
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
        source: PresetSource::User {
            path: PathBuf::from(path),
        },
    });
}

/// `UiEvent::RenamePreset` — args: path (`path_len`), then new name (`name_len`).
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

/// `UiEvent::DeletePreset` — arg: preset path.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_delete_preset(path_len: u32) {
    let s = state();
    let path = s.arg_string(0, path_len as usize);
    s.post(UiEvent::DeletePreset {
        path: PathBuf::from(path),
    });
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
    s.post(UiEvent::MovePreset {
        path: PathBuf::from(path),
        dest_folder,
    });
}

/// `UiEvent::NewFolder` — arg: suggested folder name (the store uniquifies).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_ui_new_folder(suggested_len: u32) {
    let s = state();
    let suggested = s.arg_string(0, suggested_len as usize);
    s.post(UiEvent::NewFolder { suggested });
}

/// `UiEvent::RenameFolder` — args: old name (`old_len`), then new (`new_len`).
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
// controller goes live, WITHOUT journalling (it is already stored).

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
/// TOML record (`rec_len`). Returns 1 on success, 0 if the record fails to parse
/// (a corrupt / foreign entry is skipped rather than aborting hydration).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_hydrate_preset(key_len: u32, rec_len: u32) -> u32 {
    let s = state();
    let key = s.arg_string(0, key_len as usize);
    let start = (key_len as usize).min(s.arg_in.len());
    let end = (key_len as usize)
        .saturating_add(rec_len as usize)
        .min(s.arg_in.len());
    let rec = s.arg_in[start..end].to_vec();
    match decode_record(&rec) {
        Ok((meta, blob, warnings)) => {
            if let Ok(mut u) = s.user.lock() {
                u.hydrate_preset(&key, meta, blob, warnings);
            }
            1
        }
        Err(_) => 0,
    }
}

/// Finish hydration: refresh the user corpus from the cache + rebuild the corpus
/// JSON (JS reads it synchronously after this).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_hydrate_done() {
    state().hydrate_done();
}

// Deferred-write journal — drained off the tick and shipped to IndexedDB.

/// Drain the user store's pending persistence ops into the packed journal
/// buffer; returns its byte length.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_take_journal() -> u32 {
    state().take_journal()
}

/// Pointer to the packed journal buffer (valid until the next `vxnc_take_journal`).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_journal_out_ptr() -> *const u8 {
    state().journal_out.as_ptr()
}

// Full patch-state snapshot / restore (autosave + share link).

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

/// Reserve `len` bytes in the state scratch buffer and return its pointer.
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

/// Serialise the current patch to sparse TOML into the TOML scratch buffer;
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

/// Reserve `len` bytes in the TOML scratch buffer and return its pointer.
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_toml_buf_reserve(len: u32) -> *mut u8 {
    let s = state();
    s.toml_buf.clear();
    s.toml_buf.resize(len as usize, 0);
    s.toml_buf.as_mut_ptr()
}

/// Parse the `len`-byte TOML staged in the TOML scratch buffer and apply it.
/// Returns 1 on success, 0 if malformed (model left untouched).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_import_toml(len: u32) -> u32 {
    state().import_toml(len as usize)
}

// Tick + drains.

/// Drive one controller tick: drain the UI/host queues into the model and pack
/// the resulting ViewEvents into the drain buffer.
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
/// SAB — the bulk path a preset load needs (181 values at once).
#[unsafe(no_mangle)]
pub extern "C" fn vxnc_values_ptr() -> *const f32 {
    state().values_out.as_ptr()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vxn1b_engine::params::{ParamId as P, clap_id_of};
    use vxn1b_engine::params::MATRIX_SLOTS;
    use vxn1b_engine::{Curve, DestId, KeyMode, PATCH_PARAMS, ScopeOp, ScopeTap, SourceId};

    fn fresh() -> Box<ControllerState> {
        ControllerState::new()
    }

    // ── Wire decoder ────────────────────────────────────────────────────────
    //
    // Decodes the drain buffer the JS bridge will decode, so a packing change
    // that JS could not read fails here first.

    #[derive(Debug, Clone, PartialEq)]
    enum Rec {
        Param {
            id: usize,
            plain: f32,
            norm: f32,
            display: String,
        },
        Matrix(Vec<[u8; 4]>),
        Key {
            mode: u8,
            split: u8,
            link: u8,
        },
        PresetLoaded {
            name: String,
            src: u32,
            warnings: Vec<String>,
        },
        Corpus(Option<String>),
        Status(String),
    }

    struct Cur<'a> {
        b: &'a [u8],
        p: usize,
    }
    impl<'a> Cur<'a> {
        fn u32(&mut self) -> u32 {
            let v = u32::from_le_bytes(self.b[self.p..self.p + 4].try_into().unwrap());
            self.p += 4;
            v
        }
        fn f32(&mut self) -> f32 {
            let v = f32::from_le_bytes(self.b[self.p..self.p + 4].try_into().unwrap());
            self.p += 4;
            v
        }
        fn u8(&mut self) -> u8 {
            let v = self.b[self.p];
            self.p += 1;
            v
        }
        fn s(&mut self) -> String {
            let n = self.u32() as usize;
            let v = String::from_utf8(self.b[self.p..self.p + n].to_vec()).unwrap();
            self.p += n;
            v
        }
    }

    fn decode(buf: &[u8]) -> Vec<Rec> {
        let mut c = Cur { b: buf, p: 0 };
        let count = c.u32();
        let mut out = Vec::new();
        for _ in 0..count {
            let tag = c.u32();
            out.push(match tag {
                VE_PARAM_CHANGED => Rec::Param {
                    id: c.u32() as usize,
                    plain: c.f32(),
                    norm: c.f32(),
                    display: c.s(),
                },
                VE_MATRIX_SNAPSHOT => {
                    let mut slots = Vec::new();
                    for _ in 0..(2 * MATRIX_SLOTS) {
                        slots.push([c.u8(), c.u8(), c.u8(), c.u8()]);
                    }
                    Rec::Matrix(slots)
                }
                VE_KEY_STATE => Rec::Key {
                    mode: c.u8(),
                    split: c.u8(),
                    link: c.u8(),
                },
                VE_PRESET_LOADED => {
                    let name = c.s();
                    let src = c.u32();
                    match src {
                        PRESET_SRC_FACTORY => {
                            c.u32();
                        }
                        PRESET_SRC_USER => {
                            c.s();
                        }
                        _ => {}
                    }
                    let n = c.u32();
                    let warnings = (0..n).map(|_| c.s()).collect();
                    Rec::PresetLoaded {
                        name,
                        src,
                        warnings,
                    }
                }
                VE_CORPUS_CHANGED => {
                    let has = c.u32();
                    Rec::Corpus(if has == 1 { Some(c.s()) } else { None })
                }
                VE_STATUS => Rec::Status(c.s()),
                other => panic!("unknown view tag {other}"),
            });
        }
        // The decoder must consume the whole buffer: a length field that lies is
        // exactly the bug this catches on the JS side.
        assert_eq!(c.p, buf.len(), "decoder did not consume the whole batch");
        out
    }

    fn params(recs: &[Rec]) -> Vec<usize> {
        recs.iter()
            .filter_map(|r| match r {
                Rec::Param { id, .. } => Some(*id),
                _ => None,
            })
            .collect()
    }

    fn display_of(recs: &[Rec], want: usize) -> Option<String> {
        recs.iter().find_map(|r| match r {
            Rec::Param { id, display, .. } if *id == want => Some(display.clone()),
            _ => None,
        })
    }

    fn key_rec(recs: &[Rec]) -> Option<(u8, u8, u8)> {
        recs.iter().find_map(|r| match r {
            Rec::Key { mode, split, link } => Some((*mode, *split, *link)),
            _ => None,
        })
    }

    fn matrix_rec(recs: &[Rec]) -> Option<Vec<[u8; 4]>> {
        recs.iter().find_map(|r| match r {
            Rec::Matrix(s) => Some(s.clone()),
            _ => None,
        })
    }

    // ── Surface ─────────────────────────────────────────────────────────────

    #[test]
    fn total_params_agrees_with_the_engine() {
        assert_eq!(vxnc_total_params() as usize, TOTAL_PARAMS);
        assert_eq!(vxnc_patch_count() as usize, vxn1b_engine::PATCH_COUNT);
        assert_eq!(TOTAL_PARAMS, 2 * vxn1b_engine::PATCH_COUNT + vxn1b_engine::GLOBAL_PARAMS.len());
    }

    /// The norm path is `from_fader`, not `from_normalized`: the descriptor's
    /// taper is part of the calibration (0243). Proven on Cutoff, which is
    /// `Exp { mid: 800 }` — a linear midpoint would land near 8 kHz.
    #[test]
    fn norm_path_applies_the_descriptor_taper() {
        let mut s = fresh();
        let id = clap_id_of(Layer::L1, P::Cutoff);
        let desc = vxn1b_engine::desc_for_clap_id(id).unwrap();
        let linear_mid = desc.from_normalized(0.5);

        s.post(UiEvent::SetParamNorm {
            id: ParamId::new(id),
            norm: 0.5,
        });
        s.tick();
        let got = s.model.get(id);
        assert!(
            (got - linear_mid).abs() > linear_mid * 0.25,
            "norm path looks linear: got {got}, linear midpoint {linear_mid}"
        );
        // Round-trips through the same taper.
        assert!((s.model.get_normalized(id) - 0.5).abs() < 1e-3);

        // …and the plain path is untapered.
        s.post(UiEvent::SetParam {
            id: ParamId::new(id),
            plain: 1000.0,
        });
        s.tick();
        assert!((s.model.get(id) - 1000.0).abs() < 1e-3);
    }

    /// Echo is left at its default `true` — the only Model→View emitter here.
    /// Exactly one record: not zero (vxn-2's echo-off + bitset-drain setup,
    /// which has no bits to drain on this model) and not two.
    #[test]
    fn set_param_surfaces_exactly_one_param_changed() {
        let mut s = fresh();
        s.tick(); // clear the boot matrix/key echo
        let id = clap_id_of(Layer::L1, P::Cutoff);
        s.post(UiEvent::SetParam {
            id: ParamId::new(id),
            plain: 900.0,
        });
        s.tick();
        let recs = decode(&s.view_out);
        assert_eq!(params(&recs), vec![id], "expected exactly one ParamChanged");
    }

    /// `norm` and `display` are the two fields `param-store.mjs` stubs today;
    /// both must be descriptor-derived and survive the wire.
    #[test]
    fn view_batch_round_trips_norm_and_display() {
        let mut s = fresh();
        s.tick();
        let id = clap_id_of(Layer::L1, P::Cutoff);
        s.post(UiEvent::SetParam {
            id: ParamId::new(id),
            plain: 1234.0,
        });
        s.tick();
        let recs = decode(&s.view_out);
        let Rec::Param {
            plain,
            norm,
            display,
            ..
        } = recs.iter().find(|r| matches!(r, Rec::Param { .. })).unwrap().clone()
        else {
            unreachable!()
        };
        assert!((plain - 1234.0).abs() < 1e-3);
        assert!((norm - s.model.get_normalized(id)).abs() < 1e-6);
        assert!(norm > 0.0 && norm < 1.0);
        assert!(!display.is_empty());
        assert_ne!(display, format!("{plain}"), "display is not a raw stringify");
    }

    /// A sync toggle's flip does not move its rate param's value, but it swaps
    /// the readout between Hz and a subdivision label — and the faceplate
    /// repaints only from what it is sent.
    #[test]
    fn sync_flip_refreshes_its_rate_partner() {
        let mut s = fresh();
        s.tick();
        let sync = clap_id_of(Layer::L1, P::Lfo1Sync);
        let rate = clap_id_of(Layer::L1, P::Lfo1Rate);
        let free_label = sync_aware_display(&s.model, rate, s.model.get(rate));
        let rate_before = s.model.get(rate);

        s.post(UiEvent::SetParam {
            id: ParamId::new(sync),
            plain: 1.0,
        });
        s.tick();
        let recs = decode(&s.view_out);
        let ids = params(&recs);
        assert!(ids.contains(&sync), "the toggle itself did not surface");
        assert!(
            ids.contains(&rate),
            "sync flip did not re-push its rate partner: {ids:?}"
        );
        let synced_label = display_of(&recs, rate).unwrap();
        assert_ne!(
            synced_label, free_label,
            "the partner's display did not switch to a subdivision label"
        );
        // The partner's *value* is unchanged — only its label. This is exactly
        // why a plain value diff would miss it.
        assert_eq!(s.model.get(rate), rate_before);
    }

    /// The two non-param echoes seed on the first tick (the page has been told
    /// nothing yet) and then go quiet.
    #[test]
    fn first_tick_seeds_the_non_param_echoes_then_quiesces() {
        let mut s = fresh();
        s.tick();
        let recs = decode(&s.view_out);
        assert!(matrix_rec(&recs).is_some(), "no matrix seed on first tick");
        assert!(key_rec(&recs).is_some(), "no key seed on first tick");
        assert!(params(&recs).is_empty(), "nothing set a param yet");

        s.tick();
        assert!(decode(&s.view_out).is_empty(), "second tick should be silent");
    }

    /// The editor-attach path: a fresh page needs every param plus the
    /// non-automatable state, since it has no memory of either.
    #[test]
    fn editor_ready_rebroadcasts_params_and_non_param_state() {
        let mut s = fresh();
        s.tick(); // consume the boot seed so the re-seed is provably the attach
        s.post(UiEvent::EditorReady);
        s.tick();
        let recs = decode(&s.view_out);
        let mut ids = params(&recs);
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), TOTAL_PARAMS, "attach did not re-broadcast the table");
        assert!(matrix_rec(&recs).is_some(), "attach did not re-seed the matrix");
        assert!(key_rec(&recs).is_some(), "attach did not re-seed the key state");
    }

    // ── Custom opcodes: controller-side vs ring-only ─────────────────────────

    /// Key ops reach the model and echo. These are "both" ops — the bridge also
    /// puts them on the ring for the engine wasm (0291); this is the model half.
    #[test]
    fn key_ops_reach_the_model_and_echo() {
        let mut s = fresh();
        s.tick();
        s.post_custom(KeyOp::SetKeyMode(2)); // Split
        s.post_custom(KeyOp::SetSplitPoint(48));
        s.post_custom(KeyOp::SetLfo2Link(true));
        s.tick();
        let key = s.model.key_state();
        assert_eq!(key.key_mode(), KeyMode::Split);
        assert_eq!(key.split_point, 48);
        assert!(key.lfo2_link);
        assert_eq!(
            key_rec(&decode(&s.view_out)),
            Some((KeyMode::Split as u8, 48, 1)),
            "the key echo did not carry the new state"
        );
    }

    /// Matrix topology likewise — and the echo carries the whole table, both
    /// layers, with depths deliberately absent (they are params).
    #[test]
    fn matrix_edit_reaches_the_model_and_echoes() {
        let mut s = fresh();
        s.tick();
        s.post_custom(MatrixEdit {
            layer: Layer::L2,
            slot: 3,
            field: MatrixField::Dest,
            value: DestId::Cutoff as u8,
        });
        s.tick();
        let live = s.model.matrix_snapshot();
        assert_eq!(live[1].slots[3].dest, DestId::Cutoff);
        let slots = matrix_rec(&decode(&s.view_out)).expect("no matrix echo");
        assert_eq!(slots.len(), 2 * MATRIX_SLOTS);
        // Layer 2 slot 3 lives at offset MATRIX_SLOTS + 3; byte 1 is `dest`.
        assert_eq!(slots[MATRIX_SLOTS + 3][1], DestId::Cutoff as u8);
    }

    /// An unknown matrix field is dropped, not guessed at — a bad selector must
    /// never silently rewrite a different field of the slot.
    #[test]
    fn unknown_matrix_field_is_dropped() {
        assert_eq!(matrix_field_from_wire(0), Some(MatrixField::Source));
        assert_eq!(matrix_field_from_wire(1), Some(MatrixField::Dest));
        assert_eq!(matrix_field_from_wire(2), Some(MatrixField::Curve));
        assert_eq!(matrix_field_from_wire(3), Some(MatrixField::ScaleSrc));
        assert_eq!(matrix_field_from_wire(4), None);
        assert_eq!(matrix_field_from_wire(u32::MAX), None);
    }

    /// The scope tap is **ring-only**: pure audio-thread view state, so it must
    /// never touch the patch, the state blob or the corpus. There is no
    /// controller opcode for it; if one is ever added by accident, the custom
    /// handler still ignores the payload.
    #[test]
    fn scope_op_is_ring_only_and_never_reaches_the_model() {
        let mut s = fresh();
        s.tick();
        let before = ParamModel::snapshot_bytes(&*s.model);
        s.post_custom(ScopeOp::SetTap(ScopeTap::Layer2));
        s.tick();
        assert_eq!(
            ParamModel::snapshot_bytes(&*s.model),
            before,
            "a scope-tap op mutated the patch"
        );
        assert!(
            decode(&s.view_out).is_empty(),
            "a scope-tap op produced view events"
        );
    }

    /// `copy_layer` duplicates the patch and the topology, leaves the mixer
    /// strip alone, and — because it writes the model directly rather than
    /// through the Controller — must still re-broadcast.
    #[test]
    fn copy_layer_duplicates_patch_and_topology_but_not_the_mixer_strip() {
        let mut s = fresh();
        s.tick();

        // Make Layer 1 distinctive, in both a param and its topology.
        let cutoff1 = clap_id_of(Layer::L1, P::Cutoff);
        let cutoff2 = clap_id_of(Layer::L2, P::Cutoff);
        s.post(UiEvent::SetParam {
            id: ParamId::new(cutoff1),
            plain: 900.0,
        });
        s.post_custom(MatrixEdit {
            layer: Layer::L1,
            slot: 5,
            field: MatrixField::Source,
            value: SourceId::Lfo2 as u8,
        });
        s.tick();

        // Give the two layers different levels so the exclusion is observable.
        let level1 = clap_id_of(Layer::L1, P::LayerLevel);
        let level2 = clap_id_of(Layer::L2, P::LayerLevel);
        s.model.set(level1, 0.9);
        s.model.set(level2, 0.2);

        s.post_custom(PatchOp::CopyLayer {
            from: Layer::L1,
            to: Layer::L2,
        });
        s.tick();

        assert!(
            (s.model.get(cutoff2) - 900.0).abs() < 1.0,
            "the patch param was not copied"
        );
        assert_eq!(
            s.model.matrix_snapshot()[1].slots[5].source,
            SourceId::Lfo2,
            "the topology was not copied"
        );
        assert!(
            (s.model.get(level2) - 0.2).abs() < 1e-6,
            "the mixer strip was copied but must be left alone"
        );

        // The copy writes the model behind the Controller's back, so without an
        // explicit re-broadcast the page would show the old Layer 2 patch.
        let recs = decode(&s.view_out);
        let mut ids = params(&recs);
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(
            ids.len(),
            TOTAL_PARAMS,
            "copy_layer did not re-broadcast the table"
        );
        assert!(
            matrix_rec(&recs).is_some(),
            "copy_layer did not echo the new topology"
        );
    }

    // ── Presets ─────────────────────────────────────────────────────────────

    /// The factory bank is embedded (`include_dir!`), not fetched: no
    /// `factory.bin`, no load opcode. It is therefore complete before the first
    /// tick, so the corpus JSON is readable at boot.
    #[test]
    fn factory_bank_is_embedded_and_listed_in_the_corpus() {
        let s = fresh();
        let n = s.ctrl.preset_store().factory_len();
        assert!(n > 0, "no embedded factory presets");
        assert_eq!(vxn1b_engine::preset_io::EnginePresetStore::new().factory_len(), n);
        let json = String::from_utf8(s.corpus_json.clone()).unwrap();
        assert!(json.contains("factory"), "corpus json missing factory group: {json}");
        let first = s.ctrl.preset_store().factory_meta(0).unwrap();
        assert!(
            json.contains(&first.name),
            "corpus json does not list factory preset {:?}",
            first.name
        );
    }

    #[test]
    fn factory_load_rebroadcasts_and_reports() {
        let mut s = fresh();
        s.tick();
        s.post(UiEvent::LoadPreset {
            source: PresetSource::Factory { index: 0 },
        });
        s.tick();
        let recs = decode(&s.view_out);
        let loaded = recs.iter().find_map(|r| match r {
            Rec::PresetLoaded { name, src, .. } => Some((name.clone(), *src)),
            _ => None,
        });
        let (name, src) = loaded.expect("factory load produced no PresetLoaded");
        assert_eq!(src, PRESET_SRC_FACTORY);
        assert!(!name.is_empty());
        let mut ids = params(&recs);
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), TOTAL_PARAMS, "factory load did not re-broadcast");
    }

    /// Stage `parts` (concatenated) into the arg buffer, as JS does before an op.
    fn stage(s: &mut ControllerState, parts: &[&[u8]]) {
        s.arg_in.clear();
        for p in parts {
            s.arg_in.extend_from_slice(p);
        }
    }

    fn journal_tags(buf: &[u8]) -> Vec<(u32, String)> {
        let mut c = Cur { b: buf, p: 0 };
        let n = c.u32();
        let mut out = Vec::new();
        for _ in 0..n {
            let tag = c.u32();
            let key = c.s();
            if tag == JW_PUT {
                let len = c.u32() as usize;
                c.p += len;
            }
            out.push((tag, key));
        }
        assert_eq!(c.p, buf.len(), "journal decoder did not consume the batch");
        out
    }

    #[test]
    fn user_save_journals_and_republishes_the_corpus() {
        let mut s = fresh();
        s.tick();
        stage(&mut s, &[b"My Patch", b"Leads"]);
        s.post(UiEvent::SavePreset {
            name: "My Patch".into(),
            folder: Some("Leads".into()),
        });
        s.tick();

        let recs = decode(&s.view_out);
        assert!(
            recs.iter().any(|r| matches!(r, Rec::Corpus(_))),
            "save did not announce a corpus change"
        );
        let json = String::from_utf8(s.corpus_json.clone()).unwrap();
        assert!(json.contains("My Patch"), "corpus json missing the saved preset");

        let n = s.take_journal() as usize;
        let ops = journal_tags(&s.journal_out[..n]);
        assert!(ops.iter().any(|(t, k)| *t == JW_PUT_FOLDER && k == "Leads"));
        assert!(ops.iter().any(|(t, k)| *t == JW_PUT && k == "Leads/My Patch.toml"));
        assert_eq!(s.take_journal(), 4, "journal did not drain (count-only header)");
    }

    #[test]
    fn user_save_load_rename_move_delete_round_trip() {
        let mut s = fresh();
        s.tick();
        let cutoff = clap_id_of(Layer::L1, P::Cutoff);
        s.post(UiEvent::SetParam {
            id: ParamId::new(cutoff),
            plain: 777.0,
        });
        s.post(UiEvent::SavePreset {
            name: "A".into(),
            folder: None,
        });
        s.tick();

        // Move it off the value, then load the preset back.
        s.post(UiEvent::SetParam {
            id: ParamId::new(cutoff),
            plain: 200.0,
        });
        s.tick();
        s.post(UiEvent::LoadPreset {
            source: PresetSource::User {
                path: PathBuf::from("A.toml"),
            },
        });
        s.tick();
        assert!((s.model.get(cutoff) - 777.0).abs() < 1.0, "user preset did not reload");

        s.post(UiEvent::RenamePreset {
            path: PathBuf::from("A.toml"),
            new_name: "B".into(),
        });
        s.post(UiEvent::MovePreset {
            path: PathBuf::from("B.toml"),
            dest_folder: Some("Pads".into()),
        });
        s.tick();
        let json = String::from_utf8(s.corpus_json.clone()).unwrap();
        assert!(json.contains("Pads"), "moved preset's folder missing: {json}");

        s.post(UiEvent::DeletePreset {
            path: PathBuf::from("Pads/B.toml"),
        });
        s.tick();
        assert!(
            s.ctrl
                .preset_store()
                .user_load(&PathBuf::from("Pads/B.toml"))
                .is_err(),
            "deleted preset still loads"
        );
    }

    #[test]
    fn folder_ops_mutate_the_cache_and_journal() {
        let mut s = fresh();
        s.tick();
        s.post(UiEvent::NewFolder {
            suggested: "Keys".into(),
        });
        s.tick();
        s.post(UiEvent::RenameFolder {
            old_name: "Keys".into(),
            new_name: "Bells".into(),
        });
        s.tick();
        let tree = s.ctrl.preset_store().list_user_tree();
        assert!(tree.iter().any(|f| f.name.as_deref() == Some("Bells")));
        s.post(UiEvent::DeleteFolder {
            name: "Bells".into(),
        });
        s.tick();
        let tree = s.ctrl.preset_store().list_user_tree();
        assert!(tree.iter().all(|f| f.name.as_deref() != Some("Bells")));

        let n = s.take_journal() as usize;
        let ops = journal_tags(&s.journal_out[..n]);
        assert!(ops.iter().any(|(t, k)| *t == JW_PUT_FOLDER && k == "Keys"));
        assert!(ops.iter().any(|(t, k)| *t == JW_DELETE_FOLDER && k == "Bells"));
    }

    #[test]
    fn hydrate_seeds_the_cache_without_journalling() {
        // Save in one instance to get a real stored record…
        let mut a = fresh();
        a.tick();
        a.post(UiEvent::SavePreset {
            name: "Hydrated".into(),
            folder: Some("F".into()),
        });
        a.tick();
        let n = a.take_journal() as usize;
        let mut c = Cur {
            b: &a.journal_out[..n],
            p: 0,
        };
        let ops = c.u32();
        let mut record = Vec::new();
        for _ in 0..ops {
            let tag = c.u32();
            let key = c.s();
            if tag == JW_PUT {
                let len = c.u32() as usize;
                let bytes = c.b[c.p..c.p + len].to_vec();
                c.p += len;
                if key == "F/Hydrated.toml" {
                    record = bytes;
                }
            }
        }
        assert!(!record.is_empty(), "no Put for the saved preset");

        // …and replay it into a fresh one, as the boot path does.
        let mut b = fresh();
        stage(&mut b, &[b"F"]);
        vxnc_hydrate_folder_on(&mut b, 1);
        stage(&mut b, &[b"F/Hydrated.toml", &record]);
        assert_eq!(
            vxnc_hydrate_preset_on(&mut b, "F/Hydrated.toml".len(), record.len()),
            1,
            "hydration rejected a record it had just written"
        );
        b.hydrate_done();
        assert_eq!(b.take_journal(), 4, "hydration journalled (header only expected)");
        let json = String::from_utf8(b.corpus_json.clone()).unwrap();
        assert!(json.contains("Hydrated"), "hydrated preset not in the corpus");
        assert!(
            b.ctrl
                .preset_store()
                .user_load(&PathBuf::from("F/Hydrated.toml"))
                .is_ok()
        );
    }

    // Instance-scoped stand-ins for the two hydrate opcodes (the real ones read
    // the global STATE, which tests do not construct).
    fn vxnc_hydrate_folder_on(s: &mut ControllerState, name_len: usize) {
        let name = s.arg_string(0, name_len);
        s.user.lock().unwrap().hydrate_folder(&name);
    }
    fn vxnc_hydrate_preset_on(s: &mut ControllerState, key_len: usize, rec_len: usize) -> u32 {
        let key = s.arg_string(0, key_len);
        let rec = s.arg_in[key_len..key_len + rec_len].to_vec();
        match decode_record(&rec) {
            Ok((meta, blob, warnings)) => {
                s.user.lock().unwrap().hydrate_preset(&key, meta, blob, warnings);
                1
            }
            Err(_) => 0,
        }
    }

    // ── State + TOML ────────────────────────────────────────────────────────

    /// Snapshot / restore is the autosave + share-link path. The restore writes
    /// the model directly, so it must also re-broadcast — including the matrix
    /// and key state, which no param event covers.
    #[test]
    fn snapshot_restore_round_trips_and_rebroadcasts() {
        let mut s = fresh();
        s.tick();
        let cutoff = clap_id_of(Layer::L1, P::Cutoff);
        s.post(UiEvent::SetParam {
            id: ParamId::new(cutoff),
            plain: 640.0,
        });
        s.post_custom(KeyOp::SetKeyMode(2));
        s.post_custom(MatrixEdit {
            layer: Layer::L1,
            slot: 2,
            field: MatrixField::Curve,
            value: Curve::Exp as u8,
        });
        s.tick();

        let n = s.snapshot_state() as usize;
        let saved = s.state_buf[..n].to_vec();

        // Move everything away from the snapshot.
        s.post(UiEvent::SetParam {
            id: ParamId::new(cutoff),
            plain: 120.0,
        });
        s.post_custom(KeyOp::SetKeyMode(0));
        s.post_custom(MatrixEdit {
            layer: Layer::L1,
            slot: 2,
            field: MatrixField::Curve,
            value: Curve::Lin as u8,
        });
        s.tick();

        s.state_buf.clear();
        s.state_buf.extend_from_slice(&saved);
        assert_eq!(s.restore_state(saved.len()), 1);
        s.tick();

        assert!((s.model.get(cutoff) - 640.0).abs() < 1.0);
        assert_eq!(s.model.key_state().key_mode(), KeyMode::Split);
        assert_eq!(s.model.matrix_snapshot()[0].slots[2].curve, Curve::Exp);

        let recs = decode(&s.view_out);
        let mut ids = params(&recs);
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), TOTAL_PARAMS, "restore did not re-broadcast the table");
        assert!(matrix_rec(&recs).is_some(), "restore did not echo the topology");
        assert!(key_rec(&recs).is_some(), "restore did not echo the key state");
    }

    #[test]
    fn restore_rejects_a_bad_blob_without_mutating() {
        let mut s = fresh();
        s.tick();
        let before = ParamModel::snapshot_bytes(&*s.model);
        s.state_buf.clear();
        s.state_buf.extend_from_slice(&[1, 2, 3, 4]);
        assert_eq!(s.restore_state(4), 0);
        assert_eq!(ParamModel::snapshot_bytes(&*s.model), before);
        s.tick();
        assert!(
            decode(&s.view_out).is_empty(),
            "a rejected restore still emitted events"
        );
    }

    #[test]
    fn export_import_toml_round_trips() {
        let mut s = fresh();
        s.tick();
        let cutoff = clap_id_of(Layer::L1, P::Cutoff);
        s.post(UiEvent::SetParam {
            id: ParamId::new(cutoff),
            plain: 512.0,
        });
        s.tick();

        stage(&mut s, &[b"Shared Patch"]);
        let n = s.export_toml("Shared Patch".len()) as usize;
        assert!(n > 0);
        let text = String::from_utf8(s.toml_buf[..n].to_vec()).unwrap();
        assert!(text.contains("Shared Patch"), "export lost the name");

        s.post(UiEvent::SetParam {
            id: ParamId::new(cutoff),
            plain: 100.0,
        });
        s.tick();

        s.toml_buf.clear();
        s.toml_buf.extend_from_slice(text.as_bytes());
        assert_eq!(s.import_toml(text.len()), 1);
        s.tick();
        assert!((s.model.get(cutoff) - 512.0).abs() < 1.0, "import did not apply");
        let mut ids = params(&decode(&s.view_out));
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), TOTAL_PARAMS, "import did not re-broadcast");
    }

    #[test]
    fn import_rejects_garbage_without_mutating() {
        let mut s = fresh();
        s.tick();
        let before = ParamModel::snapshot_bytes(&*s.model);
        let junk = b"not a preset at all";
        s.toml_buf.clear();
        s.toml_buf.extend_from_slice(junk);
        assert_eq!(s.import_toml(junk.len()), 0);
        assert_eq!(ParamModel::snapshot_bytes(&*s.model), before);
    }

    /// Every patch param exists on both layers at a distinct id — the flat
    /// two-layer map the JS side has to split back apart.
    #[test]
    fn the_two_layers_occupy_distinct_id_ranges() {
        for &p in PATCH_PARAMS.iter() {
            let a = clap_id_of(Layer::L1, p);
            let b = clap_id_of(Layer::L2, p);
            assert_ne!(a, b, "{p:?} collides across layers");
            assert!(a < TOTAL_PARAMS && b < TOTAL_PARAMS);
        }
    }
}
