//! Audio-thread automation → view diff (0078).
//!
//! `VxnAudioProcessor::process` writes the shared param store directly,
//! bypassing the controller, so the editor never sees those changes through
//! the controller's view-event queue. The main-thread timer diffs the store
//! against a `last_seen` mirror and emits a `ParamChanged` per drift. This is
//! that diff as a pure function — no host boundary — so the NaN-aware change
//! detection and the "sync flip refreshes its rate partner's label" rule are
//! unit-testable.

use crate::events::ViewEvent;
use crate::model::{ParamId, ParamModel};
use crate::sync::{rate_partner_clap_id, sync_aware_display};

/// Generic NaN-aware param diff — the single diff loop both the native CLAP
/// timer ([`diff_params`]) and the web readback pump
/// (`vxn-web-controller::pump_readback`) delegate to.
///
/// Walk `0..n`, compare `get(i)` against `last_seen[i]` (updated in place),
/// and invoke `on_change(i, plain)` for every slot whose value drifted. NaN
/// never equals itself, so seeding `last_seen` all-`NaN` forces a full
/// broadcast on the first call (used to populate the page on editor open).
///
/// When `sync_partner` is set, a changed **sync toggle** additionally fires
/// `on_change` for its rate/time partner — the partner's displayed subdivision
/// label depends on the toggle even though the rate value itself didn't move.
/// This rule is vxn-1-specific, so it stays a caller opt-in and never leaks
/// into `vxn-core-app`. `last_seen` is NOT updated for the forced partner (its
/// value didn't change; only its display did).
pub fn nan_diff(
    n: usize,
    last_seen: &mut [f32],
    get: impl Fn(usize) -> f32,
    sync_partner: bool,
    mut on_change: impl FnMut(usize, f32),
) {
    // Sync flips refresh their rate partner's display label even though the
    // rate's value didn't change. Collect those first, emit after the main pass.
    let mut force_rate_refresh: Vec<usize> = Vec::new();
    for (i, seen) in last_seen.iter_mut().enumerate().take(n) {
        let plain = get(i);
        if plain == *seen {
            continue;
        }
        *seen = plain;
        on_change(i, plain);
        if sync_partner {
            if let Some(rate_id) = rate_partner_clap_id(i) {
                force_rate_refresh.push(rate_id);
            }
        }
    }
    for rate_id in force_rate_refresh {
        on_change(rate_id, get(rate_id));
    }
}

/// Diff `model` against `last_seen` (updating it in place) and return a
/// `ParamChanged` for every param whose value drifted. A changed **sync
/// toggle** additionally re-emits its rate/time partner: the partner's
/// displayed subdivision label depends on the toggle even though the rate
/// value itself didn't move.
///
/// NaN-aware: NaN never equals itself, so seeding `last_seen` all-`NaN`
/// forces a full broadcast on the first call (used to populate the page on
/// editor open).
pub fn diff_params<M: ParamModel + ?Sized>(model: &M, last_seen: &mut [f32]) -> Vec<ViewEvent> {
    let mut events = Vec::new();
    nan_diff(
        model.total(),
        last_seen,
        |i| model.get(ParamId::new(i)),
        true, // vxn-1 sync-toggle → rate-partner label refresh
        |i, plain| events.push(param_changed(model, i, plain)),
    );
    events
}

