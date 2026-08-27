// Main-thread controller wasm glue (ticket 0291) — the JS half of the web MVC.
//
// Instantiates `vxn1b-web-controller` (ticket 0290) on the main thread and
// drives it over the narrow C-ABI opcode surface it exposes: `vxnc_ui_*` in,
// packed ViewEvents out, `vxnc_tick`. No Rust enums cross; no wasm-bindgen.
//
// The controller holds the AUTHORITATIVE param values in ITS linear memory (the
// engine wasm in the worklet has its own), so this glue mirrors them into the
// param SAB the worklet reads lock-free, and decodes the packed ViewEvent drain
// into the same event-object shapes the native faceplate's `applyViewEvents`
// consumes.
//
// Deliberately a pure wasm wrapper: it knows nothing about the event ring or
// the page. Routing an opcode to BOTH the controller and the ring is
// `faceplate-bridge.mjs`'s job, so the two destinations stay legible in one
// place rather than being buried in half the setters here.
//
// Ported from vxn-2's `controller.mjs`, with three shape differences:
//   - 6 record tags, not 8 (no operator tab, no KS/EG curves; VXN1b adds the
//     matrix snapshot and the keyboard record).
//   - No factory asset. 0290 embeds the bank, so the corpus JSON is readable
//     immediately after `vxnc_new()` — there is nothing to fetch and no
//     `loadFactoryAsset`.
//   - Two-layer param space, so the wasm exports patch and global counts. JS
//     stores `patchCount` but does not compute ids from it — the page gets
//     `__PATCH_COUNT__` baked in from Rust. It is here because the boot
//     handshake checks it against the mirror (0312).

import { PATCH_COUNT, GLOBAL_COUNT, TOTAL_PARAMS } from "./param-store.mjs";

const DEFAULT_CONTROLLER_WASM_URL = "./vxn1b_web_controller.wasm";

// ViewEvent record tags — MUST match vxn1b-web-controller/src/lib.rs (VE_*).
export const VE_PARAM_CHANGED = 1;
export const VE_MATRIX_SNAPSHOT = 2;
export const VE_KEY_STATE = 3;
export const VE_PRESET_LOADED = 4;
export const VE_CORPUS_CHANGED = 5;
export const VE_STATUS = 6;

// PresetSource discriminants inside VE_PRESET_LOADED (match lib.rs).
const PRESET_SRC_NONE = 0;
const PRESET_SRC_FACTORY = 1;
const PRESET_SRC_USER = 2;

// Persistence-journal wire tags (match lib.rs JW_*).
export const JW_PUT = 1;
export const JW_DELETE = 2;
export const JW_PUT_FOLDER = 3;
export const JW_DELETE_FOLDER = 4;

// Sentinel length for an absent optional opcode argument (folder = root).
export const ARG_NONE = 0xffffffff;

// Matrix geometry, mirrored from the Rust packer. Kept local rather than
// imported from event-codec.mjs so a decode bug can't be masked by the same
// constant being wrong in both places — `wasm-agreement` pins the real ones.
const LAYERS = 2;
const SLOTS_PER_LAYER = 16;

