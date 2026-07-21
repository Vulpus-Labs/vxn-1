//! Host-tempo sync for the LFOs (E004 / 0015).
//!
//! When an LFO's sync is on, its rate knob no longer means free-running Hz —
//! it selects a **musical subdivision** locked to the host tempo. The knob's
//! normalised position picks a subdivision from [`SUBDIVISIONS`] (coarse →
//! fine), and [`synced_hz`] resolves that to an actual Hz from the current BPM.
//!
//! The LFO core stays Hz-driven (ADR 0002 §Consequences): sync is purely a rate
//! computation here, isolated from [`vxn_dsp::LfoCore`].

use crate::model::{ParamId, ParamModel};
use crate::params::{
    GlobalParam, ParamDesc, ParamRef, PatchParam, desc_for_clap_id, global_clap_id, param_ref,
    patch_clap_id,
};

// Re-exports the shared subdivision table (and its index lookup and rate/period
// resolvers) under the `synced_*` names vxn-1's engine/editor use
// (`synced_hz`/`synced_seconds` = core's `subdivision_hz`/`_seconds`). Keeps
// only the per-synth CLAP-id sync helpers below.
pub use vxn_core_utils::sync::{
    DEFAULT_TEMPO_BPM, SUBDIVISIONS, Subdivision, index_from_norm,
    subdivision_hz as synced_hz, subdivision_seconds as synced_seconds,
};

/// Sync partner CLAP id for a rate/time param — returns the matching sync
/// toggle's id when the input is one of the sync-pairable rate/time params
/// (LFO 1 / LFO 2 rate, Delay time). `None` for anything else.
///
/// Mirrors the editor's `locateSyncPartners` so the host's `value_to_text`
/// and the engine's `ParamChanged` broadcast can render subdivisions when
/// sync is on, matching the editor's value popup.
pub fn sync_partner_clap_id(id: usize) -> Option<usize> {
    match param_ref(id)? {
        ParamRef::Patch(layer, PatchParam::LfoRate) => {
            Some(patch_clap_id(layer, PatchParam::LfoSync))
        }
        ParamRef::Global(GlobalParam::Lfo2Rate) => Some(global_clap_id(GlobalParam::Lfo2Sync)),
        ParamRef::Global(GlobalParam::DelayTime) => Some(global_clap_id(GlobalParam::DelaySync)),
        _ => None,
    }
}

/// Inverse of [`sync_partner_clap_id`]: given a sync flag's CLAP id, returns
/// its rate/time partner's id. Used to refresh a synced rate fader's display
/// when its sync toggle flips while the rate value itself hasn't changed.
pub fn rate_partner_clap_id(id: usize) -> Option<usize> {
    match param_ref(id)? {
        ParamRef::Patch(layer, PatchParam::LfoSync) => {
            Some(patch_clap_id(layer, PatchParam::LfoRate))
        }
        ParamRef::Global(GlobalParam::Lfo2Sync) => Some(global_clap_id(GlobalParam::Lfo2Rate)),
        ParamRef::Global(GlobalParam::DelaySync) => Some(global_clap_id(GlobalParam::DelayTime)),
        _ => None,
    }
}

/// Whether `id` is a sync toggle whose rate partner needs a display refresh
/// on flip. Convenience over [`rate_partner_clap_id`] returning a bool.
pub fn is_sync_flag(id: usize) -> bool {
    rate_partner_clap_id(id).is_some()
}

/// Subdivision label corresponding to a rate/time param value, using the
/// fader-position mapping the engine's sync resolution applies (`to_fader`
/// → `index_from_norm`). Caller has already determined sync is on.
pub fn synced_label_for(desc: &ParamDesc, value: f32) -> &'static str {
    let pos = desc.to_fader(value);
    SUBDIVISIONS[index_from_norm(pos)].label
}

