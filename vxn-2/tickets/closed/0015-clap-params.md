---
id: "0015"
title: CLAP params extension wired to ParamTable
priority: high
created: 2026-06-05
epic: E002
---

## Summary

Wire the CLAP `params` extension to `vxn2-engine::params::ParamTable`
(from 0012) so the host enumerates the full parameter table, displays
correct names / ranges / units / default values, formats values for the
generic-controls panel, and parses text edits back.

After this ticket the host's parameter UI is fully populated, parameters
group by section, and reading values reflects the shared store — but
moving them produces no sound yet (0016 routes them into the engine).

## Acceptance criteria

- [ ] `PluginMainThreadParams` impl on `VxnMainThread`:
      - `count()` returns `TOTAL_PARAMS`.
      - `get_info(param_index, info)` writes a `ParamInfo` populated
        from `desc_for_clap_id(idx)`: id, name (`desc.label`), module
        (from `module_for_clap_id` — groups by Upper / Lower / Global /
        Op-N etc., for the host's automation list), `min_value`,
        `max_value`, `default_value`. Flags: always
        `IS_AUTOMATABLE`; add `IS_STEPPED` for non-float kinds.
      - `get_value(param_id)` reads from `SharedParams::get`.
      - `value_to_text(param_id, value, writer)` writes
        `desc.display(value)` (the descriptor owns its unit
        formatting).
      - `text_to_value(param_id, text)` takes the leading numeric
        token (digits + `.` + `-`) and parses as f64. Unit suffix
        ignored (matches VXN1 behaviour).
      - `flush(input, output)` folds host param events into the
        shared store via a small helper; for E002 it writes directly
        with `SharedParams::set` (no Controller). Output events stay
        empty until the UI epic.
- [ ] `PluginAudioProcessorParams::flush` on `VxnAudioProcessor`
      folds events into `LocalParams::apply_input`, calls
      `synth.set_param(idx, value)` for each touched id, and
      `LocalParams::publish` at the end so the shared store reflects
      the new values for `get_value` on the next host poll.
- [ ] `vxn2-engine::params` exports `desc_for_clap_id(idx: usize) ->
      Option<&'static ParamDesc>` and `module_for_clap_id(idx: usize)
      -> &'static str`. Module strings group params logically — e.g.
      `Upper / Op 1`, `Upper / LFO 2`, `Global / Delay`, `Global /
      Master` — so the host's tree-view is navigable.
- [ ] Stepped vs continuous correctly flagged: enums + ints get
      `IS_STEPPED`; floats do not. Verified against `ParamKind` from
      0012.
- [ ] Range fidelity: spot-test 6 params (1 per kind: per-op ratio
      float, algorithm int, lfo1 shape enum, mod env shape enum,
      stack_distrib enum, master_volume float) and assert that
      `min_value`, `max_value`, `default_value` match `PARAMETERS.md`.
- [ ] Display strings: `value_to_text` for `master_volume = -6.0`
      yields `"-6.0 dB"` (or whatever the descriptor formatter
      returns); for `algo = 5` yields `"5"`; for `lfo1_shape = 0`
      yields `"Sine"`. Add 5–10 spot tests.
- [ ] Integration test (with a stub `OutputEvents`): drive
      `PluginMainThreadParams::flush` with a `ParamValue` event for
      `master_volume = -3.0`; assert `SharedParams::get` reflects
      that value. Same for `PluginAudioProcessorParams::flush` after
      a Synth is wired (gate on 0016 if needed; skeleton test here).

## Notes

The 174-param `ParamTable` is large — `desc_for_clap_id` must be O(1)
(direct index into a `const` slice) not a linear search. 0012's
implementation already enforces this.

Module strings deserve thought: the host's automation panel shows them
as folders. `Upper / Op 1` reads better than `op1_upper`. Use `/` as
the separator. Strings can be `&'static str` baked into the descriptor
metadata, no runtime allocation.

`text_to_value` parsing the leading numeric token (not the whole
string) is deliberate: hosts pass `"-6.0 dB"` back through the same
field that displays it. The lazy parse matches VXN1 and is
forgiving enough for typed input like `"−6"` (after the en-dash
normalisation the host may do for us).

Sync-aware display strings (e.g. delay time showing `1/8` when
`delay_sync` is on) are out of scope here — that's a polish item for
the UI epic. The plain unit display in this ticket is enough for
generic-control playthrough. Note in code where the sync-aware
branch would slot in so future-you doesn't have to re-discover the
seam.
