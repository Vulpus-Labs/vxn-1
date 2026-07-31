// Main-thread controller wasm glue (ticket 0157) — the JS half of the web MVC.
//
// Instantiates the `vxn2-web-controller` wasm on the main thread (the engine
// wasm runs in the worklet) and drives it over the narrow C-ABI opcode surface
// it exposes (ticket 0154): `vxnc_ui_*` in, packed ViewEvents out, `vxnc_tick`.
// Never crosses Rust enums; no wasm-bindgen.
//
// The controller holds the AUTHORITATIVE param values in ITS linear memory; this
// glue mirrors them into the store SAB the worklet reads lock-free (so UI edits
// reach the engine), and decodes the packed ViewEvent drain into the same event-
// object shape the native faceplate's `applyViewEvents` consumes.
//
// Ported from vxn-1's `controller.mjs`; the vxn-2 surface is leaner — no key-
// mode/split/layer, and preset / journal / state / TOML ops are deferred to 0159.

import { TOTAL_PARAMS } from "./param-store.mjs";

const DEFAULT_CONTROLLER_WASM_URL = "./vxn2_web_controller.wasm";

// ViewEvent record tags — MUST match vxn2-web-controller/src/lib.rs (VE_*).
export const VE_PARAM_CHANGED = 1;
export const VE_OP_TAB_CHANGED = 2;
export const VE_MATRIX_SNAPSHOT = 3;
export const VE_KS_CURVE_SNAPSHOT = 4;
export const VE_EG_CURVE_SNAPSHOT = 5;
export const VE_PRESET_LOADED = 6;
export const VE_CORPUS_CHANGED = 7;
export const VE_STATUS = 8;

// PresetSource discriminants in the VE_PRESET_LOADED record (match lib.rs).
const PRESET_SRC_NONE = 0;
const PRESET_SRC_FACTORY = 1;
const PRESET_SRC_USER = 2;

// Persistence-journal wire tags (match lib.rs JW_*).
const JW_PUT = 1;
const JW_DELETE = 2;
const JW_PUT_FOLDER = 3;
const JW_DELETE_FOLDER = 4;

// Sentinel length for an absent optional opcode argument (folder = root).
const ARG_NONE = 0xffffffff;

// Decode a packed ViewEvent out-buffer into an array of event objects whose
// shape matches what the faceplate's `applyViewEvents` (`main.js`) consumes —
// i.e. the SAME JSON the native `serialise_custom_view` / core serialiser emit,
// each carrying a `kind` discriminant. The wire layout is the tag-prefixed
// binary protocol documented in vxn2-web-controller/src/lib.rs. Pulled out so a
// node test exercises THIS decoder against the Rust packer's bytes — drift fails
// in CI, not silently at runtime.
export function decodeViewEvents(buffer, ptr, len) {
  const view = new DataView(buffer, ptr, len);
  const dec = new TextDecoder();
  let off = 0;
  const u32 = () => {
    const v = view.getUint32(off, true);
    off += 4;
    return v;
  };
  const u8 = () => view.getUint8(off++);
  const f32 = () => {
    const v = view.getFloat32(off, true);
    off += 4;
    return v;
  };
  const str = () => {
    const n = u32();
    const s = dec.decode(new Uint8Array(buffer, ptr + off, n));
    off += n;
    return s;
  };

  const count = u32();
  const out = [];
  for (let i = 0; i < count; i++) {
    const tag = u32();
    switch (tag) {
      case VE_PARAM_CHANGED:
        out.push({ kind: "param_changed", id: u32(), plain: f32(), norm: f32(), display: str() });
        break;
      case VE_OP_TAB_CHANGED:
        out.push({ kind: "op_tab_changed", op: u32() });
        break;
      case VE_MATRIX_SNAPSHOT: {
        const n = u32();
        const rows = [];
        for (let r = 0; r < n; r++) {
          rows.push({
            source: u8(),
            dest: u8(),
            curve: u8(),
            active: u8() !== 0,
            depth: f32(),
            scale: u8(), // E033 secondary scale source (JS field name matches the native JSON wire)
          });
        }
        out.push({ kind: "matrix_snapshot", rows });
        break;
      }
      case VE_KS_CURVE_SNAPSHOT: {
        // 6 ops × [L, R] u8.
        const curves = [];
        for (let opi = 0; opi < 6; opi++) curves.push([u8(), u8()]);
        out.push({ kind: "ks_curve_snapshot", curves });
        break;
      }
      case VE_EG_CURVE_SNAPSHOT: {
        const curves = [];
        for (let opi = 0; opi < 6; opi++) curves.push(u8());
        out.push({ kind: "eg_curve_snapshot", curves });
        break;
      }
      case VE_PRESET_LOADED: {
        const name = str();
        const srcKind = u32();
        let source = null;
        if (srcKind === PRESET_SRC_FACTORY) source = { kind: "factory", index: u32() };
        else if (srcKind === PRESET_SRC_USER) source = { kind: "user", path: str() };
        const warnCount = u32();
        const warnings = [];
        for (let w = 0; w < warnCount; w++) warnings.push(str());
        out.push({ kind: "preset_loaded", name, source, warnings });
        break;
      }
      case VE_CORPUS_CHANGED: {
        const hasFollow = u32();
        const follow = hasFollow ? str() : null;
        out.push({ kind: "preset_corpus_changed", follow });
        break;
      }
      case VE_STATUS:
        out.push({ kind: "status", line: str() });
        break;
      default:
        throw new Error(`controller: unknown ViewEvent tag ${tag}`);
    }
  }
  return out;
}