/// Decode the packed ViewEvent drain into event objects whose shape matches
/// what the page's dispatcher consumes — the SAME JSON the native serialisers
/// emit (`vxn-core-ui-web::view_event_json` for the shared kinds,
/// `vxn1b-ui-web::serialise_custom_payload` for `matrix` / `keys`), each with a
/// `kind` discriminant.
///
/// Exported so a node test exercises THIS decoder against bytes the Rust packer
/// actually produced: drift then fails in CI rather than silently at runtime.
export function decodeViewEvents(buffer, ptr, len) {
  if (!len) return [];
  const view = new DataView(buffer, ptr, len);
  const dec = new TextDecoder();
  let off = 0;
  const u32 = () => {
    const v = view.getUint32(off, true);
    off += 4;
    return v;
  };
  const f32 = () => {
    const v = view.getFloat32(off, true);
    off += 4;
    return v;
  };
  const u8 = () => view.getUint8(off++);
  const str = () => {
    const n = u32();
    const bytes = new Uint8Array(buffer, ptr + off, n);
    off += n;
    return dec.decode(bytes);
  };

  const count = u32();
  const out = [];
  for (let i = 0; i < count; i++) {
    const tag = u32();
    switch (tag) {
      case VE_PARAM_CHANGED:
        out.push({
          kind: "param_changed",
          id: u32(),
          plain: f32(),
          norm: f32(),
          display: str(),
        });
        break;
      case VE_MATRIX_SNAPSHOT: {
        // Both layers, 16 slots each: source, dest, curve, scale_src. Depths
        // are CLAP params and arrive as `param_changed`, exactly as they do
        // natively — the page's overlay merges the two.
        const slots = [];
        for (let l = 0; l < LAYERS; l++) {
          const layer = [];
          for (let s = 0; s < SLOTS_PER_LAYER; s++) {
            layer.push({ source: u8(), dest: u8(), curve: u8(), scale: u8() });
          }
          slots.push(layer);
        }
        out.push({ kind: "matrix", slots });
        break;
      }
      case VE_KEY_STATE:
        // `link` is a bool in the native echo; the wire carries 0/1.
        out.push({ kind: "keys", mode: u8(), split: u8(), link: u8() !== 0 });
        break;
      case VE_PRESET_LOADED: {
        const name = str();
        const srcKind = u32();
        let source = null;
        if (srcKind === PRESET_SRC_FACTORY) source = { kind: "factory", index: u32() };
        else if (srcKind === PRESET_SRC_USER) source = { kind: "user", path: str() };
        const nWarn = u32();
        const warnings = [];
        for (let w = 0; w < nWarn; w++) warnings.push(str());
        out.push({ kind: "preset_loaded", name, source, warnings });
        break;
      }
      case VE_CORPUS_CHANGED: {
        const has = u32();
        out.push({ kind: "preset_corpus_changed", follow: has === 1 ? str() : null });
        break;
      }
      case VE_STATUS:
        out.push({ kind: "status", line: str() });
        break;
      default:
        // A tag we don't know means the Rust packer moved and this file did
        // not. There is no way to find the next record's boundary, so fail
        // loudly rather than emit garbage for the rest of the batch.
        throw new Error(`unknown ViewEvent tag ${tag} at record ${i}`);
    }
  }
  return out;
}

/// Decode the packed persistence journal into `{tag, key, bytes?}` ops, the
/// shape the shared `preset-persistence.mjs` (0284) applies to IndexedDB.
export function decodeJournal(buffer, ptr, len) {
  if (!len) return [];
  const view = new DataView(buffer, ptr, len);
  const dec = new TextDecoder();
  let off = 0;
  const u32 = () => {
    const v = view.getUint32(off, true);
    off += 4;
    return v;
  };
  const str = () => {
    const n = u32();
    const s = dec.decode(new Uint8Array(buffer, ptr + off, n));
    off += n;
    return s;
  };
  const bytes = () => {
    const n = u32();
    // Copied, not a view: the next opcode's `_stage` can resize (and detach)
    // the wasm heap this points into.
    const b = new Uint8Array(buffer, ptr + off, n).slice();
    off += n;
    return b;
  };
  const count = u32();
  const ops = [];
  for (let i = 0; i < count; i++) {
    const tag = u32();
    switch (tag) {
      // The `kind` strings and the key/name split are `preset-storage.mjs`'s
      // contract, not ours: `applyWrites` switches on them, and folder ops carry
      // `name` where preset ops carry `key`.
      case JW_PUT:
        ops.push({ kind: "put", key: str(), bytes: bytes() });
        break;
      case JW_DELETE:
        ops.push({ kind: "delete", key: str() });
        break;
      case JW_PUT_FOLDER:
        ops.push({ kind: "put_folder", name: str() });
        break;
      case JW_DELETE_FOLDER:
        ops.push({ kind: "delete_folder", name: str() });
        break;
      default:
        throw new Error(`controller: unknown journal tag ${tag}`);
    }
  }
  return ops;
}

const enc = new TextEncoder();

