// Worklet audio-host driver (ticket 0156) — the JS half of the production
// render loop. Ported from vxn-1's `vxn-wasm/web/audio-host`; the vxn-2 changes
// are: no key-mode/split (dropped), `vxn_host_render(host, n)` takes no shared-
// state args, and the param fold calls `vxn_host_set_param` (block-start fold,
// since vxn-2 has no per-id engine setter — the wasm host folds the store into
// the engine inside `vxn_host_render` via `Engine::snapshot_params`).
//
// The heavy lifting lives in Rust (`../src/host.rs vxn_host_render`); this driver
// only marshals. Per quantum it:
//   1. pushes changed store values into the wasm host block-start (the
//      `applyStoreToHost` fold);
//   2. drains the ring's due wire-bytes straight into the wasm decode scratch;
//   3. makes ONE wasm call that folds params, slices the block at event offsets,
//      decodes+applies, and renders each slice (sub-chunked to CONTROL_BLOCK);
//   4. copies the stereo output out of linear memory.

import { EventRing, SLOT_BYTES } from "./event-ring.mjs";
import { ParamStore, newWorkletSeen, applyStoreToHost } from "./param-store.mjs";

export class AudioHost {
  // `wasm` is the instantiated exports object. The SABs are optional so the host
  // degrades cleanly: no ring => no events, no store => no param fold (handy for
  // tests and the no-input bring-up case).
  constructor(wasm, { ringSab = null, storeSab = null, sampleRate, capacity } = {}) {
    this.x = wasm;
    this.host = wasm.vxn_host_new(sampleRate);
    this.Q = wasm.vxn_quantum();
    this.maxEvents = wasm.vxn_host_max_events();

    this.ring = ringSab ? new EventRing(ringSab, capacity) : null;
    this.store = storeSab ? new ParamStore(storeSab) : null;
    this.workletSeen = this.store ? newWorkletSeen() : null;

    // Cached views over linear memory (events scratch + stereo out). Re-derived
    // ONLY when the wasm buffer changes (a memory growth detaches them). Fresh
    // allocation per quantum churns the GC — on Safari's JSC that stalls the
    // realtime audio thread and crackles.
    this._buf = null;
    this._eventsU8 = null;
    this._outLview = null;
    this._outRview = null;

    // Host facade the store→host fold calls; routes to the wasm param setter.
    this.hostFacade = {
      setParam: (id, v) => this.x.vxn_host_set_param(this.host, id, v),
    };
  }

  // Rebuild the engine at a new sample rate (context sample-rate change).
  setSampleRate(sr) {
    this.x.vxn_host_set_sample_rate(this.host, sr);
  }

  // All-notes-off / clear voices without touching ring or store: used on
  // resume-after-suspend and on re-init recovery to avoid stuck notes.
  reset() {
    this.x.vxn_host_reset(this.host);
  }

  // Test hook: arm a forced wasm trap on the next process(), so the trap-safety
  // boundary can be exercised headlessly. No-op in production paths.
  armTrap() {
    this._armTrap = true;
  }

  // (Re)derive the cached memory views. Called lazily from process() only when
  // the wasm buffer identity changes — a memory growth detaches the old views and
  // the pointers can move, so all three rebuild together.
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
    // recovery path can be proven. The trap throws out of process().
    if (this._armTrap) {
      this._armTrap = false;
      this.x.vxn_host_force_trap();
    }

    // (1) Param store fold: push current-value drift into the host block-start.
    // The wasm host folds the store into the engine inside vxn_host_render.
    if (this.store) applyStoreToHost(this.store, this.hostFacade, this.workletSeen);

    // Ensure the cached views are live before touching linear memory. In steady
    // state the buffer never changes → a pointer compare, no allocation.
    if (this._buf !== this.x.memory.buffer) this._refreshViews();

    // (2) Drain ring bytes straight into the wasm decode scratch.
    let n = 0;
    if (this.ring) n = this.ring.drainRawInto(this._eventsU8);

    // (3) One render call: fold params, slice at offsets, decode+apply, render.
    this.x.vxn_host_render(this.host, n);

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
