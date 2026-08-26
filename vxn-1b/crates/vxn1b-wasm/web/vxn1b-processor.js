// AudioWorkletProcessor for the production audio host (0289).
//
// A worklet shell around the shared render/lifecycle runner. AudioWorklet module
// scope supports static ESM imports (resolved by `audioWorklet.addModule`) but
// has no `fetch`: the main thread hands over the wasm bytes and the three SABs
// through `processorOptions`. Instantiation is a raw `WebAssembly.instantiate` —
// no wasm-bindgen, which could not run here anyway.

import { WorkletHostRunner } from "./host-runner.mjs";

class Vxn1bHostProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    this.alive = true;

    const opts = options.processorOptions;

    this.runner = new WorkletHostRunner({
      wasmBytes: opts.wasmBytes,
      ringSab: opts.ringSab,
      storeSab: opts.storeSab,
      telemetrySab: opts.telemetrySab,
      sampleRate, // worklet global
      capacity: opts.capacity,
      onReady: () => this.port.postMessage({ type: "ready" }),
      // A trap takes the engine down for good on this port (0297): recovering
      // audio without the non-automatable state would resume the WRONG patch.
      onTrap: (e, count) =>
        this.port.postMessage({
          type: "trap",
          message: String((e && e.message) || e),
          count,
        }),
    });

    // Controller -> worklet lifecycle messages. Notes, params and domain state
    // all flow over the RING, not the port — the port carries only lifecycle.
    this.port.onmessage = (e) => {
      const m = e.data;
      switch (m.type) {
        case "reset":
          this.runner.reset(); // resume-after-suspend
          break;
        case "destroy":
          this.runner.destroy();
          this.alive = false;
          break;
        default:
          break;
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

registerProcessor("vxn1b-host-processor", Vxn1bHostProcessor);
