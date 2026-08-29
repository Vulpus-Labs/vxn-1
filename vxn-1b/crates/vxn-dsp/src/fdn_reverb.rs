//! Re-export of `vxn_core_dsp::reverb`.
//!
//! Ticket 0230 unified this with vxn-2's copy. They shared topology, the
//! `BASE_MS` table, the LFO scheme **and** the equal-power mix law — the
//! epic's claim that vxn-2 mixed linearly was read off a stale doc comment,
//! and is wrong. The only real difference was where bypass lived: here it was
//! delegated to `FxChain`'s outer crossfade, there it is an internal fade.
//!
//! So vxn-1b's reverb changes in one way only, the same way its phaser did in
//! 0228: switching the send off now glides the wet down over 30 ms inside the
//! kernel instead of crossfading the whole effect out over 10 ms from outside.
//! The tail rings through the fade either way. Steady-state audio — the sound
//! of the reverb itself at any fixed mix — is unchanged.
pub use vxn_core_dsp::fx::FxKernel;
pub use vxn_core_dsp::reverb::{FdnReverb, FdnReverbParams};
