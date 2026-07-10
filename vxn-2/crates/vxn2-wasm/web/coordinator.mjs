// Main-thread coordinator (ticket 0156) — the web analogue of vxn2-clap's
// audio-side bootstrap: the half that INSTANTIATES the E030 transport and feeds
// it. Where host-runner.mjs is the worklet-side render+lifecycle owner, this is
// the main-side owner: it creates the AudioContext, loads the worklet, allocates
// the two shared SABs, hands the worklet its wasm bytes, and exposes the producer
// surface (notes/params) that writes into those SABs.
//
// Ported from vxn-1's `vxn-wasm/web/coordinator`. vxn-2 changes: no key-mode/
// split shared state (dropped), the flat 209-param store, and the vxn2-* worklet
// / wasm names. The AudioContext lifecycle state machine (autoplay unlock,
// suspend/resume + voice flush, device change, teardown, CPU meter) ports
// verbatim.

import { createRingSAB, EventRing, DEFAULT_CAPACITY } from "./event-ring.mjs";
import {
  createParamSAB,
  ParamStore,
  newLastSeen,
  pollDiffs,
  TOTAL_PARAMS,
} from "./param-store.mjs";

// The worklet module is `vxn2-processor.js`; the processor registers as this
// name. Defaults match the shipped bundle so the browser path is zero-config.
const PROCESSOR_NAME = "vxn2-host-processor";
const DEFAULT_WORKLET_URL = "./vxn2-processor.js";
const DEFAULT_WASM_URL = "./vxn2_wasm.wasm";

// Apple WebKit (Safari) runs the AudioWorklet with ~one quantum of output
// headroom and no FP flush-to-zero — almost no slack, so any per-quantum render-
// thread work risks a late quantum → audible glitches. The CPU meter's per-
// quantum timing + periodic postMessage is exactly such work (and its date-clock
// reading on Safari was junk anyway), so we disable it there. Chromium keeps it.
function isAppleWebKit() {
  if (typeof navigator === "undefined") return false; // Node harness
  const ua = navigator.userAgent || "";
  const vendor = navigator.vendor || "";
  return /Apple/.test(vendor) && !/CriOS|FxiOS|EdgiOS|Chrome|Chromium|Edg|Android/.test(ua);
}

export class WebHost {
  // Construct cheaply (no audio side-effects); the AudioContext is created in
  // start(), which MUST be called from a user-gesture handler (autoplay policy).
  // Options mirror vxn-1's: wasmUrl / workletUrl / wasmBytes / capacity /
  // onReady / onTrap / onState / onCpu and the AudioContext / AudioWorkletNode /
  // fetch / mediaDevices injection seams for headless testing.
  constructor({
    wasmUrl = DEFAULT_WASM_URL,
    workletUrl = DEFAULT_WORKLET_URL,
    wasmBytes = null,
    capacity = DEFAULT_CAPACITY,
    onReady = () => {},
    onTrap = () => {},
    onState = () => {},
    onCpu = () => {},
    AudioContextClass = globalThis.AudioContext,
    AudioWorkletNodeClass = globalThis.AudioWorkletNode,
    fetchImpl = globalThis.fetch,
    mediaDevices = globalThis.navigator ? globalThis.navigator.mediaDevices : null,
  } = {}) {
    this.wasmUrl = wasmUrl;
    this.workletUrl = workletUrl;
    this.wasmBytes = wasmBytes;
    this.capacity = capacity;
    this._onReady = onReady;
    this._onTrap = onTrap;
    this._onState = onState;
    this._onCpu = onCpu;
    this._AudioContext = AudioContextClass;
    this._AudioWorkletNode = AudioWorkletNodeClass;
    this._fetch = fetchImpl ? fetchImpl.bind(globalThis) : null;
    this._mediaDevices = mediaDevices || null;

    // Allocate the transport SABs up front (cheap, no audio context needed) so
    // the producer surface is usable the instant the WebHost exists — events
    // written before `ready` buffer in the ring and apply on the first live
    // quantum (the silence-until-ready contract host-runner.mjs guarantees).
    this.ringSab = createRingSAB(this.capacity);
    this.storeSab = createParamSAB();
    this.ring = new EventRing(this.ringSab, this.capacity); // producer side
    this.store = new ParamStore(this.storeSab); // controller side
    this._lastSeen = newLastSeen(); // readback diff mirror (0157 consumes)

    this.ctx = null;
    this.node = null;
    this.ready = false; // worklet posted `ready`
    this.trapCount = 0;

    // ---- gate / lifecycle state machine -----------------------------------
    //   "idle" | "starting" | "running" | "suspended" | "closed"
    // gateState is the single source of truth the UI hook renders from (onState).
    this.gateState = "idle";
    this._statechange = null;
    this._devicechange = null;
    this._tornDown = false;

    // Resolves when the worklet reaches "audio live". Await it to gate UI that
    // needs a sounding engine; start() does NOT block on it.
    this.whenReady = new Promise((res) => (this._resolveReady = res));
  }

