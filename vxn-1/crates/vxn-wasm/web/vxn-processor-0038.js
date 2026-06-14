// AudioWorkletProcessor for the production audio-host (tickets 0038 + 0040).
//
// 0038 built the render loop (one vxn_host_render per quantum); 0040 wraps it in
// the lifecycle runner (host-runner.mjs): instantiate-from-bytes, silence-until-
// ready, sample-rate, suspend/resume reset, teardown, and render-thread trap
// safety. This file is just the worklet shell around that shared runner.
//
// AudioWorklet module scope supports static ESM imports (resolved by
// audioWorklet.addModule) but has no fetch: the main thread hands us the wasm
// bytes plus the ring/store SABs through processorOptions. Instantiation is the
// raw WebAssembly.instantiate from 0034 — no wasm-bindgen.

import { WorkletHostRunner } from "./host-runner.mjs";

class VxnHostProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    this.alive = true;

    const opts = options.processorOptions;
    this.runner = new WorkletHostRunner({
      wasmBytes: opts.wasmBytes,
      ringSab: opts.ringSab,
      storeSab: opts.storeSab, // optional
      sampleRate, // worklet global
      capacity: opts.capacity,
      // Surface lifecycle to the main thread (E016/E018 react to these).
      onReady: () => this.port.postMessage({ type: "ready" }),
      onTrap: (e, count) =>
        this.port.postMessage({ type: "trap", message: String(e && e.message || e), count }),
    });

    // Controller -> worklet lifecycle + shared-state messages. Params/notes flow
    // over the ring, not the port. Messages sent before ready are still honoured:
    // the runner buffers key-mode/split and applies them on instantiate.
    this.port.onmessage = (e) => {
      const m = e.data;
      switch (m.type) {
        case "keyMode": this.runner.setKeyMode(m.value); break;
        case "splitPoint": this.runner.setSplitPoint(m.value); break;
        case "sampleRate": this.runner.setSampleRate(m.value); break;
        case "reset": this.runner.reset(); break; // resume-after-suspend
        case "destroy": this.runner.destroy(); this.alive = false; break;
        default: break;
      }
    };

    this.runner.init(); // async; process() renders silence until it resolves
  }

  process(_inputs, outputs) {
    if (!this.alive) return false; // teardown: let the node be collected
    const out = outputs[0];
    this.runner.process(out[0], out[1]); // silence-until-ready + trap-safe
    return true;
  }
}

registerProcessor("vxn-host-processor", VxnHostProcessor);
