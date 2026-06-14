# vxn-app-wasm-probe — controller-in-wasm + param-store probe (ticket 0036)

Throwaway de-risk spike feeding **ADR 0009** (controller placement +
cross-thread param store) for epic E015 (vxn-1 web/WASM port). Not shipped.

## What it proves

1. **`vxn-app`'s MVC controller compiles to `wasm32-unknown-unknown`
   unchanged.** `src/lib.rs` constructs a real `vxn_app::Controller`
   (which wraps `vxn_core_app::Controller`) with throwaway `ParamModel` +
   `PresetStore` impls, posts `UiEvent`s through the bounded channel,
   `tick()`s, and drains `ViewEvent`s — forcing the whole controller code
   path (mpsc channels, `Arc<Mutex>` corpus, param-broadcast loop,
   `Box<dyn Any>` custom-event downcast) into the binary.

2. **The two param-store mechanisms, compared.** `param_store_bench.mjs`
   is a Node prototype benchmarking (a) a `SharedArrayBuffer` of 165
   atomics indexed by CLAP id vs (b) param-set events on an SPSC byte
   ring, on bulk preset load (165 params) and the audio→main diff
   readback (the `push_param_diffs` pump).

## Reproduce

```bash
# controller-to-wasm build + size
cargo build -p vxn-app-wasm-probe --target wasm32-unknown-unknown --release
ls -l target/wasm32-unknown-unknown/release/vxn_app_wasm_probe.wasm

# param-store comparison
node vxn-1/crates/vxn-app-wasm-probe/param_store_bench.mjs
```

## Results (M1, rustc 1.95, Node 26)

- Controller wasm: **160 872 bytes raw, ~57 KB gzipped** (engine spike
  0034 is 172 KB raw for comparison — same order of magnitude).
- Bulk load: SAB ~1 µs vs ring ~84 µs (≈50–80× faster — flat 165-store
  write vs 165 framed records + drain).
- Diff readback (8 drifted params/tick): SAB ~4 µs vs ring ~4 µs (a wash).

See ADR 0009 for the decisions these numbers drove.

## Teardown

Delete this crate, drop the `"vxn-1/crates/vxn-app-wasm-probe"` line from
the workspace `Cargo.toml`, and the decisions live on in ADR 0009.
