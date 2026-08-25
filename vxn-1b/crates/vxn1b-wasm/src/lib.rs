//! `vxn1b-wasm` — VXN1b's WASM/browser audio engine (0286, epic E045).
//!
//! The production surface is [`host`] (`vxn1b_host_*` exports) and [`codec`]
//! (the binary event codec). They are driven from JS by the worklet processor →
//! audio-host → coordinator chain (0287-0289).
//!
//! Raw C ABI, no wasm-bindgen: the module has to instantiate inside an
//! `AudioWorkletGlobalScope`, which has no DOM, no `fetch` and none of the glue
//! wasm-bindgen expects to generate around it.

// Binary event codec (Rust half). Typed encode/decode over the 16-byte slot
// framing vxn-1 froze in 0035, plus `apply(event, &mut Engine)` with dispatch
// parity to vxn1b-clap's bespoke `dispatch`. The JS half lands in 0287.
pub mod codec;

// Worklet audio-host — the production render loop. Owns the Engine, a
// linear-memory event-decode scratch and the output buffers, and ports the CLAP
// batch loop into one `vxn1b_host_render` call per quantum.
pub mod host;

/// Web Audio render-quantum size. AudioWorklet always calls `process()` with
/// 128-frame planar buffers.
pub(crate) const QUANTUM: usize = 128;
