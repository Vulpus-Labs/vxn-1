// AudioWorkletProcessor for the production audio-host.
//
// Worklet shell around the shared render/lifecycle runner.
//
// AudioWorklet module scope supports static ESM imports (resolved by
// audioWorklet.addModule) but has no fetch: the main thread hands us the wasm
// bytes plus the ring/store SABs through processorOptions. Instantiation is the
// raw WebAssembly.instantiate from 0034 — no wasm-bindgen.

import { WorkletHostRunner } from "./host-runner.mjs";

// ---- render-load meter clock ----------------------------------------------
// Best available wall clock in AudioWorkletGlobalScope. `performance.now()` is
// high-resolution but historically absent from the worklet scope
// (WebAudio/web-audio-api#2413); `Date.now()` (~1ms) is always present. We do NOT
// fall back to a constant 0.
// With Date.now()'s coarse resolution a single sub-ms quantum reads 0/1ms, but
// accumulated per-quantum over a window it converges to the true mean: a render
// crosses a millisecond boundary with probability proportional to its duration.
const CPU_CLOCK =
  typeof performance !== "undefined" && typeof performance.now === "function"
    ? { now: () => performance.now(), kind: "performance" }
    : { now: () => Date.now(), kind: "date" };

class VxnHostProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    this.alive = true;

    const opts = options.processorOptions;

    // ---- render-load meter (CPU %) ----------------------------------------
    // Sum render time over a window of quanta and divide by the window's
    // wall-clock budget; report the mean load. Windowed (not per-quantum) so the
    // coarse Date.now() path averages out. DISABLED on hosts with no render-
    // thread slack (Safari, via processorOptions.cpuMeter=false): there the
    // per-quantum Date.now() + periodic postMessage can itself glitch the audio.
    this._cpuEnabled = opts.cpuMeter !== false;
    this._cpuAccum = 0; // summed render ms this window
    this._cpuQuanta = 0;
    this._cpuWindow = 64; // ~170ms @ 48k/128 — ~6 Hz reporting
    this._cpuEma = 0; // smoothed mean load across windows (displayed number)
    this._cpuEmaInit = false;
    this._cpuPeakHold = 0; // decaying peak of *window* loads (bar), never per-quantum
    this._cpuClockLogged = false;
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
    // Meter disabled (Safari): render with ZERO extra render-thread work — no
    // clock reads, no accumulation, no postMessage.
    if (!this._cpuEnabled) {
      this.runner.process(out[0], out[1]);
      return true;
    }
    const t0 = CPU_CLOCK.now();
    this.runner.process(out[0], out[1]); // silence-until-ready + trap-safe
    this._accumCpu(CPU_CLOCK.now() - t0, out[0] ? out[0].length : 128);
    return true;
  }

  // Accumulate render time over a window, then once per window derive a single
  // load figure and post a smoothed `cpu` message. We deliberately never look at
  // a single quantum's dt: on the coarse date clock it is only ever 0 or ~1ms,
  // which as an instantaneous load is meaningless (1ms / 2.67ms ≈ 37%). The
  // window mean is the only stable estimator; an EMA across windows tames the
  // residual quantisation noise, and the bar's "peak" is a slow decaying hold of
  // *window* loads — not a per-quantum spike.
  _accumCpu(dtMs, frames) {
    this._cpuAccum += dtMs;
    if (++this._cpuQuanta < this._cpuWindow) return;

    const budgetMs = (frames / sampleRate) * 1000; // per-quantum wall budget
    const windowLoad = budgetMs > 0 ? this._cpuAccum / (this._cpuQuanta * budgetMs) : 0;

    // EMA (α 0.2 → ~5-window time constant, ~0.8s) for the displayed number.
    this._cpuEma = this._cpuEmaInit ? this._cpuEma * 0.8 + windowLoad * 0.2 : windowLoad;
    this._cpuEmaInit = true;
    // Peak-hold: jump up to a hot window immediately, decay ~0.88/window (~1.3s).
    this._cpuPeakHold = Math.max(windowLoad, this._cpuPeakHold * 0.88);

    const msg = { type: "cpu", load: this._cpuEma, peak: this._cpuPeakHold };
    if (!this._cpuClockLogged) { msg.clock = CPU_CLOCK.kind; this._cpuClockLogged = true; }
    this.port.postMessage(msg);
    this._cpuAccum = 0;
    this._cpuQuanta = 0;
  }
}

registerProcessor("vxn-host-processor", VxnHostProcessor);
