// Worklet audio-host driver (ticket 0038) — the JS half of the production
// render loop.
//
// ONE code path, imported by BOTH the Node harness (harness-0038.mjs) and the
// AudioWorklet (vxn-processor-0038.js), so what we prove headlessly is byte-for-
// byte what the browser runs — the same discipline the 0035 spike used for
// event-ring.mjs.
//
// The heavy lifting lives in Rust now (src/host.rs `vxn_host_render`): this
// driver only marshals. Per quantum it:
//   1. folds the 0039 param store into the engine (block-start, changed-only) —
//      the `LocalParams` analogue;
//   2. copies the 0035 ring's due wire-bytes straight into the wasm decode
//      scratch (no per-event JS objects);
//   3. makes ONE wasm call that sets key-mode/split, slices the block at event
//      offsets, decodes+applies, and renders each slice;
//   4. copies the stereo output out of linear memory.
//
// Compare the 0035 spike, which drove the slice loop from JS with O(events +
// slices) boundary crossings per quantum; this is O(1).

import { EventRing, SLOT_BYTES } from "./event-ring.mjs";
import { ParamStore, newWorkletSeen, applyStoreToEngine } from "./param-store.mjs";

export class AudioHost {
  // `wasm` is the instantiated exports object (the `instance.exports`). The
  // SABs are optional so the host degrades cleanly: no ring => no events, no
  // store => no param fold (handy for tests and the no-input bring-up case).
  constructor(wasm, { ringSab = null, storeSab = null, sampleRate, capacity } = {}) {
    this.x = wasm;
    this.host = wasm.vxn_host_new(sampleRate);
    this.Q = wasm.vxn_quantum();
    this.maxEvents = wasm.vxn_host_max_events();

    this.ring = ringSab ? new EventRing(ringSab, capacity) : null;
    this.store = storeSab ? new ParamStore(storeSab) : null;
    this.workletSeen = this.store ? newWorkletSeen() : null;

    // Engine facade the 0039 store→engine fold calls; routes to the host synth.
    this.engine = {
      setParam: (id, v) => this.x.vxn_host_set_param(this.host, id, v),
    };

    // Non-automatable shared state (ADR 0003 §3) — set by the controller, read
    // once per quantum before event ingestion. Defaults track the engine.
    this.keyMode = 0; // 0 Whole, 1 Dual, 2 Split
    this.splitPoint = 60;
  }

  setKeyMode(mode) {
    this.keyMode = mode & 0xff;
  }
  setSplitPoint(note) {
    this.splitPoint = note & 0xff;
  }

  // Byte view over the wasm event scratch. Re-derived each quantum because a
  // wasm memory growth detaches any cached typed array — the same reason the
  // 0034/0035 code re-derives its output views every process().
  _eventsView() {
    return new Uint8Array(
      this.x.memory.buffer,
      this.x.vxn_host_events_ptr(this.host),
      this.maxEvents * SLOT_BYTES,
    );
  }

  // Render one quantum into `outL`/`outR` (Float32Array, length Q). Returns the
  // number of events drained this quantum (instrumentation).
  process(outL, outR) {
    // (1) Param store fold: apply current-value drift to the engine block-start.
    if (this.store) applyStoreToEngine(this.store, this.engine, this.workletSeen);

    // (2) Drain ring bytes straight into the wasm decode scratch.
    let n = 0;
    if (this.ring) n = this.ring.drainRawInto(this._eventsView());

    // (3) One render call: set km/split, slice at offsets, decode+apply, render.
    this.x.vxn_host_render(this.host, n, this.keyMode, this.splitPoint);

    // (4) Copy the stereo output out of linear memory.
    const buf = this.x.memory.buffer;
    const l = new Float32Array(buf, this.x.vxn_host_out_l(this.host), this.Q);
    const r = new Float32Array(buf, this.x.vxn_host_out_r(this.host), this.Q);
    outL.set(l);
    if (outR) outR.set(r);
    return n;
  }

  destroy() {
    if (this.host) {
      this.x.vxn_host_destroy(this.host);
      this.host = 0;
    }
  }
}
