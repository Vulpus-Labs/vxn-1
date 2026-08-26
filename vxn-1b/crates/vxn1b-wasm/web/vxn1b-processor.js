// AudioWorkletProcessor for the production audio host (0289).
//
// A worklet shell around the shared render/lifecycle runner. AudioWorklet module
// scope supports static ESM imports (resolved by `audioWorklet.addModule`) but
// has no `fetch`: the main thread hands over the wasm bytes and the three SABs
// through `processorOptions`. Instantiation is a raw `WebAssembly.instantiate` —
// no wasm-bindgen, which could not run here anyway.

import { WorkletHostRunner } from "./host-runner.mjs";

// Best available wall clock in AudioWorkletGlobalScope for the render-load meter
// (0309). `performance.now()` is high-res but historically absent from the
// worklet scope; `Date.now()` (~1ms) is always present. Never fall back to a
// constant — vxn-1's original meter did, and read 0 everywhere. Over a window of
// quanta the coarse clock converges to the true mean.
const CPU_CLOCK =
  typeof performance !== "undefined" && typeof performance.now === "function"
    ? { now: () => performance.now(), kind: "performance" }
    : { now: () => Date.now(), kind: "date" };

class Vxn1bHostProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    this.alive = true;

    const opts = options.processorOptions;

    // ---- render-load meter (CPU %) ----------------------------------------
    // Sum render time over a window of quanta and divide by the window's
    // wall-clock budget. Windowed so the coarse date clock averages out.
    // DISABLED on hosts with no render-thread slack (Safari, via
    // processorOptions.cpuMeter=false): there the per-quantum timing and the
    // periodic postMessage can themselves glitch the audio, which is the whole
    // problem the meter exists to show ([[vxn1-web-safari-audioworklet]]).
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
    // Meter disabled (Safari): render with ZERO extra render-thread work — not
    // even a clock read.
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
  // per window. Never look at a single quantum's dt: on the date clock it is only
  // 0 or ~1 ms. The window mean is the only stable estimator, and an EMA across
  // windows tames the residual quantisation noise.
  _accumCpu(dtMs, frames) {
    this._cpuAccum += dtMs;
    if (++this._cpuQuanta < this._cpuWindow) return;

    const budgetMs = (frames / sampleRate) * 1000;
    const windowLoad = budgetMs > 0 ? this._cpuAccum / (this._cpuQuanta * budgetMs) : 0;

    this._cpuEma = this._cpuEmaInit ? this._cpuEma * 0.8 + windowLoad * 0.2 : windowLoad;
    this._cpuEmaInit = true;
    // Peak decays rather than latching, so a one-off spike shows and then clears.
    this._cpuPeakHold = Math.max(windowLoad, this._cpuPeakHold * 0.88);

    const msg = { type: "cpu", load: this._cpuEma, peak: this._cpuPeakHold };
    // Report which clock we got, once — a `date` clock means the reading is a
    // window mean of 1ms steps, not a precise figure.
    if (!this._cpuClockLogged) {
      msg.clock = CPU_CLOCK.kind;
      this._cpuClockLogged = true;
    }
    this.port.postMessage(msg);
    this._cpuAccum = 0;
    this._cpuQuanta = 0;
  }
}

registerProcessor("vxn1b-host-processor", Vxn1bHostProcessor);
