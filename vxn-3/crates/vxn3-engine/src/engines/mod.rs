//! The engine roster (ADR 0001 §6): `Kick/Tone` (poly), `Metal` (modal
//! resonator), `Noise` (poly perc) — all plugging into the same `TrackEngine`
//! slot, per-block dispatch, and SoA block.

pub mod kick_tone;
pub mod metal;
pub mod noise;

pub use kick_tone::{KickTone, KickTonePatch};
pub use metal::{Metal, MetalPatch};
pub use noise::{Noise, NoisePatch};

use crate::track_engine::{EngineKind, TrackEngine};

/// Build a fresh engine of the given kind with its default patch — the factory
/// the main thread uses to construct an engine before handing it to the audio
/// thread over the swap channel.
pub fn make(kind: EngineKind, sample_rate: f32) -> Box<dyn TrackEngine> {
    match kind {
        EngineKind::KickTone => Box::new(KickTone::with_default_patch(sample_rate)),
        EngineKind::Metal => Box::new(Metal::with_default_patch(sample_rate)),
        EngineKind::Noise => Box::new(Noise::with_default_patch(sample_rate)),
    }
}
