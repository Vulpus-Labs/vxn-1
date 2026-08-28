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

/// The three sync pairs, as `(rate/time, sync toggle)` [`ParamId`]s.
///
/// [`sync_partner_clap_id`] and [`rate_partner_clap_id`] are exact inverses,
/// and before 0319 each spelled the mapping out as its own three-arm match —
/// mirror images kept in step by hand, so adding a fourth syncable control
/// meant remembering to edit both. Searching one table in either direction
/// makes them inverses by construction.
///
/// The delay pair is patch-global (only layer 1 carries it); the two LFO pairs
/// are per-layer. `layer_of` on the resolved [`ClapRef`] is what carries that
/// distinction, so it is not encoded here.
const SYNC_PAIRS: [(ParamId, ParamId); 3] = [
    (ParamId::Lfo1Rate, ParamId::Lfo1Sync),
    (ParamId::Lfo2Rate, ParamId::Lfo2Sync),
    (ParamId::DelayTime, ParamId::DelaySync),
];

/// Resolve one half of a sync pair to the CLAP id of its partner. `pick` reads
/// the partner out of a matched row, so the two public directions differ by
/// that one closure and nothing else.
fn partner_clap_id(clap_id: usize, pick: fn(&(ParamId, ParamId)) -> (ParamId, ParamId)) -> Option<usize> {
    let (layer, param) = match clap_ref(clap_id)? {
        ClapRef::Patch(layer, param) => (layer, param),
        // A global sync param belongs to the one delay, which layer 1 owns.
        ClapRef::Global(param) => (crate::params::Layer::L1, param),
    };
    let row = SYNC_PAIRS.iter().find(|pair| pick(pair).0 == param)?;
    Some(clap_id_of(layer, pick(row).1))
}

/// Sync-toggle CLAP id partnered with a rate/time param's CLAP id. `None` for
/// anything that isn't sync-pairable. Mirrors the faceplate's
/// `locateSyncPartners`, so the host's `value_to_text` and the editor's popup
/// agree on when a subdivision label is shown.
pub fn sync_partner_clap_id(clap_id: usize) -> Option<usize> {
    partner_clap_id(clap_id, |&(rate, sync)| (rate, sync))
}

/// Inverse of [`sync_partner_clap_id`]: the rate/time partner of a sync flag.
/// Used to refresh a synced fader's readout when the toggle flips but the
/// underlying rate value hasn't changed.
pub fn rate_partner_clap_id(clap_id: usize) -> Option<usize> {
    partner_clap_id(clap_id, |&(rate, sync)| (sync, rate))
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

    /// 0319 replaced two hand-mirrored three-arm matches with one table read in
    /// both directions. Neither function had a test, and "exact inverses" is
    /// exactly the property that was being maintained by hand — so pin it, over
    /// every CLAP id rather than the three the matches happened to name.
    #[test]
    fn sync_and_rate_partners_are_exact_inverses() {
        use crate::params::{Layer, TOTAL_PARAMS};

        let mut paired = 0usize;
        for id in 0..TOTAL_PARAMS {
            match sync_partner_clap_id(id) {
                Some(sync_id) => {
                    paired += 1;
                    assert_eq!(
                        rate_partner_clap_id(sync_id),
                        Some(id),
                        "clap id {id} → sync {sync_id} did not come back"
                    );
                }
                // Not a rate: then it must not be reachable as one from the
                // other side either, unless it is itself a sync toggle.
                None => {
                    if let Some(rate_id) = rate_partner_clap_id(id) {
                        assert_eq!(rate_partner_clap_id(id), Some(rate_id));
                        assert_eq!(sync_partner_clap_id(rate_id), Some(id));
                    }
                }
            }
        }
        // Two LFOs per layer plus the one patch-global delay.
        assert_eq!(paired, 2 * 2 + 1, "expected five sync pairs across both layers");

        // Spot-check that the pairs are the ones intended, not merely
        // self-consistent.
        for layer in [Layer::L1, Layer::L2] {
            assert_eq!(
                sync_partner_clap_id(clap_id_of(layer, ParamId::Lfo1Rate)),
                Some(clap_id_of(layer, ParamId::Lfo1Sync))
            );
            assert_eq!(
                sync_partner_clap_id(clap_id_of(layer, ParamId::Lfo2Rate)),
                Some(clap_id_of(layer, ParamId::Lfo2Sync))
            );
        }
        assert_eq!(
            sync_partner_clap_id(clap_id_of(Layer::L1, ParamId::DelayTime)),
            Some(clap_id_of(Layer::L1, ParamId::DelaySync))
        );
        // A param with no sync partner stays unpaired in both directions.
        let cutoff = clap_id_of(Layer::L1, ParamId::Cutoff);
        assert_eq!(sync_partner_clap_id(cutoff), None);
        assert_eq!(rate_partner_clap_id(cutoff), None);
    }
}
