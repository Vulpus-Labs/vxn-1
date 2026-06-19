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

    // Cached views over linear memory (events scratch + stereo out). Re-derived
    // ONLY when the wasm buffer changes (a memory growth detaches them) — keyed
    // on buffer identity below. Allocating these fresh per quantum (the old path)
    // churns the GC, which on Safari's JSC stalls the realtime audio thread and
    // crackles; cache them so the steady-state render allocates nothing.
    this._buf = null;
    this._eventsU8 = null;
    this._outLview = null;
    this._outRview = null;

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

  // Rebuild the engine at a new sample rate (context sample-rate change, 0040).
  setSampleRate(sr) {
    this.x.vxn_host_set_sample_rate(this.host, sr);
  }

  // All-notes-off / clear voices without touching ring or store (0040): used on
  // resume-after-suspend and on re-init recovery to avoid stuck notes.
  reset() {
    this.x.vxn_host_reset(this.host);
  }

  // Test hook (0040): arm a forced wasm trap on the next process(), so the trap-
  // safety boundary can be exercised headlessly. No-op in production paths.
  armTrap() {
    this._armTrap = true;
  }

  // (Re)derive the cached memory views. Called lazily from process() only when
  // the wasm buffer identity changes — a memory growth detaches the old views,
  // and the underlying pointers can move, so all three must be rebuilt together.
  _refreshViews() {
    const buf = this.x.memory.buffer;
    this._buf = buf;
    this._eventsU8 = new Uint8Array(
      buf,
      this.x.vxn_host_events_ptr(this.host),
      this.maxEvents * SLOT_BYTES,
    );
    this._outLview = new Float32Array(buf, this.x.vxn_host_out_l(this.host), this.Q);
    this._outRview = new Float32Array(buf, this.x.vxn_host_out_r(this.host), this.Q);
  }

  // Render one quantum into `outL`/`outR` (Float32Array, length Q). Returns the
  // number of events drained this quantum (instrumentation).
  process(outL, outR) {
    // Test hook: trigger a render-thread trap so the worklet boundary's catch +
    // recovery path can be proven (0040). The trap throws out of process().
    if (this._armTrap) {
      this._armTrap = false;
      this.x.vxn_host_force_trap();
    }

    // (1) Param store fold: apply current-value drift to the engine block-start.
    if (this.store) applyStoreToEngine(this.store, this.engine, this.workletSeen);

    // Ensure the cached views are live before we touch linear memory. In steady
    // state the buffer never changes, so this is a pointer compare and nothing is
    // allocated; it only re-derives across a (rare) memory growth.
    if (this._buf !== this.x.memory.buffer) this._refreshViews();

    // (2) Drain ring bytes straight into the wasm decode scratch.
    let n = 0;
    if (this.ring) n = this.ring.drainRawInto(this._eventsU8);

    // (3) One render call: set km/split, slice at offsets, decode+apply, render.
    this.x.vxn_host_render(this.host, n, this.keyMode, this.splitPoint);

    // (4) Copy the stereo output out of linear memory. Re-check identity: the
    // render call could have grown memory and detached the output views.
    if (this._buf !== this.x.memory.buffer) this._refreshViews();
    outL.set(this._outLview);
    if (outR) outR.set(this._outRview);
    return n;
  }

  destroy() {
    if (this.host) {
      this.x.vxn_host_destroy(this.host);
      this.host = 0;
    }
  }
}
