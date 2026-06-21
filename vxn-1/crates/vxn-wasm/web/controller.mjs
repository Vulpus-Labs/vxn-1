// Main-thread controller wasm glue (ticket 0044) — the JS half of ADR 0009 §1.
//
// The web port reuses vxn-app's MVC Controller VERBATIM, compiled to wasm
// (vxn-web-controller). This module instantiates that SECOND wasm module on the
// main thread (the engine wasm runs in the worklet), and drives it over the
// narrow C-ABI OPCODE surface the crate exposes — UiEvent in, ViewEvent out,
// tick — never crossing Rust enums. No wasm-bindgen, same raw-instantiate
// approach as the engine (0034).
//
// It is the controller-side owner of the 0039 param SAB: the controller holds
// the AUTHORITATIVE param values in ITS linear memory; this glue mirrors them
// into the store SAB the worklet reads lock-free, and feeds the worklet's
// readback region back through the controller's diff pump (port of vxn-clap
// push_param_diffs) so audio-thread / automation writes echo back as
// ViewEvent::ParamChanged. The two wasm modules never share linear memory; the
// SAB is the dedicated third buffer both map (ADR 0009 §2).
//
// ONE code path, like the rest of E015: the same class is exercised by the Node
// test (controller.test.mjs) and the browser, so the headless proof is the
// byte-for-byte transport the page runs.
//
// SCOPE (0044): instantiate → post a UiEvent opcode → model mutates → value
// lands in the store SAB → drain ViewEvents to a smoke sink; the diff pump
// echoes an audio-thread/automation write back as ParamChanged. OUT of scope:
// the faceplate / full UiEvent↔ViewEvent UI marshalling (E018), Web MIDI
// (E017), IndexedDB presets (E019).

import { TOTAL_PARAMS } from "./param-store.mjs";

const DEFAULT_CONTROLLER_WASM_URL = "./vxn_web_controller.wasm";

// ViewEvent record tags — MUST match vxn-web-controller/src/lib.rs (VE_*).
export const VE_PARAM_CHANGED = 1;
export const VE_KEY_MODE_CHANGED = 2;
export const VE_SPLIT_POINT_CHANGED = 3;
export const VE_EDIT_LAYER_CHANGED = 4;
export const VE_PRESET_LOADED = 5;

// PresetSource discriminants in the VE_PRESET_LOADED record (match lib.rs).
const PRESET_SRC_NONE = 0;
const PRESET_SRC_FACTORY = 1;
const PRESET_SRC_USER = 2;

// KeyMode discriminants (match vxn_app::KeyMode).
export const KEY_MODE_WHOLE = 0;
export const KEY_MODE_DUAL = 1;
export const KEY_MODE_SPLIT = 2;

// Layer discriminants (match vxn_app::Layer).
export const LAYER_UPPER = 0;
export const LAYER_LOWER = 1;

export class WebController {
  // Construct cheaply; instantiate() does the async wasm load. Options:
  //   wasmUrl   : dist-relative URL of the controller wasm (default matches bundle).
  //   wasmBytes : pre-fetched controller bytes; skips the fetch (Node harness).
  //   store     : a ParamStore (param-store.mjs) over the SHARED 0039 SAB. The
  //               controller mirrors its model into store.write()/reads its
  //               readback region. Optional: without it the controller still
  //               runs (smoke), it just doesn't touch the SAB.
  //   onViewEvents : sink called with the decoded ViewEvent array each tick
  //                  (the faceplate bridge is E018; this is the smoke sink).
  //   fetchImpl : fetch seam (defaults to global fetch).
  constructor({
    wasmUrl = DEFAULT_CONTROLLER_WASM_URL,
    wasmBytes = null,
    store = null,
    onViewEvents = () => {},
    fetchImpl = globalThis.fetch,
  } = {}) {
    this.wasmUrl = wasmUrl;
    this.wasmBytes = wasmBytes;
    this.store = store;
    this._onViewEvents = onViewEvents;
    this._fetch = fetchImpl ? fetchImpl.bind(globalThis) : null;

    this.x = null; // instance.exports
    // Worklet-side mirror of what we last mirrored into the SAB, so a tick only
    // writes CHANGED slots (latest-value-wins store, not a stream). NaN-seeded
    // so the first mirror writes every param.
    this._mirrored = new Float32Array(TOTAL_PARAMS).fill(NaN);
  }