fn param_changed<M: ParamModel + ?Sized>(model: &M, clap_id: usize, plain: f32) -> ViewEvent {
    let id = ParamId::new(clap_id);
    ViewEvent::ParamChanged {
        id,
        plain,
        norm: model.get_normalized(id),
        display: sync_aware_display(model, clap_id, plain),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{
        GlobalParam, ParamDesc, TOTAL_PARAMS, desc_for_clap_id, global_clap_id,
    };

    /// In-memory store backed by a flat value vector. The diff only reads the
    /// model, so `set*` are unreachable; descriptors come from the real table
    /// (sync-aware display reads it, not `descriptor`).
    struct VecModel(Vec<f32>);

    impl ParamModel for VecModel {
        fn total(&self) -> usize {
            self.0.len()
        }
        fn get(&self, id: ParamId) -> f32 {
            self.0[id.raw()]
        }
        fn set(&self, _: ParamId, _: f32) {
            unreachable!("diff never writes the model")
        }
        fn get_normalized(&self, id: ParamId) -> f32 {
            self.0[id.raw()]
        }
        fn set_normalized(&self, _: ParamId, _: f32) {
            unreachable!("diff never writes the model")
        }
        fn gesture(&self, _: ParamId) -> bool {
            false
        }
        fn set_gesture(&self, _: ParamId, _: bool) {}
        fn descriptor(&self, id: ParamId) -> Option<&'static ParamDesc> {
            desc_for_clap_id(id.raw())
        }
        fn snapshot_bytes(&self) -> Vec<u8> {
            Vec::new()
        }
        fn restore_from_bytes(&self, _: &[u8]) -> Result<(), String> {
            Ok(())
        }
    }

    fn zeros() -> VecModel {
        VecModel(vec![0.0; TOTAL_PARAMS])
    }

    /// (clap id, display) of every `ParamChanged` in order.
    fn changes(events: &[ViewEvent]) -> Vec<(usize, &str)> {
        events
            .iter()
            .map(|ev| match ev {
                ViewEvent::ParamChanged { id, display, .. } => (id.raw(), display.as_str()),
                _ => panic!("non-ParamChanged emitted"),
            })
            .collect()
    }

    #[test]
    fn plain_value_change_emits_one_event() {
        // Param 0 (Upper osc-1 wave) isn't a sync flag, so a lone change to it
        // produces exactly one event and no rate-partner refresh.
        let mut model = zeros();
        let mut last_seen = vec![0.0; TOTAL_PARAMS];
        model.0[0] = 1.0;
        let events = diff_params(&model, &mut last_seen);
        let ids: Vec<usize> = changes(&events).into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, vec![0]);
        // last_seen is updated, so a second diff with no change is silent.
        assert!(diff_params(&model, &mut last_seen).is_empty());
    }

    #[test]
    fn no_change_skips_and_nan_seed_broadcasts() {
        use std::collections::HashSet;
        let model = zeros();
        // Equal current/last_seen → nothing emitted.
        let mut equal = vec![0.0; TOTAL_PARAMS];
        assert!(diff_params(&model, &mut equal).is_empty());
        // NaN seed never equals anything → every param broadcasts (sync flags
        // additionally re-emit their rate partner, so len may exceed the
        // count); afterwards the mirror is quiescent.
        let mut nan_seed = vec![f32::NAN; TOTAL_PARAMS];
        let events = diff_params(&model, &mut nan_seed);
        let ids: HashSet<usize> = changes(&events).into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids.len(), TOTAL_PARAMS, "every param id is broadcast");
        assert!(nan_seed.iter().all(|v| *v == 0.0));
        assert!(diff_params(&model, &mut nan_seed).is_empty());
    }

    #[test]
    fn sync_flip_forces_rate_partner_refresh() {
        let sync_id = global_clap_id(GlobalParam::Lfo2Sync);
        let rate_id = global_clap_id(GlobalParam::Lfo2Rate);
        let rate_desc = desc_for_clap_id(rate_id).unwrap();

        // Sync flips off→on; the rate value itself is unchanged (last_seen
        // already matches it), so only the flip would normally fire.
        let mut model = zeros();
        model.0[sync_id] = 1.0;
        model.0[rate_id] = rate_desc.min;
        let mut last_seen = vec![0.0; TOTAL_PARAMS];
        last_seen[rate_id] = rate_desc.min; // rate value did NOT move

        let events = diff_params(&model, &mut last_seen);
        let got = changes(&events);
        // The flip emits, and its rate partner is force-refreshed even though
        // the rate value didn't change — and renders a subdivision label now
        // that sync is on.
        assert_eq!(got, vec![(sync_id, "On"), (rate_id, "1/1")]);
    }
}