  _setGate(state) {
    if (this.gateState === state) return;
    this.gateState = state;
    try {
      this._onState(state);
    } catch {}
  }

  // Boot to "audio live". Call from a user-gesture handler. Creates the context,
  // loads the worklet module + fetches the wasm IN PARALLEL, then constructs the
  // node over our SABs and resumes.
  async start() {
    if (this._tornDown) throw new Error("WebHost torn down; construct a fresh one");
    if (this.ctx) throw new Error("WebHost.start() already called");
    if (!this._AudioContext) throw new Error("no AudioContext available");

    this._setGate("starting");
    this.ctx = new this._AudioContext();

    this._attachStateChange();
    this._attachDeviceChange();

    // Worklet scope can't fetch; the main thread fetches the wasm and hands the
    // bytes through processorOptions. addModule resolves the worklet's static ESM
    // imports (host-runner/audio-host/ring/store).
    const [wasmBytes] = await Promise.all([
      this._loadWasmBytes(),
      this.ctx.audioWorklet.addModule(this.workletUrl),
    ]);
    this.wasmBytes = wasmBytes;

    // Seed the param store with the engine's defaults BEFORE the worklet starts.
    // The store's slots are zero-initialised and the worklet's first-quantum fold
    // (NaN-seeded workletSeen) pushes ALL params — so an unseeded store would
    // clobber every param to 0.0 and silence the voice. `SharedParams::new` seeds
    // real defaults on the engine side, so we snapshot them off a throwaway engine
    // instance here (the controller wasm, 0157, later becomes the authoritative
    // seeder by mirroring its model into the store).
    await this._seedStoreFromDefaults(wasmBytes);

    // Construct the node over our SABs. sampleRate is NOT passed: the worklet
    // reads it from its own global, which is the context rate. capacity MUST
    // match our ring. Disable the render-load meter on Safari.
    this._cpuMeterEnabled = !isAppleWebKit();
    this.node = new this._AudioWorkletNode(this.ctx, PROCESSOR_NAME, {
      numberOfInputs: 0,
      numberOfOutputs: 1,
      outputChannelCount: [2],
      processorOptions: {
        wasmBytes,
        ringSab: this.ringSab,
        storeSab: this.storeSab,
        capacity: this.capacity,
        cpuMeter: this._cpuMeterEnabled,
      },
    });

    this.node.port.onmessage = (e) => this._onPortMessage(e.data);
    this.node.connect(this.ctx.destination);
    if (!this._cpuMeterEnabled) this._onCpu(null, null);

    // Autoplay unlock: the context starts `suspended`; resume() MUST be inside a
    // user-gesture call stack (start()'s contract).
    await this.ctx.resume();
    this._setGate(this.ctx.state === "running" ? "running" : "suspended");
    return this;
  }

  // ---- suspend / resume ---------------------------------------------------

  _attachStateChange() {
    if (!this.ctx || typeof this.ctx.addEventListener !== "function") return;
    this._statechange = () => this._onStateChange();
    this.ctx.addEventListener("statechange", this._statechange);
  }
  _detachStateChange() {
    if (this.ctx && this._statechange && typeof this.ctx.removeEventListener === "function") {
      this.ctx.removeEventListener("statechange", this._statechange);
    }
    this._statechange = null;
  }

  // Mirror an AudioContext.statechange into the gate machine. On resume from a
  // suspend, post the worklet a `reset` so a long suspend can't leave stuck notes
  // (all-notes-off WITHOUT touching the ring or store — transport state intact).
  // We deliberately do NOT flush on SUSPEND (nothing renders while suspended).
  _onStateChange() {
    if (!this.ctx) return;
    switch (this.ctx.state) {
      case "running":
        if (this.gateState === "suspended") this._flushVoicesOnResume();
        this._setGate("running");
        break;
      case "suspended":
        this._setGate("suspended");
        break;
      case "closed":
        this._setGate("closed");
        break;
      default:
        break;
    }
  }

  _flushVoicesOnResume() {
    try {
      this.node?.port.postMessage({ type: "reset" });
    } catch {}
  }

  async suspend() {
    if (this.ctx && typeof this.ctx.suspend === "function" && this.ctx.state === "running") {
      await this.ctx.suspend();
      if (!this._statechange && this.ctx.state === "suspended") this._setGate("suspended");
    }
  }

  async resume() {
    if (this.ctx && typeof this.ctx.resume === "function" && this.ctx.state === "suspended") {
      const wasSuspended = this.gateState === "suspended";
      await this.ctx.resume();
      if (!this._statechange && this.ctx.state === "running") {
        if (wasSuspended) this._flushVoicesOnResume();
        this._setGate("running");
      }
    }
  }