export class WebController {
  // Construct cheaply; instantiate() does the async wasm load. Options:
  //   wasmUrl   : dist-relative URL of the controller wasm.
  //   wasmBytes : pre-fetched controller bytes; skips the fetch (node test).
  //   store     : a ParamStore over the SHARED param SAB. The controller mirrors
  //               its model values into it so the worklet applies them. Optional.
  //   ring      : the coordinator's EventRing producer. Matrix topology has no
  //               CLAP id so it can't ride `store`; setMatrixRow pushes it here so
  //               the worklet's audible route follows the UI (ticket 0193). Optional.
  //   onViewEvents : sink called with the decoded event-object array each tick.
  //   fetchImpl : fetch seam (defaults to global fetch).
  constructor({
    wasmUrl = DEFAULT_CONTROLLER_WASM_URL,
    wasmBytes = null,
    store = null,
    ring = null,
    onViewEvents = () => {},
    fetchImpl = globalThis.fetch,
  } = {}) {
    this.wasmUrl = wasmUrl;
    this.wasmBytes = wasmBytes;
    this.store = store;
    this.ring = ring;
    this._onViewEvents = onViewEvents;
    this._fetch = fetchImpl ? fetchImpl.bind(globalThis) : null;

    this.x = null; // instance.exports
    // Mirror of what we last wrote into the store SAB, so a tick only writes
    // CHANGED slots (latest-value-wins store). NaN-seeded → first mirror writes all.
    this._mirrored = new Float32Array(TOTAL_PARAMS).fill(NaN);
  }

  async instantiate() {
    if (this.x) throw new Error("WebController.instantiate() already called");
    const bytes = await this._loadBytes();
    const { instance } = await WebAssembly.instantiate(bytes, {});
    this.x = instance.exports;

    // The param count is owned by the wasm (vxn2-engine); assert the JS mirror
    // agrees so drift is caught at boot rather than as silent corruption.
    const total = this.x.vxnc_total_params();
    if (total !== TOTAL_PARAMS) {
      throw new Error(
        `controller TOTAL_PARAMS ${total} != JS mirror ${TOTAL_PARAMS} — param layout drift`,
      );
    }
    this.totalParams = total;
    this.x.vxnc_new();
    return this;
  }

  async _loadBytes() {
    if (this.wasmBytes) return this.wasmBytes;
    if (!this._fetch) throw new Error("no fetch and no wasmBytes provided");
    const resp = await this._fetch(this.wasmUrl);
    if (!resp.ok) throw new Error(`controller wasm fetch failed: ${resp.status}`);
    return resp.arrayBuffer();
  }

  // ---- UiEvent opcode surface (1:1 with vxnc_ui_* exports) ----------------

  beginGesture(id) {
    this.x.vxnc_ui_begin_gesture(id >>> 0);
  }
  endGesture(id) {
    this.x.vxnc_ui_end_gesture(id >>> 0);
  }
  setParamNorm(id, norm) {
    this.x.vxnc_ui_set_param_norm(id >>> 0, norm);
  }
  setParam(id, plain) {
    this.x.vxnc_ui_set_param(id >>> 0, plain);
  }
  editorReady() {
    this.x.vxnc_ui_editor_ready();
  }

