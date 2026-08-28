//! Re-export of `vxn_core_dsp::phaser`.
//!
//! This kernel moved to `vxn-core-dsp` in ticket 0228 (epic E041) — vxn-2's
//! copy was the superset of vxn-1b's, so it became the shared canon and the
//! move left this crate's render bit-identical.
//!
//! Two API notes for call sites:
//!
//! - `StereoPhaser` now implements `vxn_core_dsp::fx::FxKernel`, so `new`,
//!   `set_params`, `process`, `reset`, `clear` and `is_active` come in through
//!   that trait — hence the `pub use` below.
//! - The old positional `set_params(rate, depth, fb, mix, spread)` is now
//!   `set_swept(rate, depth, fb, spread)` plus the mix/enable pair carried by
//!   `PhaserParams`; `set_from` is unchanged in behaviour and kept here as an
//!   alias for `FxKernel::set_params`.
pub use vxn_core_dsp::fx::FxKernel;
pub use vxn_core_dsp::phaser::{PhaserParams, StereoPhaser};
