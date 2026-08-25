// Worklet audio-host driver (0289) — the JS half of the production render loop.
//
// ONE code path, imported by both the AudioWorklet (`vxn1b-processor.js`) and
// the headless tests, so what is proven in Node is byte-for-byte what the
// browser runs.
//
// The heavy lifting is in Rust (`src/host.rs`, `vxn1b_host_render`); this driver
// only marshals. Per quantum it:
//   1. folds the param store into the engine (block-start, changed-only) — the
//      `LocalParams` analogue;
//   2. copies the ring's due wire-bytes straight into the wasm decode scratch,
//      with no per-event JS objects;
//   3. makes ONE wasm call that slices the block at event offsets, decodes and
//      applies, and renders each slice;
//   4. copies the stereo output out of linear memory;
//   5. ticks the telemetry writer, which publishes meter/scope frames on its own
//      rate division.
//
// Note what is NOT here, versus vxn-1's: no key-mode or split-point arguments.
// VXN1b's non-automatable state rides the ring (tags 7/8/11), so the render call
// takes an event count and nothing else.

import { EventRing, SLOT_BYTES } from "./event-ring.mjs";
import { ParamStore, newWorkletSeen, applyStoreToEngine } from "./param-store.mjs";
import { TelemetryWriter } from "./telemetry.mjs";

export class AudioHost {
  /// `wasm` is the instantiated exports object. The SABs are optional so the
  /// host degrades cleanly: no ring means no events, no store means no param
  /// fold, no telemetry SAB means no meter/scope publishing.
  constructor(wasm, { ringSab = null, storeSab = null, telemetrySab = null, sampleRate, capacity } = {}) {
    this.x = wasm;
    this.host = wasm.vxn1b_host_new(sampleRate);
    this.Q = wasm.vxn1b_quantum();
    this.maxEvents = wasm.vxn1b_host_max_events();

    this.ring = ringSab ? new EventRing(ringSab, capacity) : null;
    this.store = storeSab ? new ParamStore(storeSab) : null;
    this.workletSeen = this.store ? newWorkletSeen() : null;

    // Cached views over linear memory (event scratch + stereo out), re-derived
    // ONLY when the wasm buffer identity changes — a memory growth detaches the
    // old views and can move the pointers. Building them fresh per quantum
    // churns the GC, which on Safari's JSC stalls the realtime thread and
    // crackles, so the steady-state render allocates nothing.
    this._buf = null;
    this._eventsU8 = null;
    this._outLview = null;
    this._outRview = null;

    // Engine facade the store fold drives.
    this.engine = {
      setParam: (id, v) => this.x.vxn1b_host_set_param(this.host, id, v),
    };

    this.telemetry = telemetrySab
      ? new TelemetryWriter(telemetrySab, {
          meterLen: wasm.vxn1b_meter_len(),
          scopeWindow: wasm.vxn1b_scope_window(),
          sampleRate,
          quantum: this.Q,
          // The frame readers hand back views over wasm memory. Re-derived per
          // call rather than cached because they are touched a few times a
          // second, not per quantum — the cost is noise at that rate, and it
          // keeps them correct across a memory growth for free.
          engine: {
            drainMeters: () => this.x.vxn1b_host_drain_meters(this.host),
            meterFrame: () =>
              new Float32Array(
                this.x.memory.buffer,
                this.x.vxn1b_host_meters_ptr(this.host),
                wasm.vxn1b_meter_len(),
              ),
            readScope: () => this.x.vxn1b_host_read_scope(this.host),
            scopeSamples: (n) =>
              new Float32Array(this.x.memory.buffer, this.x.vxn1b_host_scope_ptr(this.host), n),
          },
        })
      : null;
  }

  /// Drop every sounding voice without touching ring or store — used on
  /// resume-after-suspend so nothing is left hanging from before the stop.
  reset() {
    this.x.vxn1b_host_reset(this.host);
  }

  /// (Re)derive the cached memory views. All three must be rebuilt together: a
  /// growth detaches the old views and the underlying pointers can move.
  _refreshViews() {
    const buf = this.x.memory.buffer;
    this._buf = buf;
    this._eventsU8 = new Uint8Array(
      buf,
      this.x.vxn1b_host_events_ptr(this.host),
      this.maxEvents * SLOT_BYTES,
    );
    this._outLview = new Float32Array(buf, this.x.vxn1b_host_out_l(this.host), this.Q);
    this._outRview = new Float32Array(buf, this.x.vxn1b_host_out_r(this.host), this.Q);
  }

  /// Render one quantum into `outL` / `outR`. Returns the number of events
  /// drained (instrumentation).
  process(outL, outR) {
    // (1) Fold current-value param drift into the engine, block-start.
    if (this.store) applyStoreToEngine(this.store, this.engine, this.workletSeen);

    // Make sure the cached views are live before touching linear memory. In
    // steady state this is a pointer compare and allocates nothing.
    if (this._buf !== this.x.memory.buffer) this._refreshViews();

    // (2) Drain ring bytes straight into the wasm decode scratch.
    let n = 0;
    if (this.ring) n = this.ring.drainRawInto(this._eventsU8);

    // (3) One render call: slice at offsets, decode and apply, render each slice.
    this.x.vxn1b_host_render(this.host, n);

    // (4) Copy the stereo output out. Re-check identity first: the render call
    // could have grown memory and detached the output views.
    if (this._buf !== this.x.memory.buffer) this._refreshViews();
    outL.set(this._outLview);
    if (outR) outR.set(this._outRview);

    // (5) Telemetry, on its own rate division (~60 Hz meters, ~30 Hz scope).
    if (this.telemetry) this.telemetry.tick();
    return n;
  }

  destroy() {
    if (this.host) {
      this.x.vxn1b_host_destroy(this.host);
      this.host = 0;
    }
  }
}
