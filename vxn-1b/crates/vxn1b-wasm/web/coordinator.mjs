// Main-thread audio bootstrap (0289) — the half that OWNS the transport.
//
// `WebHost` creates the AudioContext, loads the worklet, allocates the three
// SABs, seeds the param store with the engine's defaults, and exposes the
// producer surface the UI drives. The worklet half (`vxn1b-processor.js` ->
// `host-runner.mjs` -> `audio-host.mjs`) consumes it.
//
// Construct cheaply, then `start()` from a user-gesture handler — the autoplay
// policy requires the context resume to happen inside a gesture call stack.
//
// ===========================================================================
// WHAT TRAVELS WHERE
// ===========================================================================
//
//   ring SAB       notes, sample-accurate params, and ALL non-automatable
//                  domain state (key mode, split point, LFO 2 link, matrix
//                  topology, scope tap, tempo)
//   param SAB      every CLAP-id param, block-granular, latest-value-wins,
//                  plus the audio->main readback the diff pump reads
//   telemetry SAB  meter and scope frames, worklet -> main
//   the port       lifecycle only: ready, trap, cpu, reset, destroy
//
// vxn-1 sends its key mode and split point over the PORT, because its wire
// predates having anywhere better to put them. VXN1b's ride the ring with
// everything else, which is why this coordinator has no latched shared state to
// replay onto a fresh worklet — the ring's contents are not lost by a rebuild,
// since the SABs survive it.

import { EventRing, createRingSAB, DEFAULT_CAPACITY } from "./event-ring.mjs";
import { ParamStore, createParamSAB, TOTAL_PARAMS, newLastSeen, pollDiffs } from "./param-store.mjs";
import { createTelemetrySAB, TelemetryReader } from "./telemetry.mjs";

const DEFAULT_WASM_URL = "./vxn1b_wasm.wasm";
const DEFAULT_WORKLET_URL = "./vxn1b-processor.js";
const PROCESSOR_NAME = "vxn1b-host-processor";

/// Safari/WebKit has no render-thread slack to spare: the per-quantum clock read
/// plus the periodic postMessage the CPU meter needs can itself glitch the
/// audio, and it ignores `latencyHint` so there is no output-buffer headroom to
/// absorb the variance. Detected here so the worklet can skip the work entirely.
function isAppleWebKit() {
  const ua = globalThis.navigator ? globalThis.navigator.userAgent || "" : "";
  return /AppleWebKit/.test(ua) && !/Chrome|Chromium|Edg/.test(ua);
}

export class WebHost {
  // Options:
  //   wasmUrl / workletUrl  : dist-relative URLs (defaults match the bundle).
  //   wasmBytes             : pre-fetched engine bytes; skips the fetch (tests
  //                           pass this — there is no fetch in Node).
  //   capacity              : event-ring slots (power of two). Main and worklet
  //                           MUST agree; passed through processorOptions.
  //   onReady / onTrap      : lifecycle observers.
  //   onState               : gate observer (idle | starting | running |
  //                           suspended | closed).
  //   onCpu                 : render-load observer, (load, peak) as fractions of
  //                           the per-quantum budget. Called with (null, null)
  //                           where the meter is disabled.
  //   AudioContextClass /
  //   AudioWorkletNodeClass : injection seams for headless testing.
  //   fetchImpl, mediaDevices: seams; mediaDevices null disables device-change.
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

    // Allocate the transport up front — cheap, needs no audio context — so the
    // producer surface is usable the instant the WebHost exists. Events written
    // before `ready` buffer in the ring and apply on the first live quantum
    // (the silence-until-ready contract the runner guarantees).
    this.ringSab = createRingSAB(this.capacity);
    this.storeSab = createParamSAB();
    this.ring = new EventRing(this.ringSab, this.capacity); // producer side
    this.store = new ParamStore(this.storeSab); // controller side
    this._lastSeen = newLastSeen(); // readback diff mirror

    // Telemetry is sized from the engine, so the SAB is allocated once the wasm
    // is available (start()); until then there is nothing to read.
    this.telemetrySab = null;
    this.telemetry = null;

    // Seeded once, on the first start(); see _seedStoreFromDefaults.
    this._storeSeeded = false;

    this.ctx = null;
    this.node = null;
    this.ready = false; // the worklet posted `ready`
    this.trapCount = 0;

    // Gate / lifecycle state machine. The browser's AudioContext.state is the
    // underlying truth for the live states; this mirrors its statechange
    // transitions and never drives it out of band.
    this.gateState = "idle";
    this._statechange = null;
    this._devicechange = null;
    this._tornDown = false;