  // Instantiate the controller wasm and construct the Rust Controller. After
  // this the opcode surface is live. Idempotent guard: throws on double-init.
  async instantiate() {
    if (this.x) throw new Error("WebController.instantiate() already called");
    const bytes = await this._loadBytes();
    const { instance } = await WebAssembly.instantiate(bytes, {});
    this.x = instance.exports;

    // Param addressing is read FROM the wasm (ADR 0009 §3 / §3 stability): the
    // 165-id layout is owned by vxn-app/params.rs, never hard-coded here. Assert
    // the JS mirror (param-store.mjs TOTAL_PARAMS) agrees so drift is caught at
    // boot rather than as silent corruption.
    const total = this.x.vxnc_total_params();
    if (total !== TOTAL_PARAMS) {
      throw new Error(
        `controller TOTAL_PARAMS ${total} != JS mirror ${TOTAL_PARAMS} — ` +
          `param layout drift (vxn-app vs param-store.mjs)`,
      );
    }
    this.patchCount = this.x.vxnc_patch_count();
    this.globalCount = this.x.vxnc_global_count();
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
  //
  // These post intents into the controller's bounded queue; nothing mutates the
  // model until tick(). A knob drag is begin → setParamNorm… → end, exactly the
  // native gesture bracket.

  beginGesture(clapId) {
    this.x.vxnc_ui_begin_gesture(clapId >>> 0);
  }
  endGesture(clapId) {
    this.x.vxnc_ui_end_gesture(clapId >>> 0);
  }
  setParamNorm(clapId, norm) {
    this.x.vxnc_ui_set_param_norm(clapId >>> 0, norm);
  }
  setParam(clapId, plain) {
    this.x.vxnc_ui_set_param(clapId >>> 0, plain);
  }
  editorReady() {
    this.x.vxnc_ui_editor_ready();
  }

  // Per-synth custom intents (downcast stays inside wasm). Key mode / split
  // point are non-automatable shared state (ADR 0003 §3), NOT params.
  setKeyMode(mode) {
    this.x.vxnc_ui_set_key_mode(mode >>> 0);
  }
  setSplitPoint(note) {
    this.x.vxnc_ui_set_split_point(note >>> 0);
  }
  setEditLayer(layer) {
    this.x.vxnc_ui_set_edit_layer(layer >>> 0);
  }
  resetLayer(layer) {
    this.x.vxnc_ui_reset_layer(layer >>> 0);
  }

  // ---- presets: factory bank (E019 / 0062) --------------------------------
  //
  // The factory bank is baked at build time into `factory.bin` (xtask runs
  // vxn-engine's bake-factory). The page fetches it at boot and hands the bytes
  // here; the controller parses them into its read-only factory store. Returns
  // the preset count (0 on a malformed asset).
  loadFactoryAsset(bytes) {
    const len = bytes.byteLength;
    const ptr = this.x.vxnc_factory_buf_reserve(len >>> 0);
    new Uint8Array(this.x.memory.buffer, ptr, len).set(
      bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes),
    );
    return this.x.vxnc_load_factory(len >>> 0);
  }

  // The corpus JSON (factory groups + user folders) the controller built when
  // the asset loaded — the same shape the native editor feeds
  // `window.__vxn.applyPresetCorpus`. Returns a parsed object.
  corpusJson() {
    const ptr = this.x.vxnc_corpus_json_ptr();
    const len = this.x.vxnc_corpus_json_len();
    if (!len) return { factory: [], user: [] };
    const bytes = new Uint8Array(this.x.memory.buffer, ptr, len);
    return JSON.parse(new TextDecoder().decode(bytes));
  }

  // Load factory preset `index`. The model restore + ParamChanged fan-out +
  // PresetLoaded land on the next tick().
  loadFactory(index) {
    this.x.vxnc_ui_load_factory(index >>> 0);
  }

  // ---- tick: drain queues → mutate model → drain ViewEvents ---------------
  //
  // Call this on each rAF (or after a gesture burst). It (1) ticks the
  // controller, (2) mirrors the model's current values into the store SAB so
  // the worklet applies them, and (3) decodes + dispatches the packed
  // ViewEvents to the sink. Returns the decoded ViewEvent array.
  tick() {
    this.x.vxnc_tick();
    this._mirrorToStore();
    const events = this._drainViewEvents();
    if (events.length) this._onViewEvents(events);
    return events;
  }

  // Read the controller's refreshed value snapshot out of ITS linear memory and
  // write any CHANGED slot into the shared store SAB. memory.buffer is re-viewed
  // each call: a wasm memory growth detaches cached typed arrays (same reason
  // audio-host.mjs re-views per render).
  _mirrorToStore() {
    if (!this.store) return;
    const ptr = this.x.vxnc_param_values_ptr();
    const vals = new Float32Array(this.x.memory.buffer, ptr, TOTAL_PARAMS);
    for (let id = 0; id < TOTAL_PARAMS; id++) {
      const v = vals[id];
      if (v === this._mirrored[id]) continue; // unchanged (NaN seed forces first)
      this._mirrored[id] = v;
      this.store.write(id, v);
    }
  }