  // ---- Vxn2 custom opcodes -------------------------------------------------

  setOpTab(op) {
    this.x.vxnc_ui_set_op_tab(op >>> 0);
  }
  setMatrixRow(slot, source, dest, curve, active, depth, scaleSrc = 0) {
    // (1) Controller wasm — authoritative model, drives UI snapshots.
    this.x.vxnc_ui_set_matrix_row(
      slot >>> 0, source >>> 0, dest >>> 0, curve >>> 0, active ? 1 : 0, depth, scaleSrc >>> 0,
    );
    // (2) Worklet — topology has no CLAP id so `_mirrorToStore` can't carry it;
    // push the row on the ring so the audible route follows (ticket 0193).
    // scaleSrc (E033) is topology too, so it rides the same ring push.
    if (this.ring) this.ring.pushMatrixRow(slot, source, dest, curve, active, depth, scaleSrc);
  }
  setKsCurve(op, side, curve) {
    this.x.vxnc_ui_set_ks_curve(op >>> 0, side >>> 0, curve >>> 0);
  }
  setEgCurve(op, curve) {
    this.x.vxnc_ui_set_eg_curve(op >>> 0, curve >>> 0);
  }
  requestMatrixSnapshot() {
    this.x.vxnc_ui_request_matrix_snapshot();
  }
  requestKsCurveSnapshot() {
    this.x.vxnc_ui_request_ks_curve_snapshot();
  }
  requestEgCurveSnapshot() {
    this.x.vxnc_ui_request_eg_curve_snapshot();
  }
  requestFullRebroadcast() {
    this.x.vxnc_ui_request_full_rebroadcast();
  }

  // ---- factory presets (ticket 0159, minimal) -----------------------------

  // Parse the fetched `factory.bin` bytes into the controller's factory bank.
  // Returns the preset count. Stages the bytes into wasm memory then loads.
  loadFactoryAsset(bytes) {
    const b = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    const ptr = this.x.vxnc_factory_buf_reserve(b.length >>> 0);
    new Uint8Array(this.x.memory.buffer, ptr, b.length).set(b);
    return this.x.vxnc_load_factory(b.length >>> 0);
  }

  // The browser corpus JSON (factory groups) the controller built when the
  // factory asset loaded — same shape the native editor feeds applyPresetCorpus.
  corpusJson() {
    const len = this.x.vxnc_corpus_json_len();
    if (!len) return { factory: [], user: [] };
    const ptr = this.x.vxnc_corpus_json_ptr();
    const bytes = new Uint8Array(this.x.memory.buffer, ptr, len);
    return JSON.parse(new TextDecoder().decode(bytes));
  }

  // Load factory preset `index`. The model restore + ParamChanged fan-out +
  // PresetLoaded land on the next tick().
  loadFactory(index) {
    this.x.vxnc_ui_load_factory(index >>> 0);
  }

  // Step to the previous / next preset (delta ±1).
  stepPreset(delta) {
    this.x.vxnc_ui_step_preset(delta | 0);
  }

  // ---- user presets + persistence (ticket 0159) ---------------------------

  // Stage a sequence of byte arrays into the wasm arg buffer (the concatenated
  // opcode-argument scratch). Returns the per-part byte lengths. The buffer view
  // is taken AFTER the reserve call, since resizing the Vec may grow (and detach)
  // wasm memory.
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

  // ---- user-preset opcodes (1:1 with the vxnc_ui_* user ops) --------------

