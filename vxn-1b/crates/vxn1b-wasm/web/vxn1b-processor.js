// AudioWorkletProcessor for the production audio host (0289).
//
// A worklet shell around the shared render/lifecycle runner. AudioWorklet module
// scope supports static ESM imports (resolved by `audioWorklet.addModule`) but
// has no `fetch`: the main thread hands over the wasm bytes and the three SABs
// through `processorOptions`. Instantiation is a raw `WebAssembly.instantiate` —
// no wasm-bindgen, which could not run here anyway.

import { WorkletHostRunner } from "./host-runner.mjs";

// Best available wall clock in AudioWorkletGlobalScope. `performance.now()` is
// high-resolution but historically absent from the worklet scope
// (WebAudio/web-audio-api#2413); `Date.now()` (~1 ms) is always present. There is
// deliberately no constant-0 fallback — a meter that reads zero because it has no
// clock is worse than no meter.
const CPU_CLOCK =
  typeof performance !== "undefined" && typeof performance.now === "function"
    ? { now: () => performance.now(), kind: "performance" }
    : { now: () => Date.now(), kind: "date" };

class Vxn1bHostProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    this.alive = true;

    const opts = options.processorOptions;

    // ---- render-load meter (CPU %) ---------------------------------------
    // Sum render time over a window of quanta and divide by the window's
    // wall-clock budget. Windowed rather than per-quantum so the coarse
    // Date.now() path averages out: a single sub-millisecond quantum reads 0 or
    // 1 ms, which as an instantaneous load is meaningless, but accumulated over
    // a window it converges on the true mean.
    //
    // Disabled on hosts with no render-thread slack (Safari, via
    // processorOptions.cpuMeter === false): there the per-quantum clock read plus
    // the periodic postMessage can itself glitch the audio.
    this._cpuEnabled = opts.cpuMeter !== false;
    this._cpuAccum = 0;
    this._cpuQuanta = 0;
    this._cpuWindow = 64; // ~170 ms @ 48k/128 — ~6 Hz reporting
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
    // Meter disabled: render with ZERO extra render-thread work — no clock
    // reads, no accumulation, no postMessage.
    if (!this._cpuEnabled) {
      this.runner.process(out[0], out[1]);
      return true;
    }
    const t0 = CPU_CLOCK.now();
    this.runner.process(out[0], out[1]); // silence-until-ready + trap-safe
    this._accumCpu(CPU_CLOCK.now() - t0, out[0] ? out[0].length : 128);
    return true;
  }

  /// Accumulate render time over a window, then derive one load figure and post
  /// a smoothed `cpu` message. A single quantum's dt is never used: on the
  /// coarse clock it is only ever 0 or ~1 ms, and 1 ms against a 2.67 ms budget
  /// reads as 37% for what may have been a trivial render.
  _accumCpu(dtMs, frames) {
    this._cpuAccum += dtMs;
    if (++this._cpuQuanta < this._cpuWindow) return;

    const budgetMs = (frames / sampleRate) * 1000; // per-quantum wall budget
    const windowLoad = budgetMs > 0 ? this._cpuAccum / (this._cpuQuanta * budgetMs) : 0;

    // EMA (α 0.2 → ~5-window time constant, ~0.8 s) for the displayed number.
    this._cpuEma = this._cpuEmaInit ? this._cpuEma * 0.8 + windowLoad * 0.2 : windowLoad;
    this._cpuEmaInit = true;
    // Peak-hold: jump straight to a hot window, decay ~0.88/window (~1.3 s).
    this._cpuPeakHold = Math.max(windowLoad, this._cpuPeakHold * 0.88);

    const msg = { type: "cpu", load: this._cpuEma, peak: this._cpuPeakHold };
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
