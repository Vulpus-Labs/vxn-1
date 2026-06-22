//! `vxn-wasm` — WASM/browser audio engine.
//!
//! The production surface is [`host`] (`vxn_host_*` exports, ticket 0038 +
//! onwards) and [`codec`] (binary event codec, ticket 0037). They are driven
//! by the JS side in `web/vxn-processor.js` → `web/audio-host.mjs` →
//! `web/coordinator.mjs`.
//!
//! The 0034/0035 spike `Instance` API (`vxn_new` / `vxn_process` / …) that
//! previously lived here has been removed — superseded by `host.rs`.

// 0037: binary event codec (Rust half). Typed encode/decode over the 0035
// 16-byte slot framing, plus `apply(event, &mut Synth)` with dispatch parity
// to vxn-core-clap::dispatch_event. The JS half is web/event-codec.mjs.
pub mod codec;

// 0038: worklet audio-host — the production render loop. Owns the Synth, a
// linear-memory event-decode scratch and the output buffers, and ports the CLAP
// batch loop (vxn-clap/src/lib.rs:286-390) into one `vxn_host_render` call per
// quantum: set non-automatable shared state, slice the block at event offsets,
// decode+apply via `codec`, render each slice. Supersedes the per-slice JS loop
// the 0035 spike drove from outside.
pub mod host;

/// Web Audio render-quantum size. AudioWorklet always calls `process()`
/// with 128-frame planar buffers.
pub(crate) const QUANTUM: usize = 128;
