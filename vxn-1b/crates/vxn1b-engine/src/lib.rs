//! VXN1b engine — the matrix-modulated variant of VXN1's sound engine.
//!
//! The DSP kernels are **reused verbatim** from VXN1's [`vxn_dsp`] (ADR 0001 §1);
//! what diverges is the routing: VXN1's fixed per-channel modulation is replaced
//! by a generic mod matrix. Module map:
//!
//! - [`params`] — the flat param table (0200): `ParamId` = CLAP id = index.
//! - [`voice`] — the MPE-aware 16-voice allocation + pressure spine (0198/0199).
//! - [`matrix`] — the mod-matrix data model + default patch (0201).
//! - [`eval`] — the generic source→dest evaluator (0202).
//! - [`render`] — maps evaluated dest totals onto VXN1's DSP consumption points.
//! - [`bank`] — an 8-wide matrix-driven render bank (fork of VXN1's `VoiceBank`).
//! - [`mod_smoothing`] — per-lane discontinuity guards on the pitch/PWM/Amp
//!   dests, so stepped sources don't click the block-rate matrix apply (0208).
//! - [`engine`] — the full synth: params + matrix + voices + two banks.
//! - [`state`] — the binary `clap.state` blob (params + matrix topology, 0203).
//! - [`preset`] — the portable sparse-TOML preset codec (0203).

pub mod bank;
pub mod engine;
pub mod eval;
pub mod fx;
pub mod matrix;
pub mod mod_smoothing;
pub mod params;
pub mod preset;
pub mod render;
pub mod shared;
pub mod state;
pub mod voice;

pub use bank::{BlockCtx, RenderBank};
pub use engine::Engine;
pub use eval::{DestVals, SourceInputs, SourceVals, eval_dests, eval_sources};
pub use fx::{FxChain, FxParams};
pub use matrix::{Curve, DestId, MatrixSlot, MatrixTable, SourceId, default_patch};
pub use params::{ParamId, Params, TOTAL_PARAMS, desc_for_clap_id};
pub use preset::{Meta, PresetError, from_toml_str, read_preset, write_preset};
pub use shared::SharedParams;
pub use state::PluginState;
pub use voice::Voices;

/// Maximum simultaneous voices, inherited from the shared DSP crate — VXN1b's
/// voice count is identical to VXN1's (ADR 0001 §1): two 8-lane banks.
pub const MAX_VOICES: usize = vxn_dsp::MAX_VOICES;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inherits_voice_count_from_shared_dsp() {
        assert_eq!(MAX_VOICES, vxn_dsp::MAX_VOICES);
        assert_eq!(MAX_VOICES, 2 * RenderBank::LANES);
    }
}
