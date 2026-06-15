// Main-thread coordinator (ticket 0042) — the web analogue of vxn-clap's
// audio-side bootstrap: the half that INSTANTIATES the E015 transport and feeds
// it. Where host-runner.mjs (0040) is the worklet-side render+lifecycle owner,
// this is the main-side owner: it creates the AudioContext, loads the worklet,
// allocates the two shared SABs, hands the worklet its wasm bytes, and exposes
// the producer surface (notes/params) that writes into those SABs.
//
// ONE code path, like the rest of E015: the SAME class drives both the browser
// (real AudioContext + AudioWorkletNode globals) and the Node harness
// (harness-0042.mjs, which injects a fake context whose node runs the real
// WorkletHostRunner over the same SABs). So what we prove headlessly is the
// byte-for-byte boot + transport the browser runs.
//
// SCOPE (ticket 0042): construct → "audio live" (worklet posts `ready`) →
// note/param from main reaches the audio thread → trap surfaced. OUT of scope:
// autoplay/suspend/resume/devicechange/teardown policy (0043), the UiEvent
// marshalling + controller wasm (0044), COOP/COEP serving (0045). This class
// gets to first sound; 0043 layers the lifecycle state machine on top.

import { createRingSAB, EventRing, DEFAULT_CAPACITY } from "./event-ring.mjs";
import {
  createParamSAB,
  ParamStore,
  newLastSeen,
  pollDiffs,
  TOTAL_PARAMS,
} from "./param-store.mjs";

// The worklet registers itself under this name (vxn-processor-0038.js); in dist
// the module file is `vxn-processor.js` (xtask web renames it). Defaults match
// the shipped bundle so the browser path is zero-config.
const PROCESSOR_NAME = "vxn-host-processor";
const DEFAULT_WORKLET_URL = "./vxn-processor.js";
const DEFAULT_WASM_URL = "./vxn_wasm.wasm";

export class WebHost {
  // Construct cheaply (no audio side-effects); the AudioContext is created in
  // start(), which MUST be called from a user-gesture handler (autoplay policy —
  // the gesture/unlock machinery itself is 0043). Options:
  //   wasmUrl / workletUrl : dist-relative URLs (defaults match the bundle).
  //   wasmBytes            : pre-fetched engine bytes; skips the wasmUrl fetch
  //                          (the Node harness passes this — no fetch in Node).
  //   capacity             : event-ring slots (power of two). Main and worklet
  //                          MUST agree; we pass it through processorOptions.
  //   onReady / onTrap     : lifecycle observers. onTrap(message, count) fires
  //                          on every render-thread trap the runner catches.
  //   AudioContextClass /
  //   AudioWorkletNodeClass: injection seams for headless testing; default to
  //                          the browser globals.
  //   fetchImpl            : fetch seam (defaults to global fetch).
  constructor({
    wasmUrl = DEFAULT_WASM_URL,
    workletUrl = DEFAULT_WORKLET_URL,
    wasmBytes = null,
    capacity = DEFAULT_CAPACITY,
    onReady = () => {},
    onTrap = () => {},
    AudioContextClass = globalThis.AudioContext,
    AudioWorkletNodeClass = globalThis.AudioWorkletNode,
    fetchImpl = globalThis.fetch,
  } = {}) {
    this.wasmUrl = wasmUrl;
    this.workletUrl = workletUrl;
    this.wasmBytes = wasmBytes;
    this.capacity = capacity;
    this._onReady = onReady;
    this._onTrap = onTrap;
    this._AudioContext = AudioContextClass;
    this._AudioWorkletNode = AudioWorkletNodeClass;
    this._fetch = fetchImpl ? fetchImpl.bind(globalThis) : null;

    // Allocate the transport SABs up front (cheap, no audio context needed) so
    // the producer surface is usable the instant the WebHost exists — events
    // written before `ready` buffer in the ring and apply on the first live
    // quantum (the silence-until-ready contract host-runner.mjs guarantees).
    this.ringSab = createRingSAB(this.capacity);
    this.storeSab = createParamSAB();
    this.ring = new EventRing(this.ringSab, this.capacity); // producer side
    this.store = new ParamStore(this.storeSab); // controller side
    this._lastSeen = newLastSeen(); // readback diff mirror (E018 consumes)

    this.ctx = null;
    this.node = null;
    this.ready = false; // worklet posted `ready`
    this.trapCount = 0;
    // Resolves when the worklet reaches "audio live". Await it to gate UI that
    // needs a sounding engine; start() does NOT block on it (resume can settle
    // before the async wasm instantiate).
    this.whenReady = new Promise((res) => (this._resolveReady = res));
  }

