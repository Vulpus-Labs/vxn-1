//! Host-tempo sync for the two LFOs and the delay (0267).
//!
//! Ported from VXN1's `vxn_app::sync`, re-keyed to VXN1b's two-layer CLAP map.
//! When a sync toggle is on, its partnered rate/time control no longer means
//! free-running Hz / seconds — its **fader position** selects a musical
//! subdivision locked to the host tempo, resolved against the current BPM.
//!
//! The DSP kernels stay Hz-driven: sync is purely a rate computation, applied
//! where the block context is built (LFO 1 / LFO 2) or where `FxParams` is read
//! (delay). The subdivision table itself is shared — [`vxn_core_utils::sync`].

use crate::params::{ClapRef, ParamId, Params, clap_id_of, clap_ref, desc_for_clap_id};
use vxn_core_app::ParamDesc;

pub use vxn_core_utils::sync::{
    DEFAULT_TEMPO_BPM, SUBDIVISIONS, Subdivision, index_from_norm,
    subdivision_hz as synced_hz, subdivision_seconds as synced_seconds,
};

/// Sync-toggle CLAP id partnered with a rate/time param's CLAP id. `None` for
/// anything that isn't sync-pairable. Mirrors the faceplate's
/// `locateSyncPartners`, so the host's `value_to_text` and the editor's popup
/// agree on when a subdivision label is shown.
pub fn sync_partner_clap_id(clap_id: usize) -> Option<usize> {
    let partner = match clap_ref(clap_id)? {
        ClapRef::Patch(layer, ParamId::Lfo1Rate) => clap_id_of(layer, ParamId::Lfo1Sync),
        ClapRef::Patch(layer, ParamId::Lfo2Rate) => clap_id_of(layer, ParamId::Lfo2Sync),
        ClapRef::Global(ParamId::DelayTime) => clap_id_of(crate::params::Layer::L1, ParamId::DelaySync),
        _ => return None,
    };
    Some(partner)
}

/// Inverse of [`sync_partner_clap_id`]: the rate/time partner of a sync flag.
/// Used to refresh a synced fader's readout when the toggle flips but the
/// underlying rate value hasn't changed.
pub fn rate_partner_clap_id(clap_id: usize) -> Option<usize> {
    let partner = match clap_ref(clap_id)? {
        ClapRef::Patch(layer, ParamId::Lfo1Sync) => clap_id_of(layer, ParamId::Lfo1Rate),
        ClapRef::Patch(layer, ParamId::Lfo2Sync) => clap_id_of(layer, ParamId::Lfo2Rate),
        ClapRef::Global(ParamId::DelaySync) => clap_id_of(crate::params::Layer::L1, ParamId::DelayTime),
        _ => return None,
    };
    Some(partner)
}


/// Subdivision index a rate/time value selects, using the same fader-position
/// mapping the engine's rate resolution applies.
#[inline]
pub fn subdivision_index(desc: &ParamDesc, value: f32) -> usize {
    index_from_norm(desc.to_fader(value))
}

/// Subdivision label for a rate/time value. Caller has already established that
/// sync is on.
#[inline]
pub fn synced_label_for(desc: &ParamDesc, value: f32) -> &'static str {
    SUBDIVISIONS[subdivision_index(desc, value)].label
}

/// Resolve an LFO rate in Hz for this block. Sync off: the knob is literal Hz.
/// Sync on: its fader position picks a subdivision at `tempo_bpm`. Subdivisions
/// spread linearly over `to_fader` (not the tapered Hz value) so the spacing is
/// even with no midpoint skew.
#[inline]
pub fn lfo_rate_hz(p: &Params, rate: ParamId, sync_flag: ParamId, tempo_bpm: f32) -> f32 {
    let value = p.get(rate);
    if p.bool(sync_flag) {
        synced_hz(tempo_bpm, subdivision_index(rate.desc(), value))
    } else {
        value
    }
}

/// Resolve the delay time in seconds. Sync off: literal seconds. Sync on: the
/// fader position picks a subdivision **period** at `tempo_bpm`.
///
/// The synced result is clamped to the delay **line's** capacity, not to the
/// Time knob's 2 s ceiling — the knob's range governs free-run only, and a
/// subdivision period legitimately runs past it (`1/1` is 4 s at 60 BPM). Past
/// the line's capacity there is nothing left to do but clamp, so the slowest
/// entries still flatten out at very low tempos.
#[inline]
pub fn delay_time_seconds(p: &Params, tempo_bpm: f32) -> f32 {
    let value = p.get(ParamId::DelayTime);
    if p.bool(ParamId::DelaySync) {
        let index = subdivision_index(ParamId::DelayTime.desc(), value);
        synced_seconds(tempo_bpm, index).clamp(0.0, crate::fx::DELAY_MAX_SECONDS)
    } else {
        value
    }
}

