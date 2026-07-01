---
id: "0164"
product: vxn-2
title: Extract vxn2-engine shared test apparatus — render loop, click detector, route builder
priority: medium
created: 2026-07-01
epic: E031
---

## Summary

The vxn2-engine test suite carries the densest copy-paste in the review.
The block-render loop is hand-rolled 25+ times, the 4th-difference click
detector is reimplemented in 5 files, and the "install one matrix slot"
setup is pasted across the whole matrix-route family. `alloc.rs`
(`fast_patch`/`density1`/`run_blocks`, ~787) already shows the right
fixture pattern — mirror it in `engine.rs` and hoist the cross-file
helpers to a shared test module.

Line numbers are as-reviewed on 2026-07-01; re-grep by name.

## Acceptance criteria

- [ ] Add `fn render_blocks(e: &mut Engine, n: usize) -> (Vec<f32>,
      Vec<f32>)` (or the minimal peak/RMS variant already open-coded as the
      `render_energy`/`render_and_rms`/`block_peak` closures) to the
      `engine.rs` test module top, and route the ~25 copy-pasted
      `let mut l/r = [0.0; BLK]; for _ in 0..N { e.process_block(...) }`
      loops through it (e.g. `matrix_lfo1_to_op_level_modulates_audio`,
      `matrix_voice_rand_to_lfo2_phase_decorrelates_lanes`, `matrix_mod_
      wheel_to_lfo1_rate_sweeps_log_domain`, `matrix_key_to_stack_detune_
      shifts_phase_inc`, and the StackPitch/Lfo2Phase families).
- [ ] Add `fn engine_with_route(source, dest, depth) -> Engine` that builds
      `Engine::new(SR, BLK)`, resets `MatrixTable::default()`, and installs
      a single `MatrixSlot`; replace the pasted install boilerplate.
- [ ] Extract the 4th-difference click detector
      `|b[i+2] − 4b[i+1] + 6b[i] − 4b[i−1] + b[i−2]|` into a shared
      `worst_d4(buf, range) -> f64` (new `tests/common/` module, or a
      `#[cfg(test)]` export), and use it in `note_on_click.rs` (~101),
      `note_off_click.rs` (~36), `filter_integration.rs` (~259/273),
      `level_clamp_corner.rs` (~73), and the `render_hash` path in
      `engine.rs`.
- [ ] Factor `zipper_regression.rs`'s block-edge-vs-interior second-
      difference ratio (the local `ratio` closure ~59) into a file-level
      `fn edge_interior_ratio_of(buf: &[f32]) -> f64` and call it from all
      three sites (~59/126/184).

- [ ] `cargo test -p vxn2-engine` green; behaviour of each affected test
      unchanged (same assertions, less scaffolding).

## Notes

Coordinate the `tests/common/` location with 0167 (cross-crate CLAP
test-support) so the two don't create competing shared-test crates. If a
`vxn-test-support` crate is stood up there, `worst_d4` can live in it;
otherwise a per-crate `tests/common/mod.rs` is fine for the click
detector since it's engine-only. Buried-setup rewrites of `default_patch_
renders_with_expected_envelope` and `param_audibility` are 0168, not here.