/// Thin, allocation-light wrapper over the controller wasm's opcode surface.
export class WebController {
  constructor({
    wasmUrl = DEFAULT_CONTROLLER_WASM_URL,
    wasmBytes = null,
    store = null,
    fetchImpl = typeof fetch === "function" ? fetch : null,
  } = {}) {
    this.wasmUrl = wasmUrl;
    this.wasmBytes = wasmBytes;
    this.store = store;
    this._fetch = fetchImpl ? fetchImpl.bind(globalThis) : null;

    this.x = null; // instance.exports
    this.totalParams = 0;
    this.patchCount = 0;
    // Mirror of what was last written into the param SAB, so a tick writes only
    // CHANGED slots. NaN-seeded → the first mirror writes everything.
    this._mirrored = new Float32Array(TOTAL_PARAMS).fill(NaN);
  }

  async instantiate() {
    if (this.x) throw new Error("WebController.instantiate() already called");
    const bytes = await this._loadBytes();
    const { instance } = await WebAssembly.instantiate(bytes, {});
    this.x = instance.exports;

    // The param counts are owned by the wasm (vxn1b-engine); assert the JS
    // mirror agrees so layout drift is caught at boot rather than as silent
    // corruption of whichever half is shorter.
    //
    // All THREE counts, not the total alone: a compensating drift (+1 patch,
    // -2 global) leaves the total intact while every Layer 2 and global id the
    // mirror computes is wrong — a boot that passes the handshake and then
    // writes the wrong params (0312).
    const total = this.x.vxnc_total_params();
    const patch = this.x.vxnc_patch_count();
    const global = this.x.vxnc_global_count();
    for (const [what, got, want] of [
      ["PATCH_COUNT", patch, PATCH_COUNT],
      ["GLOBAL_COUNT", global, GLOBAL_COUNT],
      ["TOTAL_PARAMS", total, TOTAL_PARAMS],
    ]) {
      if (got !== want) {
        throw new Error(`controller ${what} ${got} != JS mirror ${want} — param layout drift`);
      }
    }
    this.totalParams = total;
    this.patchCount = patch;
    this.x.vxnc_new();
    return this;
  }

  destroy() {
    if (this.x) this.x.vxnc_destroy();
  }

  async _loadBytes() {
    if (this.wasmBytes) return this.wasmBytes;
    if (!this._fetch) throw new Error("no fetch and no wasmBytes provided");
    const resp = await this._fetch(this.wasmUrl);
    if (!resp.ok) throw new Error(`controller wasm fetch failed: ${resp.status}`);
    return resp.arrayBuffer();
  }

  // ---- Hot path (1:1 with the vxnc_ui_* exports) --------------------------

  setParam(id, plain) {
    this.x.vxnc_ui_set_param(id >>> 0, plain);
  }
  setParamNorm(id, norm) {
    this.x.vxnc_ui_set_param_norm(id >>> 0, norm);
  }
  beginGesture(id) {
    this.x.vxnc_ui_begin_gesture(id >>> 0);
  }
  endGesture(id) {
    this.x.vxnc_ui_end_gesture(id >>> 0);
  }
  editorReady() {
    this.x.vxnc_ui_editor_ready();
  }

  // ---- VXN1b custom opcodes ----------------------------------------------
  //
  // The key ops and the matrix edit are "both" ops — the engine is a separate
  // wasm and needs them too. This class only does the model half; the bridge
  // pushes the matching ring event.

  setKeyMode(mode) {
    this.x.vxnc_ui_set_key_mode(mode >>> 0);
  }
  setSplitPoint(note) {
    this.x.vxnc_ui_set_split_point(note >>> 0);
  }
  setLfo2Link(on) {
    this.x.vxnc_ui_set_lfo2_link(on ? 1 : 0);
  }
  setMatrix(layer, slot, field, value) {
    this.x.vxnc_ui_set_matrix(layer >>> 0, slot >>> 0, field >>> 0, value >>> 0);
  }
  copyLayer(from, to) {
    this.x.vxnc_ui_copy_layer(from >>> 0, to >>> 0);
  }
  resetLayer(layer) {
    this.x.vxnc_ui_reset_layer(layer >>> 0);
  }

