// AudioWorkletProcessor for the production audio-host (ticket 0038).
//
// The 0035 worklet drove the slice loop from JS (renderQuantumSliced) with a
// per-slice/per-event call back into wasm. This one hands the whole quantum to
// the Rust host (vxn_host_render) via the shared AudioHost driver — one wasm
// boundary crossing per quantum. The drain + decode + slice + render now live in
// wasm; this file is just the lifecycle + buffer plumbing.
//
// AudioWorklet module scope supports static ESM imports (resolved by
// audioWorklet.addModule) but has no fetch: the main thread hands us the wasm
// bytes plus the ring/store SABs through processorOptions. Instantiation is the
// raw WebAssembly.instantiate from 0034 — no wasm-bindgen, which keeps the
// module clean in the worklet scope.

import { AudioHost } from "./audio-host.mjs";

class VxnHostProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    this.ready = false;
    this.host = null;

    const opts = options.processorOptions;
    // Controller -> worklet messages: non-automatable shared state (key mode,
    // split point). Params/notes flow over the ring, not the port.
    this.port.onmessage = (e) => {
      const m = e.data;
      if (!this.host) return;
      if (m.type === "keyMode") this.host.setKeyMode(m.value);
      else if (m.type === "splitPoint") this.host.setSplitPoint(m.value);
    };

    WebAssembly.instantiate(opts.wasmBytes, {}).then(({ instance }) => {
      this.host = new AudioHost(instance.exports, {
        ringSab: opts.ringSab,
        storeSab: opts.storeSab, // optional: omit to run without the param store
        sampleRate, // worklet global
        capacity: opts.capacity,
      });
      this.ready = true;
    });
  }

  process(_inputs, outputs) {
    const out = outputs[0];
    if (!this.ready) return true; // silence until wasm is live
    this.host.process(out[0], out[1]);
    return true;
  }
}

registerProcessor("vxn-host-processor", VxnHostProcessor);