  // Boot to "audio live". Call from a user-gesture handler. Creates the context,
  // loads the worklet module + fetches the wasm IN PARALLEL (independent), then
  // constructs the node over our SABs and resumes. Resolves once the graph is
  // connected and resume() returns; the worklet's own `ready` (async wasm
  // instantiate) arrives shortly after via whenReady / onReady.
  async start() {
    if (this.ctx) throw new Error("WebHost.start() already called");
    if (!this._AudioContext) throw new Error("no AudioContext available");

    this.ctx = new this._AudioContext();

    // Worklet scope can't fetch; the main thread fetches the wasm and hands the
    // bytes through processorOptions (the 0034 pattern). addModule resolves the
    // worklet's static ESM imports (host-runner/audio-host/ring/store).
    const [wasmBytes] = await Promise.all([
      this._loadWasmBytes(),
      this.ctx.audioWorklet.addModule(this.workletUrl),
    ]);
    this.wasmBytes = wasmBytes;

    // Seed the param store with the engine's defaults BEFORE the worklet starts.
    // The store's slots are zero-initialised and the worklet's first-quantum fold
    // (NaN-seeded workletSeen) applies ALL 165 — so an unseeded store would clobber
    // every param to 0.0 and silence the voice. The 0039 store contract is "the
    // controller seeds the store before the worklet starts"; until the controller
    // wasm (0044) owns defaults via vxn-app, we snapshot them off a throwaway
    // engine instance here. Done before node construction → populated before the
    // worklet ever reads it.
    await this._seedStoreFromDefaults(wasmBytes);

    // Construct the node over our SABs. sampleRate is NOT passed: the worklet
    // reads it from its own global (vxn-processor-0038.js), which is the context
    // rate — passing it would risk a mismatch. capacity MUST match our ring.
    this.node = new this._AudioWorkletNode(this.ctx, PROCESSOR_NAME, {
      numberOfInputs: 0,
      numberOfOutputs: 1,
      outputChannelCount: [2],
      processorOptions: {
        wasmBytes,
        ringSab: this.ringSab,
        storeSab: this.storeSab,
        capacity: this.capacity,
      },
    });

    // Surface the worklet's lifecycle port messages. ready/trap are posted by
    // the runner (host-runner.mjs onReady/onTrap → vxn-processor port).
    this.node.port.onmessage = (e) => this._onPortMessage(e.data);

    this.node.connect(this.ctx.destination);
    await this.ctx.resume();
    return this;
  }

  async _loadWasmBytes() {
    if (this.wasmBytes) return this.wasmBytes; // harness / pre-fetched
    if (!this._fetch) throw new Error("no fetch and no wasmBytes provided");
    const resp = await this._fetch(this.wasmUrl);
    if (!resp.ok) throw new Error(`wasm fetch failed: ${resp.status}`);
    return resp.arrayBuffer();
  }