  // Force the NEXT tick's mirror pass to re-write EVERY param into the store SAB
  // (re-seed the NaN sentinel). E018 calls this after the audio coordinator's
  // `start()` runs — `_seedStoreFromDefaults` (coordinator.mjs) does a `writeBulk`
  // of engine defaults into the SAME store, which would otherwise clobber any
  // value the controller already mirrored (e.g. an edit made on the unlock
  // gesture). Re-mirroring restores the controller's authoritative values
  // (ADR 0009: controller is the single source of truth on main).
  remirrorStore() {
    this._mirrored.fill(NaN);
  }

  // ---- diff pump: audio-thread readback → ParamChanged --------------------
  //
  // Port of vxn-clap push_param_diffs. Copy the worklet's readback region (the
  // values it actually applied) from the store SAB into the controller's
  // staging buffer, run the pump (routes drift through the controller as
  // HostEvent::ParamAutomation → gesture-gated ParamChanged), then tick to pack
  // + dispatch the emitted ViewEvents. Drives the host-automation echo / meters
  // path. Returns the ViewEvents the pump produced.
  pumpReadback() {
    if (!this.store) return [];
    const ptr = this.x.vxnc_readback_in_ptr();
    const staging = new Float32Array(this.x.memory.buffer, ptr, TOTAL_PARAMS);
    for (let id = 0; id < TOTAL_PARAMS; id++) {
      staging[id] = this.store.readReadback(id);
    }
    this.x.vxnc_pump_readback();
    return this.tick();
  }

  // Decode the packed ViewEvent out-buffer (see vxn-web-controller lib.rs for
  // the record layout). Re-views memory.buffer each call (growth detaches).
  _drainViewEvents() {
    const ptr = this.x.vxnc_view_out_ptr();
    const len = this.x.vxnc_view_out_len();
    const view = new DataView(this.x.memory.buffer, ptr, len);
    const dec = new TextDecoder();
    let off = 0;
    const count = view.getUint32(off, true);
    off += 4;
    const out = [];
    for (let i = 0; i < count; i++) {
      const tag = view.getUint32(off, true);
      off += 4;
      switch (tag) {
        case VE_PARAM_CHANGED: {
          const id = view.getUint32(off, true);
          off += 4;
          const plain = view.getFloat32(off, true);
          off += 4;
          const norm = view.getFloat32(off, true);
          off += 4;
          const dlen = view.getUint32(off, true);
          off += 4;
          const bytes = new Uint8Array(this.x.memory.buffer, ptr + off, dlen);
          const display = dec.decode(bytes);
          off += dlen;
          out.push({ type: "ParamChanged", id, plain, norm, display });
          break;
        }
        case VE_KEY_MODE_CHANGED: {
          const mode = view.getUint32(off, true);
          off += 4;
          out.push({ type: "KeyModeChanged", mode });
          break;
        }
        case VE_SPLIT_POINT_CHANGED: {
          const note = view.getUint32(off, true);
          off += 4;
          out.push({ type: "SplitPointChanged", note });
          break;
        }
        case VE_EDIT_LAYER_CHANGED: {
          const layer = view.getUint32(off, true);
          off += 4;
          out.push({ type: "EditLayerChanged", layer });
          break;
        }
        case VE_PRESET_LOADED: {
          const nameLen = view.getUint32(off, true);
          off += 4;
          const name = dec.decode(new Uint8Array(this.x.memory.buffer, ptr + off, nameLen));
          off += nameLen;
          const srcKind = view.getUint32(off, true);
          off += 4;
          let source = null;
          if (srcKind === PRESET_SRC_FACTORY) {
            const index = view.getUint32(off, true);
            off += 4;
            source = { kind: "factory", index };
          } else if (srcKind === PRESET_SRC_USER) {
            const pathLen = view.getUint32(off, true);
            off += 4;
            const path = dec.decode(new Uint8Array(this.x.memory.buffer, ptr + off, pathLen));
            off += pathLen;
            source = { kind: "user", path };
          }
          const warnCount = view.getUint32(off, true);
          off += 4;
          const warnings = [];
          for (let w = 0; w < warnCount; w++) {
            const wlen = view.getUint32(off, true);
            off += 4;
            warnings.push(dec.decode(new Uint8Array(this.x.memory.buffer, ptr + off, wlen)));
            off += wlen;
          }
          out.push({ type: "PresetLoaded", name, source, warnings });
          break;
        }
        default:
          // Unknown tag — the packed stream is self-describing only for known
          // tags; an unknown one means JS/Rust drift. Fail loud.
          throw new Error(`controller: unknown ViewEvent tag ${tag}`);
      }
    }
    return out;
  }

  // Tear down the Rust controller (page teardown / re-init).
  destroy() {
    if (this.x) {
      try {
        this.x.vxnc_destroy();
      } catch {}
    }
  }
}
