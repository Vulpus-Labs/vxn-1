---
id: "0162"
product: monorepo
title: Prune redundant tests and dedupe duplicated test literals
priority: medium
created: 2026-07-01
epic: E031
---

## Summary

Delete tests whose property is strictly subsumed by a stronger sibling,
and dedupe literal data that is copy-pasted across two tests where a
silent drift between the copies would defeat the test's purpose. Each
deletion must cite the named test that still covers the property.

Line numbers are as-reviewed on 2026-07-01; re-grep by test name.

## Acceptance criteria

Delete subsumed tests (subset ⊂ superset):

- [ ] `vxn-engine/src/factory.rs` `bank_is_non_empty` (~96) ⊂
      `covers_multiple_categories` (≥3 categories implies non-empty).
- [ ] `vxn-engine/src/voice.rs` `keytrack_off_ignores_note` (~2220) ⊂
      `cutoff_keytrack_scales_linearly_with_amt`'s amt=0 case (~2213).
- [ ] `vxn-engine/src/shared.rs` `state_roundtrip_through_store` (~441) ⊂
      `codec_matches_legacy_plugin_state` (~461, does the full round-trip
      plus byte parity).
- [ ] `vxn-engine/src/preset.rs` engine↔engine round-trips (~340/374) ⊂
      the byte-parity trio (`app_writer_matches_engine_byte_for_byte` +
      `app_write_parses_on_engine`). Confirm before deleting.
- [ ] `vxn2-engine/src/engine.rs` `filter_on_self_osc_is_bounded` (~3529)
      ⊂ `filter_integration.rs` `self_oscillation_bounded_at_every_factor`
      (~124, sweeps 4 cutoffs × 4 factors through the SharedParams path).
      If direct `params_mut().filter` mutation is the unique value, narrow
      to that instead of deleting.
- [ ] `vxn2-dsp/src/sine.rs` `scalar_fast_sine_landmarks` (~129) ⊂
      `fast_sine_accuracy` 100k-point sweep. Keep `scalar_lookup_sine_
      landmarks` (lookup has no sweep); share the landmark array.
- [ ] `vxn2-dsp/src/op.rs` `feedback_alters_output_vs_no_feedback` (~342)
      ⊂ `feedback_fractional_value_distinct_from_neighbours` (~367).
- [ ] `vxn-wasm/src/codec.rs` `round_trips_every_kind` (~642) ⊂
      `encode_matches_golden_bytes` ∧ `decode_matches_golden_bytes`.
- [ ] `vxn-ui-web/src/lib.rs` `batch_chunks_single_under_cap` (~702) and
      `batch_chunks_dedup_applies_before_chunking` (~736) ⊂
      `dedup_keeps_latest_param_per_id` (~677) + `batch_chunks_splits_
      above_cap` (~714).
- [ ] `vxn-ui-web/src/lib.rs` `web_page_splices_clean_and_wires_boot`
      (~1826) params-present check ⊂ `web_page_params_are_byte_identical_
      to_native` (~1860).
- [ ] `vxn2-dsp/src/delay.rs` `dc_blocker_kills_dc_in_feedback_loop` (~453)
      — loose in-loop bound (out<1.5) duplicates `dc_blocker_actually_blocks_
      dc` (~522, tighter unit bound). Keep only if in-loop integration is
      the deliberate point; otherwise delete.
- [ ] `vxn2-dsp/src/tables.rs` `fb_scale_monotone` (~71) ⊂
      `fb_scale_integer_inputs_match_table` + `fb_scale_interpolates_
      between_steps` given a monotone table. Low priority; cheap to keep.

Merge overlapping pairs (fold into one, non-overlapping params):

- [ ] `vxn2-dsp/src/filter.rs` (~439/484) — `high_cutoff_resonance_decays_
      while_low_cutoff_sustains` and `state_decays_below_floor_then_self_
      osc_never_does` measure the same excite-then-silence decay. Merge, or
      split so one clearly tests the cutoff cap and the other the resonance
      threshold with disjoint parameters.
- [ ] `vxn-engine/src/voice.rs` (~2351/2368/2380) — fold the `varied` and
      `decorrelated` `VoiceTrim` assertions into one properties test
      (bounded + deterministic-per-seed + streams-differ).

Dedupe duplicated literals (prevent silent drift):

- [ ] `vxn2-clap` state round-trip edit list is duplicated verbatim between
      `tests/smoke.rs` (~376) and `src/lib.rs` `plugin_state_save_load_
      round_trips_every_param` (~1041). Extract one
      `const EDITS: &[(&str, f64)]` shared by both (different ABI layers,
      both kept). Test-support location coordinates with 0167.

- [ ] `cargo test` green; every deletion's covering test named in the
      commit message.

## Notes

Confirmed NON-redundant, do not touch (flagged as suspicious but distinct):
`solo_note_off_falls_back...` engine vs alloc (engine exercises
`Engine::note_off`); `hadamard_is_unitary` vs `..._involution` (norm vs
H²=I); `carrier_counts_match_yamaha_chart` vs `ping_test_carrier_audibility`
(popcount vs end-to-end); `interp_1x_is_identity` vs `interp_then_decimate_
roundtrips`. Cogency repairs are 0161; apparatus extraction is 0164–0167.