  savePreset(name, folder) {
    const enc = new TextEncoder();
    const nameB = enc.encode(name || "");
    if (folder == null) {
      this._stageRaw([nameB]);
      this.x.vxnc_ui_save_preset(nameB.length >>> 0, ARG_NONE);
    } else {
      const folderB = enc.encode(folder);
      this._stageRaw([nameB, folderB]);
      this.x.vxnc_ui_save_preset(nameB.length >>> 0, folderB.length >>> 0);
    }
  }
  loadUser(path) {
    const pathB = new TextEncoder().encode(path || "");
    this._stageRaw([pathB]);
    this.x.vxnc_ui_load_user(pathB.length >>> 0);
  }
  renamePreset(path, newName) {
    const enc = new TextEncoder();
    const pathB = enc.encode(path || "");
    const nameB = enc.encode(newName || "");
    this._stageRaw([pathB, nameB]);
    this.x.vxnc_ui_rename_preset(pathB.length >>> 0, nameB.length >>> 0);
  }
  deletePreset(path) {
    const pathB = new TextEncoder().encode(path || "");
    this._stageRaw([pathB]);
    this.x.vxnc_ui_delete_preset(pathB.length >>> 0);
  }
  movePreset(path, destFolder) {
    const enc = new TextEncoder();
    const pathB = enc.encode(path || "");
    if (destFolder == null) {
      this._stageRaw([pathB]);
      this.x.vxnc_ui_move_preset(pathB.length >>> 0, ARG_NONE);
    } else {
      const folderB = enc.encode(destFolder);
      this._stageRaw([pathB, folderB]);
      this.x.vxnc_ui_move_preset(pathB.length >>> 0, folderB.length >>> 0);
    }
  }
  newFolder(suggested) {
    const b = new TextEncoder().encode(suggested || "");
    this._stageRaw([b]);
    this.x.vxnc_ui_new_folder(b.length >>> 0);
  }
  renameFolder(oldName, newName) {
    const enc = new TextEncoder();
    const oldB = enc.encode(oldName || "");
    const newB = enc.encode(newName || "");
    this._stageRaw([oldB, newB]);
    this.x.vxnc_ui_rename_folder(oldB.length >>> 0, newB.length >>> 0);
  }
  deleteFolder(name) {
    const b = new TextEncoder().encode(name || "");
    this._stageRaw([b]);
    this.x.vxnc_ui_delete_folder(b.length >>> 0);
  }

  // ---- boot hydration (replay IndexedDB into the wasm cache) --------------

  hydrateFolder(name) {
    const b = new TextEncoder().encode(name || "");
    this._stageRaw([b]);
    this.x.vxnc_hydrate_folder(b.length >>> 0);
  }
  // `bytes` is the stored TOML record (Uint8Array). Returns true if it parsed.
  hydratePreset(key, bytes) {
    const keyB = new TextEncoder().encode(key || "");
    const recB = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    this._stageRaw([keyB, recB]);
    return this.x.vxnc_hydrate_preset(keyB.length >>> 0, recB.length >>> 0) === 1;
  }
  hydrateDone() {
    this.x.vxnc_hydrate_done();
  }

  // ---- deferred-write journal (drained off the tick to IndexedDB) ---------

