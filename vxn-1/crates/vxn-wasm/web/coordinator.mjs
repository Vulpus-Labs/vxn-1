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
// note/param from main reaches the audio thread → trap surfaced. The UiEvent
// marshalling + controller wasm (0044) and COOP/COEP serving (0045) stay out.
//
// 0043 layers the AudioContext lifecycle state machine ON TOP of this class
// (autoplay unlock, suspend/resume, device change, teardown). It is the
// main-thread complement to the worklet-side 0040 lifecycle (host-runner.mjs):
//
//   - Autoplay unlock: the context is created `suspended`; start() must be
//     called from a user gesture and drives it to `running`. `gateState`
//     surfaces the unlock progress to the UI ("Start audio" → live).
//   - Suspend/resume: an AudioContext `statechange` (tab backgrounded / manual
//     suspend) is observed; on resume we post the worklet a `reset` (0040) so a
//     long suspend can't leave stuck notes — see VOICE-FLUSH note on resume().
//   - Device change: navigator.mediaDevices `devicechange` re-routes output via
//     setSinkId where supported WITHOUT rebuilding the graph; a sample-rate
//     change needs a new context, so rebuild() tears down and re-boots over the
//     SAME SABs (transport state survives; the engine rebuilds at the new rate
//     through the worklet's 0040 sampleRate path).
//   - Teardown: teardown() posts the worklet `destroy` (0040), detaches the
//     node, closes the context, and drops the SAB refs — no leaks across a
//     teardown/rebuild cycle; a fresh WebHost afterwards boots clean.

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
  //   onState              : gate/lifecycle observer. onState(gateState) fires
  //                          whenever the gate state machine transitions (idle →
  //                          starting → running → suspended → closed). The UI
  //                          (a "Start audio" button now, faceplate in E018)
  //                          renders off this.
  //   AudioContextClass /
  //   AudioWorkletNodeClass: injection seams for headless testing; default to
  //                          the browser globals.
  //   fetchImpl            : fetch seam (defaults to global fetch).
  //   mediaDevices         : navigator.mediaDevices seam for the devicechange
  //                          listener (defaults to the browser global; null to
  //                          disable device-change handling, e.g. in Node).
  constructor({
    wasmUrl = DEFAULT_WASM_URL,
    workletUrl = DEFAULT_WORKLET_URL,
    wasmBytes = null,
    capacity = DEFAULT_CAPACITY,
    onReady = () => {},
    onTrap = () => {},
    onState = () => {},
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
    this._lastSeen = newLastSeen(); // readback diff mirror (E018 consumes)

    this.ctx = null;
    this.node = null;
    this.ready = false; // worklet posted `ready`
    this.trapCount = 0;

    // ---- 0043 gate / lifecycle state machine ------------------------------
    //
    //   "idle"      : constructed, no context yet (pre-gesture).
    //   "starting"  : start() in flight (context created, worklet loading).
    //   "running"   : context `running`; audio is being driven.
    //   "suspended" : context `suspended` (tab backgrounded / manual suspend);
    //                 the audio clock is stopped, so the ring isn't drained.
    //   "closed"    : torn down; a fresh WebHost is required to boot again.
    //
    // gateState is the single source of truth the UI hook renders from (onState).
    // The browser's own AudioContext.state is the underlying truth for the live
    // states; we mirror its statechange transitions here and never drive it out
    // of band.
    this.gateState = "idle";
    this._statechange = null; // bound statechange listener (for removal)
    this._devicechange = null; // bound devicechange listener (for removal)
    this._tornDown = false; // teardown() ran — start()/rebuild() refuse

    // Resolves when the worklet reaches "audio live". Await it to gate UI that
    // needs a sounding engine; start() does NOT block on it (resume can settle
    // before the async wasm instantiate).
    this.whenReady = new Promise((res) => (this._resolveReady = res));
  }

  // Drive the gate state machine and notify the UI hook. Idempotent on no-op
  // transitions (we still skip the observer call when nothing changed).
  _setGate(state) {
    if (this.gateState === state) return;
    this.gateState = state;
    try {
      this._onState(state);
    } catch {}
  }

  // Boot to "audio live". Call from a user-gesture handler. Creates the context,
  // loads the worklet module + fetches the wasm IN PARALLEL (independent), then
  // constructs the node over our SABs and resumes. Resolves once the graph is
  // connected and resume() returns; the worklet's own `ready` (async wasm
  // instantiate) arrives shortly after via whenReady / onReady.
  async start() {
    if (this._tornDown) throw new Error("WebHost torn down; construct a fresh one");
    if (this.ctx) throw new Error("WebHost.start() already called");
    if (!this._AudioContext) throw new Error("no AudioContext available");

    this._setGate("starting");
    this.ctx = new this._AudioContext();

    // Observe the context's own lifecycle. The browser flips state on tab
    // background / OS audio interruption / manual suspend; we mirror those into
    // gateState and flush sounding voices on resume (see _onStateChange).
    this._attachStateChange();
    this._attachDeviceChange();

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
    // Autoplay unlock: the context starts `suspended`; resume() MUST be inside a
    // user-gesture call stack (start()'s contract). On success it reaches
    // `running`; the statechange listener also fires and sets the gate, but we
    // set it here too so callers that don't get a synchronous statechange (the
    // Node harness) still observe "running".
    await this.ctx.resume();
    this._setGate(this.ctx.state === "running" ? "running" : "suspended");
    return this;
  }

  // ---- 0043 suspend / resume ---------------------------------------------

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

  // Mirror an AudioContext.statechange into the gate machine.
  //
  // VOICE-FLUSH DECISION (resume): the MAIN thread owns the flush, not the
  // worklet. A suspended context's audio clock is stopped, so process() doesn't
  // run and any voices that were sounding when audio stopped would otherwise
  // resume mid-note (or hang if their note-off was eaten by an app that cleared
  // its key state while backgrounded). On resume we post the worklet a `reset`
  // (the 0040 host-runner reset() → vxn_host_reset → Synth::reset), which clears
  // sounding voices WITHOUT touching the ring or store — transport state (ring
  // read/write indices, param values) is intact, only live voices are dropped.
  // We deliberately do NOT flush on SUSPEND: there's nothing to render while
  // suspended, and resetting then would be a no-op the resume path repeats.
  _onStateChange() {
    if (!this.ctx) return;
    switch (this.ctx.state) {
      case "running":
        // Came back from suspend (or first reach). Drop any voices that were
        // mid-flight when the clock stopped so resume can't leave stuck notes.
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
    // 0040 reset path: all-notes-off without disturbing ring/store.
    try {
      this.node?.port.postMessage({ type: "reset" });
    } catch {}
  }

  // Programmatic suspend (e.g. an app "pause" button). The browser also
  // suspends on its own (tab background); both land in _onStateChange when the
  // context emits statechange. We only mirror the gate by hand when there is NO
  // statechange listener (the Node fake context), to avoid double-driving it.
  async suspend() {
    if (this.ctx && typeof this.ctx.suspend === "function" && this.ctx.state === "running") {
      await this.ctx.suspend();
      if (!this._statechange && this.ctx.state === "suspended") this._setGate("suspended");
    }
  }

  // Programmatic resume back to `running`. Must be reachable from a user gesture
  // if the suspend was an autoplay/background suspend the browser is gating. When
  // a statechange listener is active it owns the gate + voice flush (so we don't
  // double-flush); without one (Node fake) we mirror + flush here.
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

  // ---- 0043 device change -------------------------------------------------
  //
  // Two cases, decided by whether the sample rate moves:
  //
  //   (a) Same rate, different output device. Re-route in place via the context
  //       sinkId (AudioContext.setSinkId, where supported) WITHOUT touching the
  //       graph, SABs, or the worklet — the engine keeps rendering, only the
  //       output sink moves. Falls back to a no-op where setSinkId is absent
  //       (the default-device follow is then the browser's job).
  //   (b) Rate change. An AudioContext's sampleRate is immutable, so a new
  //       default device at a different rate needs a NEW context. rebuild()
  //       tears down and re-boots over the SAME SABs, so transport state (ring
  //       indices, params) survives and the engine rebuilds at the new rate via
  //       the worklet's 0040 sampleRate path. rebuild() must be driven from a
  //       gesture-safe context (it resume()s a fresh context).

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

  // Default handler: most device changes don't move the context rate, so the
  // graph stays up untouched. Apps that want to follow a specific device call
  // setSink(); apps that detect a rate change call rebuild(). Kept as a hook so
  // the faceplate (E018) can override the policy without re-listening.
  _onDeviceChange() {
    // No structural action by default — the context follows the system default
    // sink, and a rate change (rare) is surfaced for the app to call rebuild().
  }

  // Re-route output to a specific device WITHOUT rebuilding the graph (case a).
  // Resolves true if the sink moved, false if setSinkId is unavailable.
  async setSink(sinkId) {
    if (this.ctx && typeof this.ctx.setSinkId === "function") {
      await this.ctx.setSinkId(sinkId);
      return true;
    }
    return false;
  }

  // Rebuild the graph at a (possibly new) sample rate, reusing the SAME SABs so
  // transport/param state survives (case b). Tears the current context/node down
  // (worklet `destroy`, disconnect, close) but KEEPS the ring/store SABs, then
  // re-runs start() so a fresh context/worklet maps the same SABs. Must be
  // called from a user gesture (it resume()s the new context). The ring's read
  // index is wherever the old worklet left it, so no events are lost; any voices
  // sounding at teardown are gone with the old engine (a clean break, not a
  // stuck note). Returns this once the rebuilt graph's start() resolves.
  async rebuild() {
    if (this._tornDown) throw new Error("WebHost torn down; construct a fresh one");
    await this._disposeGraph(); // destroy worklet + close ctx, KEEP SABs
    // Reset the boot-completion latch for the new worklet's `ready`.
    this.ready = false;
    this.whenReady = new Promise((res) => (this._resolveReady = res));
    this._setGate("idle");
    return this.start();
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

  // ---- 0043 teardown ------------------------------------------------------

  // Tear down the audio graph (worklet + context) but KEEP the transport SABs,
  // so rebuild() can re-boot over the same shared state. Posts the worklet
  // `destroy` (0040: frees the engine, nulls its SAB refs), removes the
  // statechange/devicechange listeners, disconnects the node, and closes the
  // context. Leaves ringSab/storeSab/ring/store intact.
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

  // Full teardown: dispose the graph AND drop the SAB references so nothing —
  // engine, node, context, or shared memory — leaks. After this the WebHost is
  // spent (gateState "closed"); a fresh boot requires a NEW WebHost. The worklet
  // side already nulls its own SAB refs on `destroy` (0040), so once we drop
  // ours the SABs are unreferenced on both threads and collectable.
  async teardown() {
    await this._disposeGraph();
    this.ring = null;
    this.store = null;
    this.ringSab = null;
    this.storeSab = null;
    this._tornDown = true;
    this._setGate("closed");
  }

  // Back-compat alias (ticket 0042 harness calls dispose()). Full teardown.
  async dispose() {
    return this.teardown();
  }
}
