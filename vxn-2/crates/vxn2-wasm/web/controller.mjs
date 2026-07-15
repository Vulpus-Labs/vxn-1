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

// PresetSource discriminants in the VE_PRESET_LOADED record (match lib.rs).
const PRESET_SRC_NONE = 0;
const PRESET_SRC_FACTORY = 1;

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
        const warnCount = u32();
        const warnings = [];
        for (let w = 0; w < warnCount; w++) warnings.push(str());
        out.push({ kind: "preset_loaded", name, source, warnings });
        break;
      }
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
  setMatrixRow(slot, source, dest, curve, active, depth) {
    // (1) Controller wasm — authoritative model, drives UI snapshots.
    this.x.vxnc_ui_set_matrix_row(slot >>> 0, source >>> 0, dest >>> 0, curve >>> 0, active ? 1 : 0, depth);
    // (2) Worklet — topology has no CLAP id so `_mirrorToStore` can't carry it;
    // push the row on the ring so the audible route follows (ticket 0193).
    if (this.ring) this.ring.pushMatrixRow(slot, source, dest, curve, active, depth);
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
        this.ring.pushMatrixRow(slot, r.source, r.dest, r.curve, r.active, r.depth);
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