    // Resolves at "audio live". start() does NOT block on it — resume can settle
    // before the async wasm instantiate finishes.
    this.whenReady = new Promise((res) => (this._resolveReady = res));
  }

  _setGate(state) {
    if (this.gateState === state) return;
    this.gateState = state;
    try {
      this._onState(state);
    } catch {}
  }

  /// Boot to "audio live". Call from a user-gesture handler. Creates the
  /// context, loads the worklet module and fetches the wasm in parallel, seeds
  /// the param store, constructs the node over the SABs, and resumes.
  async start() {
    if (this._tornDown) throw new Error("WebHost torn down; construct a fresh one");
    if (this.ctx) throw new Error("WebHost.start() already called");
    if (!this._AudioContext) throw new Error("no AudioContext available");

    this._setGate("starting");
    this.ctx = new this._AudioContext();

    this._attachStateChange();
    this._attachDeviceChange();

    // Worklet scope cannot fetch, so the main thread fetches the wasm and hands
    // the bytes over through processorOptions. addModule resolves the worklet's
    // static ESM imports (runner / host / ring / store / telemetry).
    const [wasmBytes] = await Promise.all([
      this._loadWasmBytes(),
      this.ctx.audioWorklet.addModule(this.workletUrl),
    ]);
    this.wasmBytes = wasmBytes;

    // Seed the store with the engine's defaults BEFORE the worklet starts. The
    // slots are zero-initialised and the worklet's first fold is NaN-seeded, so
    // it applies EVERY id — an unseeded store would therefore write 0.0 over
    // every param and silence the instrument. Done before node construction, so
    // it is populated before the worklet can read it.
    await this._seedStoreFromDefaults(wasmBytes);

    this._cpuMeterEnabled = !isAppleWebKit();
    this.node = new this._AudioWorkletNode(this.ctx, PROCESSOR_NAME, {
      numberOfInputs: 0,
      numberOfOutputs: 1,
      outputChannelCount: [2],
      processorOptions: {
        wasmBytes,
        ringSab: this.ringSab,
        storeSab: this.storeSab,
        telemetrySab: this.telemetrySab,
        capacity: this.capacity,
        cpuMeter: this._cpuMeterEnabled,
      },
    });

    this.node.port.onmessage = (e) => this._onPortMessage(e.data);

    this.node.connect(this.ctx.destination);
    // No render-load source on Safari → report n/a rather than a frozen number.
    if (!this._cpuMeterEnabled) this._onCpu(null, null);

    // Autoplay unlock: the context starts suspended, and resume() must be inside
    // a user-gesture call stack (start()'s contract). The statechange listener
    // also sets the gate, but it is set here too so callers that get no
    // synchronous statechange (a fake context) still observe "running".
    await this.ctx.resume();
    this._setGate(this.ctx.state === "running" ? "running" : "suspended");
    return this;
  }

  async _loadWasmBytes() {
    if (this.wasmBytes) return this.wasmBytes;
    if (!this._fetch) throw new Error("no fetch and no wasmBytes provided");
    const resp = await this._fetch(this.wasmUrl);
    if (!resp.ok) throw new Error(`wasm fetch failed: ${resp.status}`);
    return resp.arrayBuffer();
  }

  /// Snapshot the engine's defaults off a throwaway instance and bulk-write them
  /// into the store, so the worklet's first fold is a no-op against the engine
  /// rather than a zeroing pass. The instance is discarded immediately; only its
  /// defaults survive, in the SAB.
  ///
  /// The telemetry SAB is sized here too, from the same instance — its region
  /// lengths are engine constants, so this is the first point they are known.
  ///
  /// ONCE, not on every start(). `rebuild()` re-runs start() over the SAME SABs
  /// precisely so the live patch survives a context change; re-seeding there
  /// would overwrite every param with its default and silently reset the user's
  /// sound on a sample-rate change or a device switch. The zero-fold this
  /// guards against is a first-boot problem only — after that the store holds
  /// the authoritative values.
  async _seedStoreFromDefaults(wasmBytes) {
    if (this._storeSeeded) return;
    const { instance } = await WebAssembly.instantiate(wasmBytes, {});
    const x = instance.exports;
    const sr = this.ctx ? this.ctx.sampleRate : 48000;
    const h = x.vxn1b_host_new(sr);

    const vals = new Float32Array(TOTAL_PARAMS);
    for (let id = 0; id < TOTAL_PARAMS; id++) vals[id] = x.vxn1b_host_get_param(h, id);
    this.store.writeBulk(vals);
    x.vxn1b_host_destroy(h);
    this._storeSeeded = true;

    if (!this.telemetrySab) {
      const meterLen = x.vxn1b_meter_len();
      const scopeWindow = x.vxn1b_scope_window();
      this.telemetrySab = createTelemetrySAB(meterLen, scopeWindow);
      this.telemetry = new TelemetryReader(this.telemetrySab, { meterLen, scopeWindow });
    }
  }

  _onPortMessage(m) {
    switch (m && m.type) {
      case "ready":
        this.ready = true;
        this._resolveReady(this);
        this._onReady();
        break;
      case "trap":
        // The runner already caught it and kicked async recovery; this only
        // observes. `ready` flips back true on the next `ready` after re-init.
        //
        // IMPORTANT: the rebuilt engine has lost every piece of non-automatable
        // state (key mode, split point, LFO 2 link, matrix topology, scope tap,
        // tempo). Params restore themselves from the store; that state does not,
        // and the controller has to re-broadcast it. The faceplate bridge (0290)
        // is what listens for this.
        this.ready = false;
        this.trapCount = m.count != null ? m.count : this.trapCount + 1;
        this._onTrap(m.message, this.trapCount);
        break;
      case "cpu":
        if (m.clock && !this._cpuClockLogged) {
          console.info(`vxn1b: CPU meter clock = ${m.clock}`);
          this._cpuClockLogged = true;
        }
        this._onCpu(m.load, m.peak);
        break;
      default:
        break;
    }
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

  /// Mirror an AudioContext.statechange into the gate machine.
  ///
  /// The MAIN thread owns the resume voice-flush, not the worklet. A suspended
  /// context's audio clock is stopped, so `process()` never runs and voices that
  /// were sounding when audio stopped would otherwise resume mid-note — or hang,
  /// if their note-off was eaten by a page that cleared its key state while
  /// backgrounded. On resume the worklet gets a `reset`, which clears sounding
  /// voices WITHOUT touching the ring or store.
  ///
  /// Deliberately NOT flushed on suspend: there is nothing to render while
  /// suspended, so resetting then is a no-op the resume path would repeat.
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

  /// Programmatic suspend. The browser also suspends on its own (tab
  /// background); both land in `_onStateChange` when the context emits
  /// statechange. The gate is mirrored by hand only when there is NO statechange
  /// listener (a fake context), to avoid double-driving it.
  async suspend() {
    if (this.ctx && typeof this.ctx.suspend === "function" && this.ctx.state === "running") {
      await this.ctx.suspend();
      if (!this._statechange && this.ctx.state === "suspended") this._setGate("suspended");
    }
  }

  /// Programmatic resume. Must be reachable from a user gesture if the browser
  /// is gating the suspend. With a statechange listener active it owns the gate
  /// and the voice flush; without one, this mirrors and flushes.
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

  // ---- device change ------------------------------------------------------
  //
  // Two cases, decided by whether the sample rate moves:
  //   (a) same rate, different device — re-route in place via setSink(), no
  //       graph change: the engine keeps rendering and only the sink moves.
  //   (b) rate change — an AudioContext's sampleRate is immutable, so this needs
  //       a NEW context. rebuild() re-boots over the SAME SABs, so transport
  //       state survives.

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

  /// No structural action by default: most device changes do not move the
  /// context rate, so the graph stays up. A hook rather than a policy, so the
  /// faceplate can override without re-listening.
  _onDeviceChange() {}

  /// Re-route output to a specific device without rebuilding the graph. Resolves
  /// true if the sink moved, false where setSinkId is unavailable.
  async setSink(sinkId) {
    if (this.ctx && typeof this.ctx.setSinkId === "function") {
      await this.ctx.setSinkId(sinkId);
      return true;
    }
    return false;
  }

  /// Rebuild the graph at a (possibly new) sample rate, reusing the SAME SABs so
  /// transport and param state survive. Must be called from a user gesture (it
  /// resumes a fresh context). The ring's read index is wherever the old worklet
  /// left it, so no events are lost; voices sounding at teardown go with the old
  /// engine — a clean break rather than a stuck note.
  async rebuild() {
    if (this._tornDown) throw new Error("WebHost torn down; construct a fresh one");
    await this._disposeGraph();
    this.ready = false;
    this.whenReady = new Promise((res) => (this._resolveReady = res));
    this._setGate("idle");
    return this.start();
  }

  // ---- producer surface: everything the UI sends downstream ---------------
  //
  // The main-thread half of the SPSC ring; the worklet drains it in its render
  // loop. All return the ring's block-writer boolean (false iff it is
  // momentarily full — the caller can retry; in practice it is sized so this
  // never fires). `offset` is the sample offset within the next quantum for
  // sample-accurate placement; 0 means "as soon as possible".
  //
  // Notes carry a MIDI channel because VXN1b is MPE-aware; a non-MPE caller
  // simply omits it and gets channel 0.

  noteOn(note, velocity = 1, offset = 0, channel = 0) {
    return this.ring.pushNoteOn(offset, note, velocity, channel);
  }
  noteOff(note, offset = 0, channel = 0) {
    return this.ring.pushNoteOff(offset, note, channel);
  }
  polyPressure(note, value, offset = 0, channel = 0) {
    return this.ring.pushPolyPressure(offset, note, value, channel);
  }
  channelPressure(value, offset = 0, channel = 0) {
    return this.ring.pushChannelPressure(offset, value, channel);
  }
  pitchBend(value, offset = 0) {
    return this.ring.pushPitchBend(offset, value);
  }
  modWheel(value, offset = 0) {
    return this.ring.pushModWheel(offset, value);
  }

  /// Sample-accurate param automation. A plain edit should go through
  /// `setParam` (the store) instead — this is for automation that has to land at
  /// a specific frame.
  paramAt(id, plain, offset = 0) {
    return this.ring.pushParam(offset, id, plain);
  }
  gestureBegin(id, offset = 0) {
    return this.ring.pushGestureBegin(offset, id);
  }
  gestureEnd(id, offset = 0) {
    return this.ring.pushGestureEnd(offset, id);
  }

  // Non-automatable domain state. None of it has a CLAP id, so none of it
  // occupies a store slot; it all rides the ring.
  setKeyMode(mode, offset = 0) {
    return this.ring.pushKeyMode(offset, mode & 0xff);
  }
  setSplitPoint(note, offset = 0) {
    return this.ring.pushSplitPoint(offset, note & 0xff);
  }
  setLfo2Link(on, offset = 0) {
    return this.ring.pushLfo2Link(offset, on);
  }
  /// One matrix slot's topology field. Slot DEPTH is a CLAP param and goes
  /// through `setParam` instead — that split is what lets a slot be automated
  /// without its routing changing underneath the automation.
  setMatrix(layer, slot, field, value, offset = 0) {
    return this.ring.pushMatrixEdit(offset, layer, slot, field, value);
  }
  setScopeTap(tap, offset = 0) {
    return this.ring.pushScopeTap(offset, tap & 0xff);
  }
  setTempo(bpm, offset = 0) {
    return this.ring.pushTempo(offset, bpm);
  }

  // ---- param store --------------------------------------------------------

  setParam(id, value) {
    this.store.write(id, value);
  }
  setParamsBulk(values) {
    this.store.writeBulk(values);
  }
  readParam(id) {
    return this.store.read(id);
  }
  /// Drain the audio->main readback into ParamChanged-equivalent records.
  pollParamDiffs() {
    return pollDiffs(this.store, this._lastSeen);
  }

  // ---- telemetry ----------------------------------------------------------

  /// Latest meter frame, or null when there is nothing new to show (unchanged,
  /// a torn read that could not be resolved, or suppressed silence).
  pollMeters() {
    return this.telemetry ? this.telemetry.readMeters() : null;
  }
  /// Latest scope window, or null on nothing-new / tap off / suppressed.
  pollScope() {
    return this.telemetry ? this.telemetry.readScope() : null;
  }

  // ---- teardown -----------------------------------------------------------

  /// Tear down the audio graph but KEEP the transport SABs, so rebuild() can
  /// re-boot over the same shared state.
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

  /// Full teardown: dispose the graph AND drop the SAB references so nothing —
  /// engine, node, context or shared memory — leaks. The WebHost is spent
  /// afterwards; a fresh boot needs a new one. The worklet nulls its own SAB
  /// refs on `destroy`, so once these go the SABs are unreferenced on both
  /// threads and collectable.
  async teardown() {
    await this._disposeGraph();
    this.ring = null;
    this.store = null;
    this.telemetry = null;
    this.ringSab = null;
    this.storeSab = null;
    this.telemetrySab = null;
    this._tornDown = true;
    this._setGate("closed");
  }
}