/// Sync-aware display string for a CLAP param: the subdivision label when the
/// param is a rate/time whose sync partner reads on, otherwise the normal
/// unit-formatted display. Shared by the host `value_to_text` path and the
/// editor's `ParamChanged` broadcast so the two readouts agree.
pub fn sync_aware_display(store: &crate::shared::SharedParams, clap_id: usize, value: f32) -> String {
    let Some(desc) = desc_for_clap_id(clap_id) else {
        return String::new();
    };
    if let Some(sync_id) = sync_partner_clap_id(clap_id) {
        if store.get(sync_id) >= 0.5 {
            return synced_label_for(desc, value).to_string();
        }
    }
    desc.display(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::Layer;

    #[test]
    fn partners_round_trip_for_every_sync_pair() {
        for (rate, flag) in [
            (ParamId::Lfo1Rate, ParamId::Lfo1Sync),
            (ParamId::Lfo2Rate, ParamId::Lfo2Sync),
        ] {
            for layer in [Layer::L1, Layer::L2] {
                let r = clap_id_of(layer, rate);
                let s = clap_id_of(layer, flag);
                assert_eq!(sync_partner_clap_id(r), Some(s));
                assert_eq!(rate_partner_clap_id(s), Some(r));
            }
        }
        let dt = clap_id_of(Layer::L1, ParamId::DelayTime);
        let ds = clap_id_of(Layer::L1, ParamId::DelaySync);
        assert_eq!(sync_partner_clap_id(dt), Some(ds));
        assert_eq!(rate_partner_clap_id(ds), Some(dt));
    }

    #[test]
    fn non_pairable_params_have_no_partner() {
        let cutoff = clap_id_of(Layer::L1, ParamId::Cutoff);
        assert_eq!(sync_partner_clap_id(cutoff), None);
        assert_eq!(rate_partner_clap_id(cutoff), None);
    }

    #[test]
    fn synced_lfo_rate_tracks_tempo_not_the_knob() {
        let mut p = Params::default();
        // Fader position for the "1/4" entry — one beat per cycle.
        let q = SUBDIVISIONS.iter().position(|s| s.label == "1/4").unwrap();
        let pos = q as f32 / (SUBDIVISIONS.len() - 1) as f32;
        let desc = ParamId::Lfo1Rate.desc();
        p.set(ParamId::Lfo1Rate, desc.from_fader(pos));
        p.set(ParamId::Lfo1Sync, 0.0);
        let free = lfo_rate_hz(&p, ParamId::Lfo1Rate, ParamId::Lfo1Sync, 120.0);
        assert!((free - p.get(ParamId::Lfo1Rate)).abs() < 1e-6, "sync off passes the knob through");
        p.set(ParamId::Lfo1Sync, 1.0);
        assert!((lfo_rate_hz(&p, ParamId::Lfo1Rate, ParamId::Lfo1Sync, 120.0) - 2.0).abs() < 1e-4);
        assert!((lfo_rate_hz(&p, ParamId::Lfo1Rate, ParamId::Lfo1Sync, 60.0) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn synced_delay_time_is_the_subdivision_period_clamped_to_range() {
        let mut p = Params::default();
        let desc = ParamId::DelayTime.desc();
        let q = SUBDIVISIONS.iter().position(|s| s.label == "1/4").unwrap();
        p.set(ParamId::DelayTime, desc.from_fader(q as f32 / (SUBDIVISIONS.len() - 1) as f32));
        p.set(ParamId::DelaySync, 1.0);
        assert!((delay_time_seconds(&p, 120.0) - 0.5).abs() < 1e-4);
        // 1/1 is 4 beats. It runs past the knob's 2 s ceiling below 120 BPM, and
        // the line — not the knob — is what may cut it short.
        let one = SUBDIVISIONS.iter().position(|s| s.label == "1/1").unwrap();
        p.set(ParamId::DelayTime, desc.from_fader(one as f32 / (SUBDIVISIONS.len() - 1) as f32));
        assert!(desc.max < 4.0, "the knob ceiling must NOT be what bounds a synced time");
        assert!((delay_time_seconds(&p, 60.0) - 4.0).abs() < 1e-4, "1/1 at 60 BPM is a full 4 s");
        // Only past the line's capacity does it flatten.
        assert!((delay_time_seconds(&p, 30.0) - crate::fx::DELAY_MAX_SECONDS).abs() < 1e-6);
    }
}
