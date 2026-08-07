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
//! - [`synth`] — the core synth as an instantiable unit: params + matrix +
//!   voices + per-layer LFO 2 + two banks (0214, ADR 0002 §1).
//! - [`engine`] — the global block: 2 × [`synth::Synth`] + the one global FX
//!   chain + master.
//! - [`state`] — the binary `clap.state` blob (params + matrix topology, 0203).
//! - [`preset`] — the portable sparse-TOML preset codec (0203).
//! - [`meters`] — meter frames over the shared lock-free bus (0240).

pub mod bank;
pub mod engine;
pub mod eval;
pub mod fx;
pub mod matrix;
pub mod meters;
pub mod mod_smoothing;
pub mod output;
pub mod params;
pub mod preset;
pub mod preset_io;
pub mod render;
pub mod shared;
pub mod state;
pub mod synth;
pub mod voice;

pub use bank::{BlockCtx, RenderBank};
pub use engine::{
    Engine, KeyMode, KeyOp, KeyState, MatrixEdit, MatrixField, DEFAULT_SPLIT_POINT,
};
pub use eval::{DestVals, SourceInputs, SourceVals, eval_dests, eval_sources};
pub use fx::{FxChain, FxParams};
pub use matrix::{
    Curve, DestId, MatrixSlot, MatrixSnapshot, MatrixTable, SourceId, default_patch,
};
pub use meters::MeterFrame;
pub use vxn_core_utils::{MeterBus, MeterTap};
pub use params::{
    ClapRef, GLOBAL_PARAMS, Layer, PATCH_COUNT, PATCH_PARAMS, ParamId, Params, TOTAL_PARAMS,
    clap_id_of, clap_module, clap_ref, desc_for_clap_id,
};
pub use preset::{Meta, PresetError, read_preset, write_preset};
pub use preset_io::EnginePresetStore;
pub use shared::SharedParams;
pub use state::{LayerState, PluginState};
pub use synth::Synth;
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