  async _loadWasmBytes() {
    if (this.wasmBytes) return this.wasmBytes; // harness / pre-fetched
    if (!this._fetch) throw new Error("no fetch and no wasmBytes provided");
    const resp = await this._fetch(this.wasmUrl);
    if (!resp.ok) throw new Error(`wasm fetch failed: ${resp.status}`);
    return resp.arrayBuffer();
  }

  // Snapshot the engine's default param values off a throwaway main-thread
  // instance and bulk-write them into the store, so the worklet's first fold is a
  // no-op against the engine rather than a zeroing pass. The instance is discarded
  // immediately; only its defaults survive (in the SAB).
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
        // The runner already caught it and kicked async recovery; we just observe.
        this.ready = false;
        this.trapCount = m.count != null ? m.count : this.trapCount + 1;
        this._onTrap(m.message, this.trapCount);
        break;
      case "cpu":
        if (m.clock && !this._cpuClockLogged) {
          console.info(`vxn2: CPU meter clock = ${m.clock}`);
          this._cpuClockLogged = true;
        }
        this._onCpu(m.load, m.peak);
        break;
      default:
        break;
    }
  }

  // ---- device change ------------------------------------------------------

  _attachDeviceChange() {
    const md = this._mediaDevices;
    if (!md || typeof md.addEventListener !== "function") return;
    this._devicechange = () => this._onDeviceChange();
    md.addEventListener("devicechange", this._devicechange);
  }
  _detachDeviceChange() {
    const md = this._mediaDevices;
    if (md && this._devicechange && typeof md.removeEventListener === "function") {
      md.removeEventListener("devicechange", this._devicechange);
    }
    this._devicechange = null;
  }

  // Default: most device changes don't move the context rate, so the graph stays
  // up untouched. Apps that follow a specific device call setSink(); apps that
  // detect a rate change call rebuild(). Kept as a hook for the faceplate (0157).
  _onDeviceChange() {}

  // Re-route output to a specific device WITHOUT rebuilding the graph. Resolves
  // true if the sink moved, false if setSinkId is unavailable.
  async setSink(sinkId) {
    if (this.ctx && typeof this.ctx.setSinkId === "function") {
      await this.ctx.setSinkId(sinkId);
      return true;
    }
    return false;
  }

  // Rebuild the graph at a (possibly new) sample rate, reusing the SAME SABs so
  // transport/param state survives. Tears the current context/node down (worklet
  // `destroy`, disconnect, close) but KEEPS the ring/store SABs, then re-runs
  // start(). Must be called from a user gesture (it resume()s the new context).
  async rebuild() {
    if (this._tornDown) throw new Error("WebHost torn down; construct a fresh one");
    await this._disposeGraph();
    this.ready = false;
    this.whenReady = new Promise((res) => (this._resolveReady = res));
    this._setGate("idle");
    return this.start();
  }

  // ---- producer surface: notes/gestures over the ring --------------------
  //
  // The main-thread half of the SPSC ring; the worklet drains them in its render
  // loop. All return the ring's block-writer boolean (false iff momentarily full).
  // `offset` is the sample offset within the next quantum (0..Q-1); 0 == ASAP.

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
  // Params travel on the latest-value-wins store, not the ring: the worklet folds
  // changed values into the engine block-start each quantum. Edits and bulk preset
  // loads both land here.

  setParam(id, value) {
    this.store.write(id, value);
  }
  setParamsBulk(values) {
    this.store.writeBulk(values); // length-TOTAL_PARAMS plain values (preset load)
  }
  readParam(id) {
    return this.store.read(id);
  }

  // Poll the audio->main readback region for params the audio thread changed
  // (host-automation echo / modulation). Returns ParamChanged-equivalent records
  // since the last poll; [] when nothing drifted. 0157's bridge drives this.
  pollParamDiffs() {
    return pollDiffs(this.store, this._lastSeen);
  }

  // ---- teardown -----------------------------------------------------------

  async _disposeGraph() {
    this._detachDeviceChange();
    this._detachStateChange();
    if (this.node) {
      try {
        this.node.port.postMessage({ type: "destroy" });
      } catch {}
      try {
        this.node.disconnect();
      } catch {}
      try {
        this.node.port.onmessage = null;
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

  // Full teardown: dispose the graph AND drop the SAB references so nothing leaks.
  // After this the WebHost is spent (gateState "closed"); a fresh boot needs a NEW
  // WebHost. The worklet nulls its own SAB refs on `destroy`, so once we drop ours
  // the SABs are unreferenced on both threads and collectable.
  async teardown() {
    await this._disposeGraph();
    this.ring = null;
    this.store = null;
    this.ringSab = null;
    this.storeSab = null;
    this._tornDown = true;
    this._setGate("closed");
  }

  async dispose() {
    return this.teardown();
  }
}
