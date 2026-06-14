// AudioWorkletProcessor for the SAB event-ring spike (ticket 0035).
//
// Replaces the 0034 postMessage path with the real transport: a lock-free SPSC
// ring in a SharedArrayBuffer, drained every quantum, with the render block
// sliced at event sample-offsets exactly like the CLAP shell. The drain + slice
// logic lives in event-ring.mjs and is shared verbatim with the Node harness —
// this file is just the wasm wiring around that one code path.
//
// AudioWorklet module scope supports static ESM imports (the module is added
// via audioWorklet.addModule, which resolves imports), but has no fetch: the
// main thread hands us the wasm bytes AND the SAB through processorOptions.

import {
  EventRing,
  renderQuantumSliced,
  renderQuantumBlockStart,
} from "./event-ring.mjs";

class VxnRingProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    this.ready = false;
    this.synth = 0;
    this.drained = []; // reused across process() calls — no render-thread alloc

    const opts = options.processorOptions;
    this.ring = new EventRing(opts.sab, opts.capacity);
    // Slicing fidelity is selectable so the browser can A/B it live; default
    // is full per-event slicing (the decision this spike lands).
    this.mode = opts.mode === "block-start" ? "block-start" : "sliced";

    WebAssembly.instantiate(opts.wasmBytes, {}).then(({ instance }) => {
      this.x = instance.exports;
      this.Q = this.x.vxn_quantum();
      this.synth = this.x.vxn_new(sampleRate); // worklet global
      // Engine facade consumed by the shared slice loop.
      const x = this.x;
      const s = this.synth;
      this.engine = {
        noteOn: (note, vel) => x.vxn_note_on(s, note, vel),
        noteOff: (note) => x.vxn_note_off(s, note),
        setParam: (idx, val) => x.vxn_set_param(s, idx, val),
        processSlice: (start, end) => x.vxn_process_slice(s, start, end),
      };
      this.ready = true;
    });
  }

  process(_inputs, outputs) {
    const out = outputs[0];
    if (!this.ready) return true; // silence until wasm is live

    // 1. Drain the ring lock-free (free-poll, no Atomics.wait).
    const recs = this.ring.drainInto(this.drained);
    // 2. Render the quantum, slicing at event offsets (CLAP parity).
    if (this.mode === "block-start") {
      renderQuantumBlockStart(this.engine, recs, this.Q);
    } else {
      renderQuantumSliced(this.engine, recs, this.Q);
    }
    // 3. Copy the rendered quantum out of linear memory.
    const buf = this.x.memory.buffer;
    const l = new Float32Array(buf, this.x.vxn_out_l(this.synth), this.Q);
    const r = new Float32Array(buf, this.x.vxn_out_r(this.synth), this.Q);
    out[0].set(l);
    if (out[1]) out[1].set(r);
    return true;
  }
}

registerProcessor("vxn-ring-processor", VxnRingProcessor);
