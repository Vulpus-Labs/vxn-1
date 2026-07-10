// AudioWorkletProcessor for the vxn-2 production audio-host (ticket 0156).
//
// The worklet shell around the shared lifecycle runner (host-runner.mjs):
// instantiate-from-bytes, silence-until-ready, sample-rate, suspend/resume reset,
// teardown, and render-thread trap safety. Ported from vxn-1's vxn-processor;
// the vxn-2 change is that there's no key-mode/split shared state on the port.
//
// AudioWorklet module scope supports static ESM imports (resolved by
// audioWorklet.addModule) but has no fetch: the main thread hands us the wasm
// bytes plus the ring/store SABs through processorOptions. Instantiation is the
// raw WebAssembly.instantiate — no wasm-bindgen.

import { WorkletHostRunner } from "./host-runner.mjs";

// Best available wall clock in AudioWorkletGlobalScope for the render-load meter.
// `performance.now()` is high-res but historically absent from the worklet scope;
// `Date.now()` (~1ms) is always present. We never fall back to a constant 0 (the
// original meter's bug — it read 0 everywhere). Over a window of quanta the
// coarse date clock converges to the true mean.
const CPU_CLOCK =
  typeof performance !== "undefined" && typeof performance.now === "function"
    ? { now: () => performance.now(), kind: "performance" }
    : { now: () => Date.now(), kind: "date" };

class Vxn2HostProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    this.alive = true;

    const opts = options.processorOptions;

    // ---- render-load meter (CPU %) ----------------------------------------
    // Sum render time over a window of quanta and divide by the window's wall-
    // clock budget; report the mean load. Windowed so the coarse Date.now() path
    // averages out. DISABLED on hosts with no render-thread slack (Safari, via
    // processorOptions.cpuMeter=false): there the per-quantum timing + periodic
    // postMessage can itself glitch the audio.
    this._cpuEnabled = opts.cpuMeter !== false;
    this._cpuAccum = 0;
    this._cpuQuanta = 0;
    this._cpuWindow = 64; // ~170ms @ 48k/128 — ~6 Hz reporting
    this._cpuEma = 0;
    this._cpuEmaInit = false;
    this._cpuPeakHold = 0;
    this._cpuClockLogged = false;

    this.runner = new WorkletHostRunner({
      wasmBytes: opts.wasmBytes,
      ringSab: opts.ringSab,
      storeSab: opts.storeSab, // optional
      sampleRate, // worklet global
      capacity: opts.capacity,
      onReady: () => this.port.postMessage({ type: "ready" }),
      onTrap: (e, count) =>
        this.port.postMessage({ type: "trap", message: String((e && e.message) || e), count }),
    });

    // Controller -> worklet lifecycle messages. Params/notes flow over the SABs,
    // not the port. vxn-2 has no key-mode/split shared state.
    this.port.onmessage = (e) => {
      const m = e.data;
      switch (m.type) {
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
    // Meter disabled (Safari): render with ZERO extra render-thread work.
    if (!this._cpuEnabled) {
      this.runner.process(out[0], out[1]);
      return true;
    }
    const t0 = CPU_CLOCK.now();
    this.runner.process(out[0], out[1]); // silence-until-ready + trap-safe
    this._accumCpu(CPU_CLOCK.now() - t0, out[0] ? out[0].length : 128);
    return true;
  }

  // Accumulate render time over a window, then derive one smoothed load figure
  // per window. Never look at a single quantum's dt: on the date clock it's only
  // 0 or ~1ms. The window mean is the only stable estimator; an EMA across
  // windows tames the residual quantisation noise.
  _accumCpu(dtMs, frames) {
    this._cpuAccum += dtMs;
    if (++this._cpuQuanta < this._cpuWindow) return;

    const budgetMs = (frames / sampleRate) * 1000;
    const windowLoad = budgetMs > 0 ? this._cpuAccum / (this._cpuQuanta * budgetMs) : 0;

    this._cpuEma = this._cpuEmaInit ? this._cpuEma * 0.8 + windowLoad * 0.2 : windowLoad;
    this._cpuEmaInit = true;
    this._cpuPeakHold = Math.max(windowLoad, this._cpuPeakHold * 0.88);

    const msg = { type: "cpu", load: this._cpuEma, peak: this._cpuPeakHold };
    if (!this._cpuClockLogged) { msg.clock = CPU_CLOCK.kind; this._cpuClockLogged = true; }
    this.port.postMessage(msg);
    this._cpuAccum = 0;
    this._cpuQuanta = 0;
  }
}

registerProcessor("vxn2-host-processor", Vxn2HostProcessor);
