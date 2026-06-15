# vxn-wasm — WASM/browser feasibility spike (ticket 0034)

Throwaway spike proving vxn-1 can run in a browser. **Verdict: GO.**
vxn-engine compiles to `wasm32-unknown-unknown` with **zero source
changes** and renders correct audio inside an `AudioWorkletProcessor`.

## What's here

- `src/lib.rs` — raw C-ABI `cdylib` wrapping `vxn_engine::Synth`. No
  wasm-bindgen (its fetch/ESM glue fights the AudioWorklet scope); the
  module instantiates with a plain `WebAssembly.instantiate`.
- `harness.mjs` — Node harness that drives the wasm exactly as the worklet
  does. Proves the render path end-to-end headlessly (audio + throughput +
  denormal probe).
- `web/` — browser deliverable: `index.html` + `vxn-processor.js` worklet
  + the built `vxn_wasm.wasm`.

## Build + run

```bash
# 1. build the wasm
cargo build -p vxn-wasm --target wasm32-unknown-unknown --release
cp target/wasm32-unknown-unknown/release/vxn_wasm.wasm \
   vxn-1/crates/vxn-wasm/web/vxn_wasm.wasm

# 2a. headless proof (no browser needed)
node vxn-1/crates/vxn-wasm/harness.mjs

# 2b. browser: serve web/ over http (AudioWorklet needs http, not file://)
cd vxn-1/crates/vxn-wasm/web && python3 -m http.server 8080
# open http://localhost:8080 -> "Start audio" -> hold "A4"
```

## Main-thread coordinator (ticket 0042)

`web/coordinator.mjs` — `class WebHost`, the main-side half of E015: it creates
the `AudioContext`, loads the worklet, allocates the event-ring + param-store
SABs, seeds the store with the engine's defaults (so the worklet's first fold
doesn't zero every param), hands the worklet its wasm bytes, and exposes the
producer surface (`noteOn`/`noteOff`/`setParam`/…). `vxn_host_get_param` was
added to the C-ABI so the coordinator can snapshot defaults pre-controller.

```bash
# headless proof (fake AudioContext runs the REAL runner over the same SABs)
node vxn-1/crates/vxn-wasm/harness-0042.mjs

# browser: cargo xtask web bundles coordinator.mjs + a booting index.html into
# target/web-dist/ — serve it with COOP/COEP (ticket 0045), click Start, hold A4.
```

## Findings

| Question | Result |
|----------|--------|
| Engine compiles to wasm32? | ✅ Zero source changes. `std::fs` preset I/O compiles (wasm std stubs it); audio path never calls it. |
| Renders audible audio? | ✅ A4 note-on peaks 0.27; silent before note-on. |
| Throughput (Node, 1 voice) | ~55× realtime. |
| Denormal cliff on decay tail? | ❌ None. Silent-skip fast path drives buffers to **exact 0.0** post-decay, so denormals never accumulate — no manual FTZ flush needed for the release case. See `vxn1-silent-skip-filter-state`. |
| AudioWorklet wiring | Main thread fetches wasm bytes + worklet module, passes bytes via `processorOptions`; worklet instantiates and renders one quantum (128 frames) per `process()` straight out of linear memory. No `SharedArrayBuffer` needed for this spike → no COOP/COEP isolation headers required. |

## Caveats / not covered (out of scope per ticket)

- Browser playback wiring is built and JS syntax-checked; the Node harness
  proves the identical render path. Open `web/` in a browser to confirm
  audibility on a real audio device.
- Denormal probe stressed the **release/decay** path (driven to zero by
  silent-skip). A held *quiet sustain* into reverb feedback is the only
  theoretical remaining denormal case; not isolated here, judged low-risk.
- Throughput measured native/Node (same hardware FP as browser wasm).
  WASM SIMD128 ≠ NEON; full 16-voice poly perf on a real browser, esp.
  mobile, still needs measuring.
- Single-threaded; note events over `port.postMessage` (fine for a spike,
  jitter-prone for tight timing — a real port wants SAB + a ring buffer).

## Effort estimate for a full port (fork at the `Synth` boundary)

- Engine/DSP to wasm + denormal hardening: ~1 wk (mostly done; silent-skip
  already covers the common case).
- AudioWorklet + wasm glue, productionised (SAB ring buffer for params/
  notes, COOP/COEP, lifecycle): ~2 wk — the real work.
- UI rewire (existing JSON opcode protocol over `postMessage` instead of
  wry `evaluate_script`) + Web MIDI + IndexedDB presets: ~1–2 wk.

Total ~4–5 wk. No architectural rewrite — the plugin shell (clack/wry) is
replaced wholesale by Web Audio + DOM; the engine ports as-is.
