// Perf-bench harness (main thread half) — ticket 0087, epic E020.
//
// Boots an AudioContext, loads the perf worklet (perf-processor.js) with the
// engine wasm bytes, and resolves with the worst-case render-time distribution
// the worklet measured. This is the BROWSER entry point for the 0087 manual
// measurement: it must run on a real page (it needs a real AudioWorklet +
// performance.now() on the render thread), not in Node.
//
// Usage (from a page on a cross-origin-isolated origin — `cargo xtask web
// --serve` provides one):
//
//   import { runPerfBench } from "./perf-harness.mjs";
//   const r = await runPerfBench();         // measures the bundled vxn-processor wasm
//   console.log(r);                         // {simd128, meanMs, p50Ms, p95Ms, maxMs, budgetMs, headroom}
//
// To compare SIMD vs scalar: build each with `cargo xtask web` and
// `cargo xtask web --scalar`, serve each, and run this on each — `r.simd128`
// labels which build was actually measured (no guessing).

const ENGINE_WASM_URL = "./vxn_wasm.wasm"; // the bundled engine module
const PERF_WORKLET_URL = "./perf-processor.js";

/**
 * Run the worst-case perf bench in the AudioWorklet and resolve with the
 * measured distribution. Options:
 *   - wasmUrl: URL of the engine `.wasm` (default the bundled `vxn_wasm.wasm`).
 *   - workletUrl: URL of `perf-processor.js`.
 *   - batch / warmupQuanta / measureQuanta: forwarded to the processor.
 *   - AudioContextClass: injectable for tests (defaults to global AudioContext).
 */
export async function runPerfBench(opts = {}) {
  const wasmUrl = opts.wasmUrl || ENGINE_WASM_URL;
  const workletUrl = opts.workletUrl || PERF_WORKLET_URL;
  const AudioCtx = opts.AudioContextClass || (typeof AudioContext !== "undefined" ? AudioContext : null);
  if (!AudioCtx) throw new Error("no AudioContext available (run this in a browser)");

  const wasmBytes = await fetch(wasmUrl).then((r) => {
    if (!r.ok) throw new Error(`failed to fetch ${wasmUrl}: ${r.status}`);
    return r.arrayBuffer();
  });

  const ctx = new AudioCtx();
  try {
    await ctx.audioWorklet.addModule(workletUrl);
    const node = new AudioWorkletNode(ctx, "vxn-perf-processor", {
      numberOfInputs: 0,
      numberOfOutputs: 1,
      outputChannelCount: [2],
      processorOptions: {
        wasmBytes,
        batch: opts.batch,
        warmupQuanta: opts.warmupQuanta,
        measureQuanta: opts.measureQuanta,
      },
    });
    node.connect(ctx.destination);
    await ctx.resume();

    const result = await new Promise((resolve, reject) => {
      const timeoutMs = opts.timeoutMs || 60000;
      const timer = setTimeout(
        () => reject(new Error(`perf bench timed out after ${timeoutMs} ms`)),
        timeoutMs,
      );
      node.port.onmessage = (e) => {
        const m = e.data;
        if (m.type === "error") {
          clearTimeout(timer);
          reject(new Error(`perf worklet error: ${m.message}`));
        } else if (m.type === "result") {
          clearTimeout(timer);
          resolve(m);
        }
        // m.type === "ready" is informational.
      };
    });
    return result;
  } finally {
    try {
      await ctx.close();
    } catch {
      /* ignore */
    }
  }
}

/** Format a result for console / on-page display. */
export function formatPerfResult(r) {
  const pct = (r.headroom * 100).toFixed(0);
  return [
    `build:    ${r.simd128 ? "SIMD128" : "scalar"} (clock: ${r.clock})`,
    `quanta:   ${r.quanta} (batch ${r.batch})`,
    `mean:     ${r.meanMs.toFixed(4)} ms/quantum`,
    `p50:      ${r.p50Ms.toFixed(4)} ms/quantum`,
    `p95:      ${r.p95Ms.toFixed(4)} ms/quantum`,
    `max:      ${r.maxMs.toFixed(4)} ms/quantum`,
    `budget:   ${r.budgetMs.toFixed(4)} ms/quantum (realtime)`,
    `headroom: ${r.headroom.toFixed(2)}x (budget / p95, ${pct}%)`,
  ].join("\n");
}
