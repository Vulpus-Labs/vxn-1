//! Re-export of `vxn_core_dsp::reverb`.
//!
//! Moved by ticket 0230 (epic E041). vxn-1b's copy shared this one's topology,
//! `BASE_MS` table, LFO scheme *and* mix law; the only real difference was that
//! vxn-1b delegated bypass to an outer crossfade while this one owns it
//! internally. That internal form is the canon, so the move left vxn-2's render
//! bit-identical.
//!
//! `FdnReverb` now implements `vxn_core_dsp::fx::FxKernel`, so `new`,
//! `set_params`, `process`, `reset`, `clear` and `is_active` arrive through the
//! trait — hence the re-export of `FxKernel` alongside it.
pub use vxn_core_dsp::fx::FxKernel;
pub use vxn_core_dsp::reverb::{FdnReverb, FdnReverbParams};
