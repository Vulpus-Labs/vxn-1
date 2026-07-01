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

- [ ] `vxn-engine/src/voice.rs` `zero_drift_is_bit_exact_env` (~2409) —
      currently asserts `x * 0.0 == 0.0` and recomputes inline. Read back
      the actual `env2[v]` params after `set_envelopes(..., 0.0)` and
      assert all voices got identical values.
- [ ] `vxn2-dsp/src/algo.rs` `resolve_route_clamps` (~511) — currently
      `let _ =` discards the result. Assert the saturated input
      (`resolve_route(0)`) yields the same routing as `resolve_route(1)`,
      mirroring `component_range_guards` (~604).
- [ ] `vxn-wasm/src/host.rs` `key_mode_applied_before_events` (~453) —
      currently only `onset(...).is_some()`. Render note 72 in Split and
      in Whole and assert the buffers differ (routing actually changed).
- [ ] `vxn-wasm/src/host.rs` `param_step_lands_at_offset` (~370) —
      name promises the change lands at offset 64 but asserts only
      "something audible." Split the render at 64 with old/new value and
      compare; assert pre-64 unaffected.
- [ ] `vxn-web-controller/src/lib.rs` (~1414) — `assert_eq!(take_journal(),
      4, ...)` uses a magic length to mean "empty." Decode and assert zero
      ops, or introduce a named `EMPTY_JOURNAL_LEN` const.

Delete (property already covered by a named test — cite it in the commit):

- [ ] `vxn-engine/src/lib.rs` `a4_is_440` (~1039) — re-asserts vxn-dsp's
      `note_to_hz(69)` constant; belongs to vxn-dsp, tests no engine logic.
- [ ] `vxn2-dsp/src/stack.rs` `fresh_stack_is_idle` (~1099) — tests the
      `Default` derive only.
- [ ] `vxn2-dsp/src/envelope.rs` `mod_env_shape_field_persists_after_cook`
      (~557) — asserts a field copy; effect covered by `mod_env_lin_*` /
      `mod_env_exp_*` (~452/481).
- [ ] `vxn2-clap/src/lib.rs` `set_tempo_propagates_to_engine` (~1029) —
      getter-after-setter on the engine, no CLAP path; transport decode is
      covered by `smoke.rs` (~488). Delete, or redirect through the
      transport-event dispatch path if that was the intent.

Trim tautological sub-assertions (keep the test, drop the dead part):

- [ ] `vxn2-engine/src/engine.rs` `matrix_lfo2_phase_fresh_note_snaps_offset`
      (~2718) — drop the HashSet "finite for u32" block; keep the `d1==d2`
      retrigger-stability check.
- [ ] `vxn2-engine/tests/param_sweep.rs` `algo_sweep_every_algo_renders_finite`
      (~160) — drop `peak >= 0.0` (tautology from `.abs()` seed); keep
      finiteness, optionally assert `peak > 0` for carrier algos.
- [ ] `vxn2-ui-web/src/lib.rs` `panel_js_files_carry_expected_exports`
      (~650) — delete the compound `&&` assert that re-ANDs tokens already
      asserted individually on ~636/638.
- [ ] `vxn2-ui-web/src/lib.rs` `build_params_json_covers_full_table` (~493)
      — the `id == i` / keys-exist checks are near-tautological; add a
      spot-check of one descriptor's actual field values against
      `desc_for_clap_id`.
- [ ] `vxn2-dsp/src/stack.rs` `bend_scales_all_lane_increments` (~1283) —
      delete the dead `want` / `min(u32::MAX)` computation (`let _ = want`)
      that implies an untested saturation bound.

Stale framing:

- [ ] `vxn2-engine/src/matrix.rs` `stack_pitch_route_evals_inert_no_panic`
      (~1567) — comment says "until 0069 wires the scatter"; 0069 shipped.
      Rename/rescope to pin what `eval_dests` still guarantees (single-column
      write), or fold into `pitch_dest_gain_scales_depth`.

- [ ] `cargo test` green across the workspace; each repaired test verified
      to fail when its feature is deliberately broken (spot-check locally).

## Notes

The repair items are the load-bearing part of this epic — they are the
only tests that currently give false confidence. `envelope.rs`
`pitch_eg_default_idle_zero` (~339) is borderline (near-tautology given
default `l=[0,0,0,0]`); leave unless the Idle→target read path is
otherwise unexercised. Related redundancy pruning is 0162; this ticket is
only the cogency fixes.