  // ---- Presets ------------------------------------------------------------

  factoryLen() {
    return this.x.vxnc_factory_len();
  }

  /// The browser corpus JSON (factory + user groups) — the same shape the
  /// native editor feeds `applyPresetCorpus`. Available immediately after
  /// `instantiate()`: the factory bank is embedded, not fetched.
  corpusJson() {
    const len = this.x.vxnc_corpus_json_len();
    if (!len) return { factory: [], user: [] };
    const ptr = this.x.vxnc_corpus_json_ptr();
    const bytes = new Uint8Array(this.x.memory.buffer, ptr, len);
    return JSON.parse(new TextDecoder().decode(bytes));
  }

  loadFactory(index) {
    this.x.vxnc_ui_load_factory(index >>> 0);
  }
  stepPreset(delta) {
    this.x.vxnc_ui_step_preset(delta | 0);
  }

  // ---- Argument staging ---------------------------------------------------

  /// Stage byte arrays into the wasm arg buffer, returning per-part lengths.
  /// The memory view is taken AFTER the reserve call: growing the Vec can
  /// resize (and detach) the wasm heap, invalidating any view held across it.
  _stageRaw(parts) {
    const total = parts.reduce((n, p) => n + p.length, 0);
    const ptr = this.x.vxnc_arg_buf_reserve(total >>> 0);
    const mem = new Uint8Array(this.x.memory.buffer, ptr, total);
    let off = 0;
    for (const p of parts) {
      mem.set(p, off);
      off += p.length;
    }
    return parts.map((p) => p.length);
  }

  /// Stage strings; `null` / `undefined` become the ARG_NONE sentinel rather
  /// than an empty string, which is a distinct (and valid) folder name.
  _stage(...args) {
    const parts = [];
    const lens = [];
    for (const a of args) {
      if (a === null || a === undefined) {
        lens.push(ARG_NONE);
      } else {
        const b = enc.encode(String(a));
        parts.push(b);
        lens.push(b.length);
      }
    }
    this._stageRaw(parts);
    return lens;
  }

  // ---- User presets + folders --------------------------------------------

  savePreset(name, folder = null) {
    const [nameLen, folderLen] = this._stage(name, folder);
    this.x.vxnc_ui_save_preset(nameLen >>> 0, folderLen >>> 0);
  }
  loadUser(path) {
    const [pathLen] = this._stage(path);
    this.x.vxnc_ui_load_user(pathLen >>> 0);
  }
  renamePreset(path, newName) {
    const [pathLen, nameLen] = this._stage(path, newName);
    this.x.vxnc_ui_rename_preset(pathLen >>> 0, nameLen >>> 0);
  }
  deletePreset(path) {
    const [pathLen] = this._stage(path);
    this.x.vxnc_ui_delete_preset(pathLen >>> 0);
  }
  movePreset(path, destFolder = null) {
    const [pathLen, folderLen] = this._stage(path, destFolder);
    this.x.vxnc_ui_move_preset(pathLen >>> 0, folderLen >>> 0);
  }
  newFolder(suggested) {
    const [len] = this._stage(suggested);
    this.x.vxnc_ui_new_folder(len >>> 0);
  }
  renameFolder(oldName, newName) {
    const [oldLen, newLen] = this._stage(oldName, newName);
    this.x.vxnc_ui_rename_folder(oldLen >>> 0, newLen >>> 0);
  }
  deleteFolder(name) {
    const [len] = this._stage(name);
    this.x.vxnc_ui_delete_folder(len >>> 0);
  }

  // ---- Boot hydration (0293 drives these) --------------------------------

  hydrateFolder(name) {
    const [len] = this._stage(name);
    this.x.vxnc_hydrate_folder(len >>> 0);
  }
  /// `record` is the stored TOML bytes. Returns 1 on success, 0 if the record
  /// failed to parse — hydration skips it rather than aborting the boot.
  hydratePreset(key, record) {
    const keyBytes = enc.encode(String(key));
    const recBytes = record instanceof Uint8Array ? record : new Uint8Array(record);
    this._stageRaw([keyBytes, recBytes]);
    return this.x.vxnc_hydrate_preset(keyBytes.length >>> 0, recBytes.length >>> 0);
  }
  hydrateDone() {
    this.x.vxnc_hydrate_done();
  }