/// Sync-aware display string for a CLAP param. When `clap_id` is an LFO/Delay
/// rate/time whose sync partner reads on, returns the matching subdivision
/// label; otherwise the normal unit-formatted display. Shared by the host
/// `value_to_text` path and the editor `ParamChanged` broadcast so both
/// readouts agree.
pub fn sync_aware_display<M: ParamModel + ?Sized>(model: &M, clap_id: usize, value: f32) -> String {
    let Some(desc) = desc_for_clap_id(clap_id) else {
        return String::new();
    };
    if let Some(sync_id) = sync_partner_clap_id(clap_id) {
        if model.get(ParamId::new(sync_id)) >= 0.5 {
            return synced_label_for(desc, value).to_string();
        }
    }
    desc.display(value)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_subdivisions_match_beat_math() {
        // 1/4 cycles once per beat: at 120 BPM that's 2 Hz; at 90, 1.5 Hz.
        let q = SUBDIVISIONS.iter().position(|s| s.label == "1/4").unwrap();
        assert!((synced_hz(120.0, q) - 2.0).abs() < 1e-5);
        assert!((synced_hz(90.0, q) - 1.5).abs() < 1e-5);
        // 1/8 is twice as fast.
        let e = SUBDIVISIONS.iter().position(|s| s.label == "1/8").unwrap();
        assert!((synced_hz(90.0, e) - 3.0).abs() < 1e-5);
    }

    #[test]
    fn dotted_and_triplet_scale_the_straight_rate() {
        let q = SUBDIVISIONS.iter().position(|s| s.label == "1/4").unwrap();
        let qd = SUBDIVISIONS.iter().position(|s| s.label == "1/4.").unwrap();
        let qt = SUBDIVISIONS.iter().position(|s| s.label == "1/4T").unwrap();
        for bpm in [90.0_f32, 140.0] {
            let straight = synced_hz(bpm, q);
            // Dotted is 1.5× longer → 2/3 the rate; triplet 2/3 longer → 1.5×.
            assert!(
                (synced_hz(bpm, qd) - straight / 1.5).abs() < 1e-4,
                "dotted {bpm}"
            );
            assert!(
                (synced_hz(bpm, qt) - straight * 1.5).abs() < 1e-4,
                "triplet {bpm}"
            );
        }
    }

    #[test]
    fn synced_seconds_is_the_period_of_synced_hz() {
        // 1/4 at 120 BPM = one beat = 0.5 s; at 60 BPM = 1.0 s.
        let q = SUBDIVISIONS.iter().position(|s| s.label == "1/4").unwrap();
        assert!((synced_seconds(120.0, q) - 0.5).abs() < 1e-6);
        assert!((synced_seconds(60.0, q) - 1.0).abs() < 1e-6);
        // It's exactly 1/synced_hz for any subdivision/tempo.
        for bpm in [60.0_f32, 128.0, 174.0] {
            for idx in 0..SUBDIVISIONS.len() {
                assert!((synced_seconds(bpm, idx) - 1.0 / synced_hz(bpm, idx)).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn sync_partner_maps_both_layers_and_globals() {
        use crate::domain::Layer;
        let up_rate = patch_clap_id(Layer::Upper, PatchParam::LfoRate);
        let up_sync = patch_clap_id(Layer::Upper, PatchParam::LfoSync);
        let lo_rate = patch_clap_id(Layer::Lower, PatchParam::LfoRate);
        let lo_sync = patch_clap_id(Layer::Lower, PatchParam::LfoSync);
        assert_eq!(sync_partner_clap_id(up_rate), Some(up_sync));
        assert_eq!(sync_partner_clap_id(lo_rate), Some(lo_sync));
        assert_eq!(rate_partner_clap_id(up_sync), Some(up_rate));
        assert_eq!(rate_partner_clap_id(lo_sync), Some(lo_rate));
        let lfo2_r = global_clap_id(GlobalParam::Lfo2Rate);
        let lfo2_s = global_clap_id(GlobalParam::Lfo2Sync);
        let dly_t = global_clap_id(GlobalParam::DelayTime);
        let dly_s = global_clap_id(GlobalParam::DelaySync);
        assert_eq!(sync_partner_clap_id(lfo2_r), Some(lfo2_s));
        assert_eq!(sync_partner_clap_id(dly_t), Some(dly_s));
        assert_eq!(rate_partner_clap_id(lfo2_s), Some(lfo2_r));
        assert_eq!(rate_partner_clap_id(dly_s), Some(dly_t));
        // Non-sync-pairable params return None.
        assert_eq!(
            sync_partner_clap_id(patch_clap_id(Layer::Upper, PatchParam::Cutoff)),
            None
        );
        assert_eq!(
            rate_partner_clap_id(patch_clap_id(Layer::Upper, PatchParam::Cutoff)),
            None
        );
        assert!(is_sync_flag(up_sync));
        assert!(!is_sync_flag(up_rate));
    }

    #[test]
    fn synced_label_for_picks_via_fader_position() {
        // Mirrors the engine's `lfo_rate_from` path: `to_fader` → `index_from_norm`
        // → SUBDIVISIONS[idx].label. Anchors at the table ends.
        let rate = PatchParam::LfoRate.desc();
        let lo = synced_label_for(rate, rate.min);
        let hi = synced_label_for(rate, rate.max);
        assert_eq!(lo, SUBDIVISIONS[0].label);
        assert_eq!(hi, SUBDIVISIONS[SUBDIVISIONS.len() - 1].label);
    }

    #[test]
    fn norm_maps_across_the_whole_table() {
        assert_eq!(index_from_norm(0.0), 0);
        assert_eq!(index_from_norm(1.0), SUBDIVISIONS.len() - 1);
        // Clamped, never out of bounds.
        assert_eq!(index_from_norm(-1.0), 0);
        assert_eq!(index_from_norm(2.0), SUBDIVISIONS.len() - 1);
    }
}
