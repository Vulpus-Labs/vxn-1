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

#[cfg(test)]
mod tests {
    use super::*;

    /// The deep patch survives a **rebuild** — the through-swap path (0179): build
    /// an engine, edit its patch to a non-default point, serialize, build a *fresh
    /// default* engine of the same kind (what the swap installs), deserialize, and
    /// the rebuilt engine must render the *edited* voice, not the default.
    #[test]
    fn patch_round_trips_through_rebuild() {
        let sr = 48_000.0;
        for kind in [EngineKind::KickTone, EngineKind::Metal, EngineKind::Noise] {
            let mut src = make(kind, sr);
            // Move every macro-mapped patch field well off its default.
            src.set_macro(0, 0.9);
            src.set_macro(1, 0.15);
            src.set_macro(2, 0.8);

            // Reference audio from the edited engine.
            let mut want = vec![0.0_f32; 4_800];
            src.on_trig(50.0, 1.0);
            src.render(&mut want);

            let mut bytes = Vec::new();
            src.serialize_patch(&mut bytes);
            assert!(!bytes.is_empty(), "{kind:?} serialized an empty patch");

            // Fresh default engine (what the swap hands the audio thread), then apply.
            let mut dst = make(kind, sr);
            dst.deserialize_patch(&bytes).expect("valid patch");
            let mut got = vec![0.0_f32; 4_800];
            dst.on_trig(50.0, 1.0);
            dst.render(&mut got);

            assert_eq!(want, got, "{kind:?} rebuilt patch does not match edited source");

            // And it is genuinely non-default: a default engine renders differently.
            let mut def = make(kind, sr);
            let mut base = vec![0.0_f32; 4_800];
            def.on_trig(50.0, 1.0);
            def.render(&mut base);
            assert_ne!(base, got, "{kind:?} edited patch indistinguishable from default");
        }
    }

    /// Backward / forward tolerance: an empty patch (v1 state blob) keeps the
    /// default; an unknown-version tag keeps the default without erroring; a
    /// truncated patch within the known version is rejected.
    #[test]
    fn patch_deserialize_tolerances() {
        let sr = 48_000.0;
        for kind in [EngineKind::KickTone, EngineKind::Metal, EngineKind::Noise] {
            let mut e = make(kind, sr);
            assert!(e.deserialize_patch(&[]).is_ok(), "{kind:?} empty patch");
            assert!(e.deserialize_patch(&[0xFF]).is_ok(), "{kind:?} unknown version tolerated");
            // Known version tag then a truncated field → Err.
            assert!(e.deserialize_patch(&[1, 0x00, 0x00]).is_err(), "{kind:?} truncated rejected");
        }
    }
}
