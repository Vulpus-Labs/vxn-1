//! Re-export of `vxn_core_dsp::delay`.
//!
//! This kernel moved to `vxn-core-dsp` in ticket 0231 (epic E041) — vxn-2's copy
//! was the superset of vxn-1b's, so it became the shared canon and the move
//! leaves this crate's render bit-identical. Three API notes for call sites:
//!
//! - `StereoDelay` now implements `vxn_core_dsp::fx::FxKernel`, so `new`,
//!   `set_params`, `process`, `reset`, `clear` and `is_active` come in through
//!   that trait — hence the `pub use` below.
//! - **Tempo is pushed, not passed.** `set_params(&p, tempo_bpm)` is now
//!   `set_tempo(tempo_bpm)` followed by `set_params(&p)`, because `FxKernel`
//!   fixes the one-argument shape. Call `set_tempo` first; a delay with
//!   `sync = false` ignores it.
//! - `StereoDelayParams` gained a `damping` field (vxn-1b's feedback-path HF
//!   damping). vxn-2 passes `0.0`, which **skips** the filter rather than
//!   running it flat — the gate is what keeps this move bit-exact.
//!
//! `synced_seconds` is gone: it was a second spelling of
//! `vxn_core_utils::sync::subdivision_seconds`, which the shared kernel uses.
pub use vxn_core_dsp::delay::{
    MAX_DELAY_MS, MAX_DELAY_S, MAX_FEEDBACK, MIN_DELAY_MS, StereoDelay, StereoDelayParams,
};
pub use vxn_core_dsp::fx::FxKernel;
