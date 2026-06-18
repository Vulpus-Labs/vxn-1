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

// High-res wall clock for the render-load meter. `performance.now()` is exposed
// in AudioWorkletGlobalScope in modern browsers; if it's missing we report 0
// load (meter shows idle) rather than break the render path.
const perfNow =
  typeof performance !== "undefined" && performance.now
    ? () => performance.now()
    : () => 0;

class VxnHostProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    this.alive = true;

    // ---- render-load meter (CPU %) ----------------------------------------
    // Time the DSP render each quantum and express it as a fraction of the
    // quantum's wall-clock budget (frames / sampleRate). EMA-smoothed, with a
    // per-window peak, posted to the main thread a few times a second.
    this._cpuBudgetMs = 0; // set lazily from the first output buffer length
    this._cpuEma = 0;
    this._cpuPeak = 0;
    this._cpuCount = 0;
    this._cpuReportEvery = 16; // ~23 Hz at 48k/128 — smooth, not spammy

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
    if (this._cpuBudgetMs === 0 && out[0]) {
      this._cpuBudgetMs = (out[0].length / sampleRate) * 1000; // quantum budget
    }
    const t0 = perfNow();
    this.runner.process(out[0], out[1]); // silence-until-ready + trap-safe
    this._accumCpu(perfNow() - t0);
    return true;
  }

  // Fold one quantum's render time into the EMA + peak, and post a `cpu` message
  // every `_cpuReportEvery` quanta. `load` is render_time / quantum_budget, so
  // 1.0 == the audio thread used its entire deadline (xrun risk above that).
  _accumCpu(dtMs) {
    if (this._cpuBudgetMs <= 0) return;
    const load = dtMs / this._cpuBudgetMs;
    this._cpuEma = this._cpuEma * 0.9 + load * 0.1;
    if (load > this._cpuPeak) this._cpuPeak = load;
    if (++this._cpuCount >= this._cpuReportEvery) {
      this.port.postMessage({ type: "cpu", load: this._cpuEma, peak: this._cpuPeak });
      this._cpuCount = 0;
      this._cpuPeak = 0;
    }
  }
}

registerProcessor("vxn-host-processor", VxnHostProcessor);