  // Drain the wasm user store's pending persistence ops into a decoded array of
  // { kind, key?, bytes?, name? } that preset-storage.applyWrites consumes.
  takeJournal() {
    const len = this.x.vxnc_take_journal();
    if (!len) return [];
    const ptr = this.x.vxnc_journal_out_ptr();
    const view = new DataView(this.x.memory.buffer, ptr, len);
    const buf = this.x.memory.buffer;
    const dec = new TextDecoder();
    let off = 0;
    const u32 = () => {
      const v = view.getUint32(off, true);
      off += 4;
      return v;
    };
    const str = () => {
      const n = u32();
      const s = dec.decode(new Uint8Array(buf, ptr + off, n));
      off += n;
      return s;
    };
    const bytes = () => {
      const n = u32();
      const b = new Uint8Array(buf, ptr + off, n).slice(); // copy off wasm memory
      off += n;
      return b;
    };
    const count = u32();
    const ops = [];
    for (let i = 0; i < count; i++) {
      const tag = u32();
      switch (tag) {
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

  // ---- full patch-state snapshot / restore (autosave + share-link) --------

  // Snapshot the full patch state as a fresh Uint8Array (copied off wasm memory).
  snapshotState() {
    const len = this.x.vxnc_snapshot_state();
    const ptr = this.x.vxnc_state_out_ptr();
    return new Uint8Array(this.x.memory.buffer, ptr, len).slice();
  }
  // Restore the model from a state blob. Returns true on success. A load is a
  // whole-patch swap, so pulse the worklet's patch-swap to silence ringing
  // voices (the load_epoch bump is controller-local — the worklet has its own
  // SharedParams).
  restoreState(blob) {
    const b = blob instanceof Uint8Array ? blob : new Uint8Array(blob);
    const ptr = this.x.vxnc_state_buf_reserve(b.length >>> 0);
    new Uint8Array(this.x.memory.buffer, ptr, b.length).set(b);
    const ok = this.x.vxnc_restore_state(b.length >>> 0) === 1;
    if (ok && this.ring) this.ring.pushPatchSwap();
    return ok;
  }

  // ---- TOML export / import (file + share) --------------------------------

  // Serialise the current patch to name-keyed TOML text.
  exportToml(name) {
    const [nameLen] = this._stageRaw([new TextEncoder().encode(name || "")]);
    const len = this.x.vxnc_export_toml(nameLen >>> 0);
    const ptr = this.x.vxnc_toml_out_ptr();
    return new TextDecoder().decode(new Uint8Array(this.x.memory.buffer, ptr, len));
  }
  // Apply a TOML patch to the model. Returns true on success. Pulses patch-swap
  // like restoreState (a runtime import is a whole-patch swap).
  importToml(text) {
    const b = new TextEncoder().encode(text || "");
    const ptr = this.x.vxnc_toml_buf_reserve(b.length >>> 0);
    new Uint8Array(this.x.memory.buffer, ptr, b.length).set(b);
    const ok = this.x.vxnc_import_toml(b.length >>> 0) === 1;
    if (ok && this.ring) this.ring.pushPatchSwap();
    return ok;
  }

  // ---- tick: drain queues → mutate model → mirror + drain ViewEvents ------

  // Call on each rAF (or after a gesture burst): (1) tick the controller, (2)
  // mirror the model's current values into the store SAB so the worklet applies
  // them, (3) decode + dispatch the packed ViewEvents to the sink. Returns the
  // decoded event-object array.
  tick() {
    this.x.vxnc_tick();
    this._mirrorToStore();
    const events = this._drainViewEvents();
    if (events.length) {
      this._mirrorControlToRing(events);
      this._onViewEvents(events);
    }
    return events;
  }

  // Mirror control state the value-store can't carry to the worklet ring (0193):
  //
  //  - `preset_loaded` → a `patchSwap` pulse. A preset load / reset silences the
  //    outgoing patch on native via a shared `load_epoch`; the web worklet holds
  //    a separate SharedParams and the epoch isn't a value param, so without this
  //    the previous patch's voices ring on. Pushed FIRST so the silence lands
  //    before the new topology below.
  //  - `matrix_snapshot` → one `pushMatrixRow` per slot. Live single-row edits
  //    push directly from `setMatrixRow`, but BULK changes (preset loads, reset)
  //    only surface a snapshot (they never call `setMatrixRow`). `mark_all_dirty`
  //    guarantees the snapshot fires on every such load.
  _mirrorControlToRing(events) {
    if (!this.ring) return;
    for (const e of events) {
      if (e.kind === "preset_loaded") this.ring.pushPatchSwap();
    }
    for (const e of events) {
      if (e.kind !== "matrix_snapshot") continue;
      for (let slot = 0; slot < e.rows.length; slot++) {
        const r = e.rows[slot];
        this.ring.pushMatrixRow(slot, r.source, r.dest, r.curve, r.active, r.depth, r.scale | 0);
      }
    }
  }

  _mirrorToStore() {
    if (!this.store) return;
    const ptr = this.x.vxnc_values_ptr();
    const vals = new Float32Array(this.x.memory.buffer, ptr, TOTAL_PARAMS);
    for (let id = 0; id < TOTAL_PARAMS; id++) {
      const v = vals[id];
      if (v === this._mirrored[id]) continue; // unchanged (NaN seed forces first)
      this._mirrored[id] = v;
      this.store.write(id, v);
    }
  }

  // Force the NEXT tick's mirror pass to re-write EVERY param into the store SAB.
  // Called after the audio coordinator's start() runs its default-seed writeBulk
  // into the SAME store, so the controller's authoritative values win again.
  remirrorStore() {
    this._mirrored.fill(NaN);
  }

  _drainViewEvents() {
    const ptr = this.x.vxnc_view_ptr();
    const len = this.x.vxnc_view_len();
    return decodeViewEvents(this.x.memory.buffer, ptr, len);
  }

  destroy() {
    if (this.x) {
      try {
        this.x.vxnc_destroy();
      } catch {}
    }
  }
}
