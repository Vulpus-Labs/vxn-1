// Perf-bench AudioWorkletProcessor (ticket 0087, epic E020).
//
// Measures the worst-case render cost of the engine wasm AT FULL POLYPHONY, in
// the AudioWorklet render thread — the only place that reflects real audio-
// callback scheduling. It instantiates the bench wasm (raw WebAssembly.instantiate,
// no wasm-bindgen — same as vxn-processor-0038.js) and, inside process(), brackets
// a batch of `vxn_bench_render` calls with performance.now(), collecting per-
// quantum render times. After a warmup + a measurement window it posts the
// distribution {mean, p50, p95, max} ms and the budget, then renders silence.
//
// Why the worklet and not Node: wasm32-unknown-unknown has no std::time, so the
// bench cannot time itself; and only the render thread sees the true scheduling.
// The clock is performance.now() when present (high-res), else Date.now() — the
// SAME fallback discipline as the production CPU meter (vxn-processor-0038.js).

const CLOCK =
  typeof performance !== "undefined" && typeof performance.now === "function"
    ? { now: () => performance.now(), kind: "performance" }
    : { now: () => Date.now(), kind: "date" };

class VxnPerfProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const opts = options.processorOptions || {};

    // How many quanta to render per process() call inside one timed bracket.
    // Batching amortises the JS<->wasm call overhead and the clock read so the
    // figure is dominated by render cost, not measurement noise.
    this.batch = opts.batch || 64;
    // Quanta to render-and-discard before measuring (let caches/branch
    // predictors and the FX tails reach steady state).
    this.warmupQuanta = opts.warmupQuanta || 4000;
    // Quanta to actually measure.
    this.measureQuanta = opts.measureQuanta || 20000;

    this.samples = []; // per-quantum render time (ms), one per quantum measured
    this.warmedUp = 0;
    this.measured = 0;
    this.done = false;
    this.ready = false;
    this.simd128 = null;

    // Instantiate the bench wasm from bytes handed in by the main thread (the
    // worklet scope has no fetch). Async; process() renders silence until ready.
    WebAssembly.instantiate(opts.wasmBytes, {})
      .then(({ instance }) => {
        this.x = instance.exports;
        this.bench = this.x.vxn_bench_new(sampleRate); // worklet global
        this.simd128 = this.x.vxn_bench_simd128() === 1;
        this.ready = true;
        this.port.postMessage({ type: "ready", simd128: this.simd128, clock: CLOCK.kind });
      })
      .catch((e) => this.port.postMessage({ type: "error", message: String((e && e.message) || e) }));
  }

  process(_inputs, outputs) {
    const out = outputs[0];
    // Silence the node output regardless — this is a measurement node, not an
    // audible one.
    if (out && out[0]) out[0].fill(0);
    if (out && out[1]) out[1].fill(0);

    if (!this.ready || this.done) return true;

    // Warmup: render without measuring.
    if (this.warmedUp < this.warmupQuanta) {
      const n = Math.min(this.batch, this.warmupQuanta - this.warmedUp);
      this.x.vxn_bench_render(this.bench, n);
      this.warmedUp += n;
      return true;
    }

    // Measurement window: time each batch, attribute the mean per-quantum time
    // to every quantum in the batch (the batch is the timed unit).
    const n = Math.min(this.batch, this.measureQuanta - this.measured);
    const t0 = CLOCK.now();
    this.x.vxn_bench_render(this.bench, n);
    const dt = CLOCK.now() - t0; // ms for the whole batch
    const perQuantum = dt / n;
    for (let i = 0; i < n; i++) this.samples.push(perQuantum);
    this.measured += n;

    if (this.measured >= this.measureQuanta) {
      this.done = true;
      this.port.postMessage(this._report());
    }
    return true;
  }

  _report() {
    const s = this.samples.slice().sort((a, b) => a - b);
    const at = (p) => s[Math.min(s.length - 1, Math.floor(p * s.length))];
    const mean = s.reduce((a, b) => a + b, 0) / (s.length || 1);
    const budgetMs = (128 / sampleRate) * 1000; // realtime budget per quantum
    return {
      type: "result",
      simd128: this.simd128,
      clock: CLOCK.kind,
      quanta: this.measured,
      batch: this.batch,
      meanMs: mean,
      p50Ms: at(0.5),
      p95Ms: at(0.95),
      maxMs: s[s.length - 1],
      budgetMs,
      // Headroom = budget / p95. >1 means worst-case-ish quanta fit the budget.
      headroom: budgetMs / (at(0.95) || budgetMs),
    };
  }
}

registerProcessor("vxn-perf-processor", VxnPerfProcessor);
