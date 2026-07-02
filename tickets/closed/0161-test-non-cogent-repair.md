---
id: "0161"
product: monorepo
title: Repair or delete non-cogent tests — pass whether feature works or not
priority: high
created: 2026-07-01
epic: E031
---

## Summary

~12 tests assert setup, a tautology, or a constant rather than the
feature's behaviour — they pass even if the feature is broken. Delete the
ones whose property is covered elsewhere; repair the ones whose intent is
real but whose assertion has no teeth. After this ticket, each repaired
test must *fail* when its feature regresses. Also fix one test whose name
and framing describe pre-shipped behaviour.

Line numbers are as-reviewed on 2026-07-01; re-grep by test name.

## Acceptance criteria

Repair (make the assertion actually distinguish correct from broken):

- [x] `vxn-engine/src/voice.rs` `zero_drift_is_bit_exact_env` (~2409) —
      currently asserts `x * 0.0 == 0.0` and recomputes inline. Read back
      the actual `env2[v]` params after `set_envelopes(..., 0.0)` and
      assert all voices got identical values.
- [x] `vxn2-dsp/src/algo.rs` `resolve_route_clamps` (~511) — currently
      `let _ =` discards the result. Assert the saturated input
      (`resolve_route(0)`) yields the same routing as `resolve_route(1)`,
      mirroring `component_range_guards` (~604).
- [ ] `vxn-wasm/src/host.rs` `key_mode_applied_before_events` (~453) —
      currently only `onset(...).is_some()`. Render note 72 in Split and
      in Whole and assert the buffers differ (routing actually changed).
      **SKIPPED** — premise infeasible for this engine: a single Poly-mode
      note routes identically under Split and Whole, and the level params
      that would differentiate use `Glide::Block` (don't settle within one
      128-sample quantum), so there is no behavioural difference to assert.
      Left as the original crash guard; flagged for the 0169 re-review.
- [x] `vxn-wasm/src/host.rs` `param_step_lands_at_offset` (~370) —
      name promises the change lands at offset 64 but asserts only
      "something audible." Split the render at 64 with old/new value and
      compare; assert pre-64 unaffected.
- [x] `vxn-web-controller/src/lib.rs` (~1414) — `assert_eq!(take_journal(),
      4, ...)` uses a magic length to mean "empty." Decode and assert zero
      ops, or introduce a named `EMPTY_JOURNAL_LEN` const.

Delete (property already covered by a named test — cite it in the commit):

- [x] `vxn-engine/src/lib.rs` `a4_is_440` (~1039) — re-asserts vxn-dsp's
      `note_to_hz(69)` constant; belongs to vxn-dsp, tests no engine logic.
- [x] `vxn2-dsp/src/stack.rs` `fresh_stack_is_idle` (~1099) — tests the
      `Default` derive only.
- [x] `vxn2-dsp/src/envelope.rs` `mod_env_shape_field_persists_after_cook`
      (~557) — asserts a field copy; effect covered by `mod_env_lin_*` /
      `mod_env_exp_*` (~452/481).
- [x] `vxn2-clap/src/lib.rs` `set_tempo_propagates_to_engine` (~1029) —
      getter-after-setter on the engine, no CLAP path; transport decode is
      covered by `smoke.rs` (~488). Delete, or redirect through the
      transport-event dispatch path if that was the intent.

Trim tautological sub-assertions (keep the test, drop the dead part):

- [x] `vxn2-engine/src/engine.rs` `matrix_lfo2_phase_fresh_note_snaps_offset`
      (~2718) — drop the HashSet "finite for u32" block; keep the `d1==d2`
      retrigger-stability check.
- [x] `vxn2-engine/tests/param_sweep.rs` `algo_sweep_every_algo_renders_finite`
      (~160) — drop `peak >= 0.0` (tautology from `.abs()` seed); keep
      finiteness, optionally assert `peak > 0` for carrier algos.
- [x] `vxn2-ui-web/src/lib.rs` `panel_js_files_carry_expected_exports`
      (~650) — delete the compound `&&` assert that re-ANDs tokens already
      asserted individually on ~636/638.
- [x] `vxn2-ui-web/src/lib.rs` `build_params_json_covers_full_table` (~493)
      — the `id == i` / keys-exist checks are near-tautological; add a
      spot-check of one descriptor's actual field values against
      `desc_for_clap_id`.
- [x] `vxn2-dsp/src/stack.rs` `bend_scales_all_lane_increments` (~1283) —
      delete the dead `want` / `min(u32::MAX)` computation (`let _ = want`)
      that implies an untested saturation bound.

Stale framing:

- [x] `vxn2-engine/src/matrix.rs` `stack_pitch_route_evals_inert_no_panic`
      (~1567) — comment says "until 0069 wires the scatter"; 0069 shipped.
      Rename/rescope to pin what `eval_dests` still guarantees (single-column
      write), or fold into `pitch_dest_gain_scales_depth`.

- [x] `cargo test` green across the workspace; each repaired test verified
      to fail when its feature is deliberately broken (spot-check locally).

## Notes

The repair items are the load-bearing part of this epic — they are the
only tests that currently give false confidence. `envelope.rs`
`pitch_eg_default_idle_zero` (~339) is borderline (near-tautology given
default `l=[0,0,0,0]`); leave unless the Idle→target read path is
otherwise unexercised. Related redundancy pruning is 0162; this ticket is
only the cogency fixes.

## Close-out (2026-07-02)

Implemented by a Sonnet agent, gated + committed as `34f2499`.

- **Repaired (4):** `zero_drift_is_bit_exact_env` (reads real `env2` output,
  asserts bit-identical across lanes + positive attack); `resolve_route_clamps`
  (fn-ptr equality vs algo 1 / algo 32 saturation); `param_step_lands_at_offset`
  (reference render, asserts samples [0..64) unaffected by the offset-64 step);
  `vxn-web-controller` journal drains (decode + assert zero ops, ×2 sites).
- **Deleted (4):** `a4_is_440` (covered by `vxn-core-utils` `midi::a4_is_440`);
  `fresh_stack_is_idle` (covered by `note_off_to_idle_with_fast_release`);
  `mod_env_shape_field_persists_after_cook` (covered by `mod_env_lin_*` /
  `mod_env_exp_*`); `set_tempo_propagates_to_engine` (covered by
  `smoke.rs::tempo_edit_does_not_break_render`).
- **Trimmed (5):** dropped the u32 "finite" HashSet block from
  `matrix_lfo2_phase_fresh_note_snaps_offset`; `algo_sweep` now asserts
  finite + `peak > 0`; dropped the redundant `&&` compound in
  `panel_js_files_carry_expected_exports`; added a real descriptor spot-check
  to `build_params_json_covers_full_table`; removed the dead `want` computation
  in `bend_scales_all_lane_increments`.
- **Rescoped (1):** `stack_pitch_route_evals_inert_no_panic` →
  `stack_pitch_dest_writes_own_column_only` (0069 has shipped).
- **Skipped (1):** `key_mode_applied_before_events` — see the criterion above.

Tests green: `vxn-engine` (168+1), `vxn-wasm` (16), `vxn-web-controller` (21),
`vxn2-dsp` (172), `vxn2-engine`, `vxn2-ui-web` (27), `vxn2-clap` (7).