  // Snapshot the engine's 165 default param values off a throwaway main-thread
  // instance and bulk-write them into the store, so the worklet's first fold is
  // a no-op against the engine rather than a zeroing pass. The instance is
  // discarded immediately; only its defaults survive (in the SAB).
  async _seedStoreFromDefaults(wasmBytes) {
    const { instance } = await WebAssembly.instantiate(wasmBytes, {});
    const x = instance.exports;
    const sr = this.ctx ? this.ctx.sampleRate : 48000;
    const h = x.vxn_host_new(sr);
    const vals = new Float32Array(TOTAL_PARAMS);
    for (let id = 0; id < TOTAL_PARAMS; id++) vals[id] = x.vxn_host_get_param(h, id);
    this.store.writeBulk(vals);
    x.vxn_host_destroy(h);
  }

  _onPortMessage(m) {
    switch (m && m.type) {
      case "ready":
        this.ready = true;
        this._resolveReady(this);
        this._onReady();
        break;
      case "trap":
        // The runner already caught it and kicked async recovery; we just
        // observe. ready flips back true on the next `ready` after re-init.
        this.ready = false;
        this.trapCount = m.count != null ? m.count : this.trapCount + 1;
        this._onTrap(m.message, this.trapCount);
        break;
      default:
        break;
    }
  }

  // ---- producer surface: notes/gestures over the ring --------------------
  //
  // These are the main-thread half of the SPSC ring; the worklet drains them in
  // its render loop. All return the EventRing's block-writer boolean (false iff
  // the ring is momentarily full — the caller can retry; in practice the ring is
  // sized so this never fires). `offset` is the sample offset within the next
  // quantum (0..Q-1) for sample-accurate placement; 0 == "as soon as possible".

  noteOn(note, velocity = 1, offset = 0) {
    return this.ring.pushNoteOn(offset, note, velocity);
  }
  noteOff(note, offset = 0) {
    return this.ring.pushNoteOff(offset, note);
  }
  pitchBend(value, offset = 0) {
    return this.ring.pushPitchBend(offset, value);
  }
  modWheel(value, offset = 0) {
    return this.ring.pushModWheel(offset, value);
  }
  sustain(on, offset = 0) {
    return this.ring.pushSustain(offset, on);
  }

  // ---- producer surface: params over the store ---------------------------
  //
  // Params travel on the latest-value-wins store (0039), not the ring: the
  // worklet folds changed values into the engine block-start each quantum. Edits
  // and bulk preset loads both land here.

  setParam(id, value) {
    this.store.write(id, value);
  }
  setParamsBulk(values) {
    this.store.writeBulk(values); // length-165 plain values (preset load)
  }
  readParam(id) {
    return this.store.read(id);
  }

  // Poll the audio->main readback region for params the audio thread changed
  // (host-automation echo / modulation). Returns ParamChanged-equivalent records
  // since the last poll; [] when nothing drifted. E018's UI bridge drives this
  // on rAF — exposed here so the readback plumbing is reachable from main.
  pollParamDiffs() {
    return pollDiffs(this.store, this._lastSeen);
  }

  // ---- non-automatable shared state (ADR 0003 §3) ------------------------
  //
  // Key mode / split point are NOT params and never occupy a store slot; they
  // travel out-of-band on the worklet port. Honoured even if sent before ready
  // (the runner buffers and applies on instantiate).

  setKeyMode(mode) {
    this.node?.port.postMessage({ type: "keyMode", value: mode & 0xff });
  }
  setSplitPoint(note) {
    this.node?.port.postMessage({ type: "splitPoint", value: note & 0xff });
  }

  // Minimal teardown so a re-init in this session doesn't leak the node/context.
  // Full lifecycle (suspend/resume/devicechange) is 0043; this is just enough to
  // dispose the graph. Posts `destroy` so the worklet frees the engine, then
  // disconnects and closes the context.
  async dispose() {
    if (this.node) {
      try {
        this.node.port.postMessage({ type: "destroy" });
      } catch {}
      try {
        this.node.disconnect();
      } catch {}
      this.node = null;
    }
    if (this.ctx) {
      try {
        await this.ctx.close();
      } catch {}
      this.ctx = null;
    }
    this.ready = false;
  }
}
