---
id: "0163"
product: monorepo
title: Collapse ui-web substring "wired" tests to asset-present guards
priority: medium
created: 2026-07-01
epic: E031
---

## Summary

The single largest low-value block in the review: ~30 `faceplate_*_wired`
tests in `vxn-ui-web/src/lib.rs` assert only that a JS/CSS/HTML token
*appears* in the assembled faceplate string (`assembled().contains("<js
token>")`). Token presence proves no behaviour — a match in a comment
passes as readily as a live call site, and the string can contain a
syntactically broken script. The real behavioural net is the gated Vitest
suite (`vxn-ui-web/src/lib.rs` ~1968). Collapse the per-token tests to a
handful of "the asset didn't vanish" guards and lean on the JS suite for
behaviour. Same disease, smaller, in `vxn2-ui-web`.

Line numbers are as-reviewed on 2026-07-01; re-grep by test name.

## Acceptance criteria

vxn-1 `vxn-ui-web/src/lib.rs`:

- [ ] Replace the ~30 `faceplate_*_wired` token-presence tests (~945–1958:
      `faceplate_bridge_object_intact`, `faceplate_text_input_bridge_wired`,
      `faceplate_status_pill_wired`, `faceplate_preset_bar_wired`,
      `faceplate_browser_mutation_flows_wired`, `faceplate_save_as_modal_
      wired`, `faceplate_browser_search_is_cross_folder`, `faceplate_
      browser_panel_wired`, `edit_layer_rebind_wired`, `header_switch_
      primitive_wired`, `keys_panel_wired`, `filter_mode_notch_dims_slope_
      strip`, `faceplate_browser_drag_drop_wired`, and siblings) with a
      small number of "asset present" guards — one per embedded asset
      (bootstrap JS, panel JS, faceplate CSS), asserting the asset is
      non-empty and spliced (no `__PLACEHOLDER__` tokens remain).
- [ ] Keep `control_tallies_match_all_rows` (~1541, catches duplicate
      mounts — real value) and the byte-identical params test (~1860).
      Fold the four `row{1..4}_*_have_expected_mounts` (~1302–1539) into a
      single `assert_mounts(&[(kind, name, label)])` helper driven by four
      data tables (the mount markers are behavioural DOM contract, worth
      keeping — just deduped).
- [ ] Add a code comment on the surviving guards pointing to the Vitest
      suite as the behavioural net, so the pattern isn't regrown.

vxn-2 `vxn2-ui-web/src/lib.rs`:

- [ ] `bootstrap_js_declares_required_surface` (~606),
      `panel_js_files_carry_expected_exports` (~619), `faceplate_css_
      carries_mockup_rules` (~592) — reduce to one asset-present guard each
      (or delete if the vxn-1 pattern already covers the shared asset).
- [ ] Collapse `build_faceplate_html_splices_css_and_bootstrap` (~507) into
      `build_faceplate_html_bundles_full_js_stack` (~674, superset); carry
      over the unique `color-scheme: dark` check.
- [ ] Keep the two `algo_data_*_match_engine_table` drift guards (~701/768)
      — genuine engine/JS drift protection — but extract their hand-rolled
      `const X = [ ... ];` slicing into `fn extract_js_array_body(js,
      decl_name) -> &str` so both share it.
- [ ] Add `fn matrix_lists_value() -> serde_json::Value` (parse
      `build_matrix_lists_json()` once) shared by the two matrix-lists
      tests (~916/940).

- [ ] `cargo test` green; Vitest suite still runs and passes; net test
      count drops but no behavioural coverage lost (behaviour lived in
      Vitest, not these greps).

## Notes

`dump_spliced_html` (vxn2-ui-web ~660) is `#[ignore]` with no assertions —
a dev tool; leave it ignored. The point of this ticket is that
`str::contains` over an asset blob is a change-detector, not a behaviour
test; do not grow the pattern. If a token genuinely must exist for wiring,
the Vitest suite exercising that wiring is the correct guard.
