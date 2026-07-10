//! `vxn2-wasm` — VXN2 WASM/browser audio engine (ticket 0153, epic E030).
//!
//! The production surface is [`host`] (`vxn_host_*` exports) and [`codec`]
//! (the 16-byte binary event codec). They are driven by the JS side in
//! `web/vxn2-processor.js` → coordinator → SAB event ring (tickets 0155/0156).
//!
//! This is the vxn-2 analogue of vxn-1's `vxn-wasm`, retargeted to the FM
//! [`vxn2_engine::engine::Engine`]. Two structural differences from vxn-1 fall
//! straight out of the vxn-2 architecture and drive the whole port:
//!
//! 1. **Params fold at block start, not per-sample.** vxn-2 has no per-id
//!    `Engine::set_param`; params ride the atomic [`SharedParams`] store and the
//!    engine folds them once per block via
//!    [`Engine::snapshot_params`](vxn2_engine::engine::Engine::snapshot_params)
//!    (which calls `apply_block_params`). The host owns a `SharedParams`, writes
//!    param edits into it, and folds at the top of each render — exactly the
//!    shape of the `vxn2-clap` process loop (the single source of render truth).
//!    Mid-block `EV_PARAM` records therefore land at the *next* quantum, the
//!    same limitation the plugin documents.
//! 2. **`process_block` is CONTROL_BLOCK-bounded.** The engine samples its
//!    block-rate state (LFOs, matrix) once per `process_block` and asserts
//!    `len <= CONTROL_BLOCK`. So each event-sliced region is further chunked
//!    into ≤[`CONTROL_BLOCK`]-frame pieces — mirroring `vxn2-clap`'s
//!    `control_chunks`.
//!
//! vxn-1's key-mode / split-point shared state has **no vxn-2 analogue** (the
//! FM engine is a single voice pool, no dual/split layer), so those events and
//! the `vxn_host_render` key-mode/split args are dropped here.

// Binary event codec (Rust half). The JS half is `web/event-codec.mjs`
// (ticket 0155) and must stay byte-identical.
pub mod codec;

// Worklet audio-host: the production render loop. Owns the `Engine`, its
// `SharedParams` store, the stereo output and the event-decode scratch.
pub mod host;

/// Web Audio render-quantum size. AudioWorklet always calls `process()`
/// with 128-frame planar buffers.
pub(crate) const QUANTUM: usize = 128;

/// Max frames per `Engine::process_block` call. The engine asserts
/// `len <= CONTROL_BLOCK` (it samples block-rate state once per call), so each
/// event-sliced region is sub-chunked to this. Must equal `vxn2-clap`'s
/// `CONTROL_BLOCK` (32) — the block-rate cadence is part of the sound.
pub(crate) const CONTROL_BLOCK: usize = 32;
