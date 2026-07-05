//! The engine roster (ADR 0001 §6; four voice families, ADR 0005): `Kick/Tone`
//! (Driven poly), `Metal` (modal + XOR metallic), `Noise` (filtered burst + clap),
//! `Struck` (BridgedT resonator) — all plugging into the same `TrackEngine` slot,
//! per-block dispatch, and SoA block.

pub mod kick_tone;
pub mod metal;
pub mod noise;
pub mod struck;

pub use kick_tone::{KickTone, KickTonePatch};
pub use metal::{Metal, MetalPatch};
pub use noise::{Noise, NoisePatch};
pub use struck::{Struck, StruckPatch};

use crate::track_engine::{EngineKind, TrackEngine};

/// Build a fresh engine of the given kind with its default patch — the factory
/// the main thread uses to construct an engine before handing it to the audio
/// thread over the swap channel.
pub fn make(kind: EngineKind, sample_rate: f32) -> Box<dyn TrackEngine> {
    match kind {
        EngineKind::KickTone => Box::new(KickTone::with_default_patch(sample_rate)),
        EngineKind::Metal => Box::new(Metal::with_default_patch(sample_rate)),
        EngineKind::Noise => Box::new(Noise::with_default_patch(sample_rate)),
        EngineKind::Struck => Box::new(Struck::with_default_patch(sample_rate)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_trig(e: &mut dyn crate::track_engine::TrackEngine) -> Vec<f32> {
        let mut buf = vec![0.0_f32; 4_800];
        e.on_trig(50.0, 1.0);
        e.render(&mut buf);
        buf
    }

    /// The **flat** engines' patch (Metal, Noise) survives a rebuild — the through-swap
    /// path (0179): edit the patch via `set_macro` (these still cook immediately),
    /// serialize, rebuild a fresh default engine, deserialize, and the rebuilt engine
    /// renders the edited voice, not the default.
    /// Every family is on the flavour runtime (Driven 0180, Noise 0182, Metal 0183):
    /// each serialises its deep patch as a flavour. Edit the flavour base, serialize,
    /// rebuild a fresh default engine, deserialize → the rebuilt engine renders the
    /// edited voice. Macro **values** are host state, not in the patch — so this varies
    /// the flavour, not `set_macro`.
    #[test]
    fn flavour_engine_round_trips_through_rebuild() {
        let sr = 48_000.0;
        let mut kick_flav = kick_tone::driven_default_flavour();
        kick_flav.base[kick_tone::P_AMP_DECAY] = 0.9;
        kick_flav.base[kick_tone::P_PITCH_DEPTH] = 6.0;
        let mut noise_flav = noise::noise_default_flavour();
        noise_flav.base[noise::P_BAND_FREQ] = 4000.0;
        noise_flav.base[noise::P_SNAP] = 0.7;
        noise_flav.base[noise::P_TAP_COUNT] = 3.0;
        let mut metal_flav = metal::metal_default_flavour();
        metal_flav.base[metal::P_XOR_MIX] = 0.8;
        metal_flav.base[metal::P_SHIMMER] = 0.5;
        metal_flav.base[metal::P_BASE_HZ] = 900.0;
        let mut struck_flav = struck::struck_default_flavour();
        struck_flav.base[struck::P_DROOP_DEPTH] = 18.0;
        struck_flav.base[struck::P_INHARM] = 0.7;
        struck_flav.base[struck::P_EXC_SHAPE] = 3.0;

        let cases = [
            (EngineKind::KickTone, kick_flav),
            (EngineKind::Noise, noise_flav),
            (EngineKind::Metal, metal_flav),
            (EngineKind::Struck, struck_flav),
        ];
        for (kind, flav) in cases {
            let mut src = make(kind, sr);
            src.apply_flavour(flav);
            let want = render_trig(&mut *src);

            let mut bytes = Vec::new();
            src.serialize_patch(&mut bytes);

            let mut dst = make(kind, sr);
            dst.deserialize_patch(&bytes).expect("valid flavour patch");
            let got = render_trig(&mut *dst);
            assert_eq!(want, got, "{kind:?} rebuilt flavour does not match edited source");

            let def = render_trig(&mut *make(kind, sr));
            assert_ne!(def, got, "{kind:?} edited flavour indistinguishable from default");
        }
    }

    /// Deep-patch tolerance (every engine is now a flavour): empty (v1 blob) keeps the
    /// default; an unknown version tag keeps the default without erroring; a truncated
    /// patch within a known version+shape is rejected. Truncation uses `[1, P, 0x00]` —
    /// version 1, the family's `n_params`, then a truncated base.
    #[test]
    fn patch_deserialize_tolerances() {
        let sr = 48_000.0;
        for kind in [EngineKind::KickTone, EngineKind::Metal, EngineKind::Noise, EngineKind::Struck] {
            let mut e = make(kind, sr);
            assert!(e.deserialize_patch(&[]).is_ok(), "{kind:?} empty patch");
            assert!(e.deserialize_patch(&[0xFF]).is_ok(), "{kind:?} unknown version tolerated");
        }
        for (kind, p) in [
            (EngineKind::KickTone, kick_tone::DRIVEN_P as u8),
            (EngineKind::Noise, noise::NOISE_P as u8),
            (EngineKind::Metal, metal::METAL_P as u8),
            (EngineKind::Struck, struck::STRUCK_P as u8),
        ] {
            let mut e = make(kind, sr);
            assert!(e.deserialize_patch(&[1, p, 0x00]).is_err(), "{kind:?} truncated flavour rejected");
        }
    }
}
