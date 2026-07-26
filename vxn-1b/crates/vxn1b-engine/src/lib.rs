//! VXN1b engine — parameter table + mod-matrix routing loop.
//!
//! Scaffold only (0197). Later tickets fill this in: the flat param table
//! (0200), the matrix data model + evaluator (0201/0202), and the block-render
//! *routing* loop that replaces VXN1's fixed per-channel resolution
//! (ADR 0001 §1/§2). The DSP kernels themselves are **reused verbatim** from
//! VXN1's [`vxn_dsp`] — VXN1b takes a direct crate dependency rather than
//! forking (ADR 0001 §1), so those tickets *wire* kernels rather than port them.
//!
//! For now the crate exposes a minimal silent [`Engine`] so the CLAP shell can
//! load in a host and render silence; audible synthesis arrives with the param
//! table and matrix tickets. The MPE-aware voice-allocation spine
//! ([`voice::Voices`]) lands first (0198) — its per-voice `channel` + `pressure`
//! fields shape the voice struct the matrix evaluator later reads.

pub mod eval;
pub mod matrix;
pub mod params;
pub mod render;
pub mod voice;

pub use eval::{DestVals, SourceInputs, SourceVals, eval_dests, eval_sources};
pub use matrix::{Curve, DestId, MatrixSlot, MatrixTable, SourceId, default_patch};
pub use params::{ParamId, Params};
pub use voice::Voices;

/// Maximum simultaneous voices, inherited from the shared DSP crate — VXN1b's
/// voice count is identical to VXN1's (ADR 0001 §1). Referenced here so the
/// `vxn-dsp` dependency is genuinely linked, not merely declared.
pub const MAX_VOICES: usize = vxn_dsp::MAX_VOICES;

/// Placeholder audio-thread engine: renders silence into pre-allocated stereo
/// buffers. The shape — capacity fixed at construction, allocation-free
/// [`process_block`](Engine::process_block) — matches the VXN1/VXN3 engines the
/// CLAP shell expects, so later tickets grow the body without changing the shell
/// contract.
pub struct Engine {
    sample_rate: f32,
    max_frames: usize,
}

impl Engine {
    /// Build an engine for a given sample rate and maximum block size. Real
    /// voice/matrix state is allocated here in later tickets.
    pub fn new(sample_rate: f32, max_frames: usize) -> Self {
        Self {
            sample_rate,
            max_frames,
        }
    }

    /// The activated sample rate.
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// The maximum block size the engine was built for.
    pub fn max_frames(&self) -> usize {
        self.max_frames
    }

    /// Render one block into pre-allocated stereo buffers. Silent scaffold;
    /// allocation-free, matching VXN1's real-time discipline (ADR 0001).
    pub fn process_block(&mut self, left: &mut [f32], right: &mut [f32]) {
        left.fill(0.0);
        right.fill(0.0);
    }

    /// Reset any per-voice/transient state. No-op while the engine is silent.
    pub fn reset(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_silence_into_dirtied_buffers() {
        let mut engine = Engine::new(48_000.0, 512);
        let mut l = vec![1.0_f32; 512];
        let mut r = vec![-1.0_f32; 512];
        engine.process_block(&mut l, &mut r);
        assert!(
            l.iter().chain(r.iter()).all(|&v| v == 0.0),
            "scaffold engine must render silence"
        );
    }

    #[test]
    fn inherits_voice_count_from_shared_dsp() {
        assert_eq!(MAX_VOICES, vxn_dsp::MAX_VOICES);
    }
}