  // ---- Journal ------------------------------------------------------------

  /// Drain the pending persistence ops. Synchronous by contract: the wasm
  /// journal is emptied now and the caller owns the flush.
  takeJournal() {
    const len = this.x.vxnc_take_journal();
    if (!len) return [];
    const ptr = this.x.vxnc_journal_out_ptr();
    return decodeJournal(this.x.memory.buffer, ptr, len);
  }

  // ---- Full state + TOML --------------------------------------------------

  /// The whole patch as the canonical state blob (autosave + share link).
  snapshotState() {
    const len = this.x.vxnc_snapshot_state();
    if (!len) return new Uint8Array(0);
    const ptr = this.x.vxnc_state_out_ptr();
    return new Uint8Array(this.x.memory.buffer, ptr, len).slice();
  }
  /// Returns true on success; a malformed blob leaves the model untouched.
  restoreState(bytes) {
    const b = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    const ptr = this.x.vxnc_state_buf_reserve(b.length >>> 0);
    new Uint8Array(this.x.memory.buffer, ptr, b.length).set(b);
    return this.x.vxnc_restore_state(b.length >>> 0) === 1;
  }
  /// The current patch as sparse TOML text — byte-identical to what the plugin
  /// would write to disk.
  exportToml(name) {
    const [len] = this._stage(name);
    const n = this.x.vxnc_export_toml(len >>> 0);
    if (!n) return "";
    const ptr = this.x.vxnc_toml_out_ptr();
    return new TextDecoder().decode(new Uint8Array(this.x.memory.buffer, ptr, n));
  }
  /// Returns true on success; a malformed file leaves the model untouched.
  importToml(text) {
    const b = enc.encode(String(text));
    const ptr = this.x.vxnc_toml_buf_reserve(b.length >>> 0);
    new Uint8Array(this.x.memory.buffer, ptr, b.length).set(b);
    return this.x.vxnc_import_toml(b.length >>> 0) === 1;
  }

  // ---- Tick ---------------------------------------------------------------

  /// Drive one controller tick and return the decoded ViewEvent batch. Does NOT
  /// mirror to the param SAB — the caller sequences that against its ring
  /// pushes (0291: topology and depths must land in the same block).
  tick() {
    this.x.vxnc_tick();
    const len = this.x.vxnc_view_len();
    if (!len) return [];
    const ptr = this.x.vxnc_view_ptr();
    return decodeViewEvents(this.x.memory.buffer, ptr, len);
  }

  /// Forget what the store has been told, so the next `mirrorToStore()` rewrites
  /// every slot.
  ///
  /// Needed because the audio graph seeds the store itself: `WebHost.start()`
  /// runs `_seedStoreFromDefaults`, a `writeBulk` of the ENGINE's defaults, on
  /// the first start. The faceplate is interactive before that gesture, so any
  /// edit or preset load made while waiting would be overwritten — and the
  /// mirror would not repair it, since it only writes slots that changed. The
  /// boot path calls this once the worklet reports ready.
  invalidateMirror() {
    this._mirrored.fill(NaN);
  }

  /// Copy changed param values into the SAB the worklet reads. Returns the
  /// number of slots written — 0 means the model did not move this tick.
  mirrorToStore() {
    if (!this.store) return 0;
    const ptr = this.x.vxnc_values_ptr();
    const vals = new Float32Array(this.x.memory.buffer, ptr, this.totalParams);
    let written = 0;
    for (let i = 0; i < this.totalParams; i++) {
      const v = vals[i];
      // NaN-aware: the NaN seed forces every slot on the first pass, and
      // thereafter only genuine drift is written.
      if (v === this._mirrored[i]) continue;
      this._mirrored[i] = v;
      this.store.write(i, v);
      written++;
    }
    return written;
  }
}
