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
    #[test]
    fn flat_engine_patch_round_trips_through_rebuild() {
        // Metal is the last engine on the flat 0179 patch (Kick=0180, Noise=0182 now
        // on the flavour runtime). Edit its patch via `set_macro` (cooks immediately),
        // serialize, rebuild, deserialize → the rebuilt engine renders the edited voice.
        let sr = 48_000.0;
        let kind = EngineKind::Metal;
        let mut src = make(kind, sr);
        src.set_macro(0, 0.9);
        src.set_macro(1, 0.15);
        src.set_macro(2, 0.8);
        let want = render_trig(&mut *src);

        let mut bytes = Vec::new();
        src.serialize_patch(&mut bytes);
        assert!(!bytes.is_empty(), "{kind:?} serialized an empty patch");

        let mut dst = make(kind, sr);
        dst.deserialize_patch(&bytes).expect("valid patch");
        let got = render_trig(&mut *dst);
        assert_eq!(want, got, "{kind:?} rebuilt patch does not match edited source");

        let def = render_trig(&mut *make(kind, sr));
        assert_ne!(def, got, "{kind:?} edited patch indistinguishable from default");
    }

    /// The **flavour** families (Driven 0180, Noise 0182) serialise their deep patch as
    /// a flavour: edit the flavour base, serialize, rebuild a fresh default engine,
    /// deserialize, and the rebuilt engine renders the edited voice. Macro **values**
    /// are host state, not in the patch — so this varies the flavour, not `set_macro`.
    #[test]
    fn flavour_engine_round_trips_through_rebuild() {
        let sr = 48_000.0;
        // (kind, an edited flavour) per flavour family.
        let mut kick_flav = kick_tone::driven_default_flavour();
        kick_flav.base[kick_tone::P_AMP_DECAY] = 0.9;
        kick_flav.base[kick_tone::P_PITCH_DEPTH] = 6.0;
        let mut noise_flav = noise::noise_default_flavour();
        noise_flav.base[noise::P_BAND_FREQ] = 4000.0;
        noise_flav.base[noise::P_SNAP] = 0.7;
        noise_flav.base[noise::P_TAP_COUNT] = 3.0;

        for (kind, flav) in [(EngineKind::KickTone, kick_flav), (EngineKind::Noise, noise_flav)] {
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

    /// Backward / forward tolerance across both patch shapes: empty (v1 blob) keeps the
    /// default; an unknown version tag keeps the default without erroring; a truncated
    /// patch within a known version is rejected.
    #[test]
    fn patch_deserialize_tolerances() {
        let sr = 48_000.0;
        for kind in [EngineKind::KickTone, EngineKind::Metal, EngineKind::Noise] {
            let mut e = make(kind, sr);
            assert!(e.deserialize_patch(&[]).is_ok(), "{kind:?} empty patch");
            assert!(e.deserialize_patch(&[0xFF]).is_ok(), "{kind:?} unknown version tolerated");
        }
        // Truncated within a known version → Err. Metal (flat): version 1 then a
        // truncated f32. Flavour engines: version 1, correct n_params, then a truncated
        // base (n_params = DRIVEN_P 6 / NOISE_P 8).
        let mut metal = make(EngineKind::Metal, sr);
        assert!(metal.deserialize_patch(&[1, 0x00, 0x00]).is_err(), "flat truncated rejected");
        let mut kick = make(EngineKind::KickTone, sr);
        assert!(kick.deserialize_patch(&[1, 6, 0x00]).is_err(), "Driven flavour truncated rejected");
        let mut noise = make(EngineKind::Noise, sr);
        assert!(noise.deserialize_patch(&[1, 8, 0x00]).is_err(), "Noise flavour truncated rejected");
    }
}
