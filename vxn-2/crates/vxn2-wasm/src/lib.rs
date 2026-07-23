//! `vxn2-wasm` — VXN2 WASM/browser audio engine.
//!
//! Production surface: [`host`] (`vxn_host_*` exports) + [`codec`] (16-byte
//! binary event codec), driven by the JS SAB event ring into the FM
//! [`vxn2_engine::engine::Engine`].
//!
//! Boundary note: JS copies the ring's wire bytes into linear memory once per
//! quantum. Params ride the atomic [`SharedParams`] store and fold into the
//! engine once at block start via [`Engine::snapshot_params`], so a mid-block
//! `EV_PARAM` lands at the *next* quantum. `process_block` samples its
//! block-rate state (LFOs, matrix) once per call and asserts `len <=
//! CONTROL_BLOCK`, so each event-sliced region is chunked into ≤[`CONTROL_BLOCK`]
//! pieces.

// Binary event codec (Rust half). The JS half is `web/event-codec.mjs` and must
// stay byte-identical.
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
