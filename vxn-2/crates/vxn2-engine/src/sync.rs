//! Sync-aware parameter display (moved from `vxn2-clap` in E006 / 0066).
//!
//! Rate / time params with a BPM-sync partner display as host-tempo
//! subdivision labels (`1/8`, `1/4 T`, …) while the partner is on, and as
//! plain Hz / ms otherwise. This is domain logic, not CLAP plumbing — both
//! the view pump and the host's `value_to_text` route through it, and any
//! future non-CLAP frontend needs the same mapping.

use crate::params::id_of;
use crate::shared::SharedParams;
use vxn2_dsp::lfo::{SUBDIVISIONS, index_from_norm};

/// Sync-toggle CLAP id for a rate/time param — returns the matching
/// sync flag's id when `id` is `lfo1-rate`, `delay-time`, or
/// `lfo2-rate`. Reverb / master don't sync.
pub fn sync_partner_clap_id(id: usize) -> Option<usize> {
    for (rate, sync) in sync_pairs() {
        if id == *rate {
            return Some(*sync);
        }
    }
    None
}

/// Inverse of [`sync_partner_clap_id`]: given a sync flag's CLAP id,
/// returns its rate / time partner. Used to refresh a synced rate
/// fader's display when its sync toggle flips while the rate value
/// itself hasn't changed.
pub fn rate_partner_clap_id(id: usize) -> Option<usize> {
    for (rate, sync) in sync_pairs() {
        if id == *sync {
            return Some(*rate);
        }
    }
    None
}

/// Sync-aware display string for a CLAP param. When `id` is a
/// rate / time param whose sync partner is on, returns the matching
/// subdivision label; otherwise the descriptor's normal unit-formatted
/// display.
pub fn sync_aware_display(params: &SharedParams, id: usize, value: f32) -> String {
    let Some(desc) = crate::params::desc(id) else {
        return String::new();
    };
    if let Some(sync_id) = sync_partner_clap_id(id) {
        if params.get(sync_id) >= 0.5 {
            return SUBDIVISIONS[index_from_norm(desc.to_normalised(value))]
                .label
                .to_string();
        }
    }
    desc.display(value)
}

/// Resolve the rate/sync CLAP-id pairs once and cache. `id_of` is a
/// linear scan over `PARAMS`; doing it per-tick × per-id would be O(N²).
/// Each entry is `(rate_or_time_id, sync_flag_id)`.
pub fn sync_pairs() -> &'static [(usize, usize)] {
    use std::sync::OnceLock;
    static PAIRS: OnceLock<Vec<(usize, usize)>> = OnceLock::new();
    PAIRS
        .get_or_init(|| {
            vec![
                (
                    id_of("lfo1-rate").expect("lfo1-rate"),
                    id_of("lfo1-sync").expect("lfo1-sync"),
                ),
                (
                    id_of("delay-time").expect("delay-time"),
                    id_of("delay-sync").expect("delay-sync"),
                ),
                (
                    id_of("lfo2-rate").expect("lfo2-rate"),
                    id_of("lfo2-sync").expect("lfo2-sync"),
                ),
            ]
        })
        .as_slice()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every pair in `sync_pairs` flips its rate display between the
    /// descriptor's unit format and a subdivision label as the partner
    /// toggles (ticket 0066 — the host's automation lane and the editor
    /// must agree).
    #[test]
    fn each_sync_pair_switches_rate_display() {
        let params = SharedParams::new();
        for &(rate, sync) in sync_pairs() {
            let desc = crate::params::desc(rate).unwrap();
            let value = desc.default;
            params.set(sync, 0.0);
            let plain = sync_aware_display(&params, rate, value);
            assert!(
                plain.contains("Hz") || plain.contains("ms"),
                "{}: sync off must show units, got {plain:?}",
                desc.id
            );
            params.set(sync, 1.0);
            let synced = sync_aware_display(&params, rate, value);
            assert!(
                synced.contains('/'),
                "{}: sync on must show a subdivision, got {synced:?}",
                desc.id
            );
        }
    }

    /// Non-sync-pairable ids fall through to the descriptor display no
    /// matter what the sync flags say.
    #[test]
    fn unpaired_ids_use_descriptor_display() {
        let params = SharedParams::new();
        for &(_, sync) in sync_pairs() {
            params.set(sync, 1.0);
        }
        let vol = id_of("master-volume").unwrap();
        let s = sync_aware_display(&params, vol, -6.0);
        assert!(s.contains("dB"), "got {s:?}");
    }
}
