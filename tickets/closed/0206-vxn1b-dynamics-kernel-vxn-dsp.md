---
id: "0206"
product: vxn-1b
title: "Copy the Dynamics kernel from VXN2 into the shared vxn-dsp crate"
priority: high
created: 2026-07-29
epic: E037
depends: []
---

## Summary

E037's one new FX kernel is **Dynamics** — a feed-forward peak compressor →
`tanh` saturator with a wet/dry glide. It already exists, fully tested, in VXN2
at [dynamics.rs](../../vxn-2/crates/vxn2-dsp/src/dynamics.rs). This ticket copies
it into the shared `vxn-dsp` crate ([vxn-dsp/src](../../vxn-1/crates/vxn-dsp/src))
so [0207](0207-vxn1b-fx-chain-wiring.md) can wire it into the VXN1b FX chain
alongside the four kernels already there (chorus, phaser, delay, reverb).

The port is **additive** — a new `dynamics` module, no edits to shared paths — so
VXN1 (which depends on `vxn-dsp` but does not route dynamics) is unaffected.

**On the "delete its internal oversampling" line in the epic:** the *kernel* has
no oversampling stage to delete. `DynamicsBlock`
([dynamics.rs:85-287](../../vxn-2/crates/vxn2-dsp/src/dynamics.rs#L85-L287)) is
already rate-agnostic — one stereo sample in / out. VXN2's 4× oversampling lives
in the **engine** (`vxn2-engine`'s `run_dynamics_os()` interpolate → process →
decimate), *not* in `dynamics.rs`. So this ticket copies the kernel verbatim
(modulo the dep adaptations below); the "runs at the global OS rate" requirement
is satisfied for free — there is no OS wrapper to strip, and 0207 simply calls
`process` inside whatever rate the engine's block loop is at.

## Design

Copy [dynamics.rs](../../vxn-2/crates/vxn2-dsp/src/dynamics.rs) into
`vxn-dsp/src/dynamics.rs`, adapting to `vxn-dsp` conventions:

- **Module wiring.** Add `pub mod dynamics;` to
  [lib.rs](../../vxn-1/crates/vxn-dsp/src/lib.rs) (alphabetical, between
  `delay_line` and `fdn_reverb`) and a `pub use dynamics::{DynamicsBlock,
  DynamicsParams};` in the re-export block. Match the `Stereo*` naming? — VXN2
  calls it `DynamicsBlock`, not `StereoDynamics`; keep `DynamicsBlock` for a
  verbatim copy rather than renaming (chorus/phaser/delay use `Stereo*`, reverb
  uses `FdnReverb`, so the crate already mixes conventions).
- **Smoother dep.** VXN2 imports `use crate::smoother::{one_pole_coeff,
  Smoothed};`. `vxn-dsp` has **no** `smoother` module — it re-exports the same
  types from `vxn_core_utils::smoothing` at
  [lib.rs](../../vxn-1/crates/vxn-dsp/src/lib.rs) (`pub use
  vxn_core_utils::smoothing::{self as smoothing, Smoothed, …, one_pole_coeff}`).
  The `Smoothed` API is identical (`new(initial, ms, sr)`, `set_target`, `snap`,
  `tick`, `current` — verified in
  [smoothing.rs](../../crates/vxn-core-utils/src/smoothing.rs)), so the only
  change is the import line: `use crate::smoother::…` → `use crate::smoothing::…`.
- **Math dep.** `use crate::math::fast_tanh;` is unchanged — `vxn-dsp`'s
  [math.rs](../../vxn-1/crates/vxn-dsp/src/math.rs) exports `fast_tanh`.
- **Test helpers.** The VXN2 tests use `crate::test_util::{assert_bit_exact_passthrough,
  assert_bit_exact_after_settle}`. `vxn-dsp` has **no** `test_util` module. Add
  the two helpers (as a `#[cfg(test)]` mod or inline in the dynamics test
  module), porting them from VXN2's `vxn2-dsp/src/test_util.rs`, so the full
  test set comes across unchanged. Prefer a shared `#[cfg(test)] mod test_util`
  if other vxn-dsp kernels would benefit; otherwise inline is fine.

Keep the copy byte-faithful otherwise: the 8-param surface (`on, threshold_db,
ratio, attack_ms, release_ms, makeup_db, drive_db, mix`), the comp→sat order,
the wet/dry glide + on/off discipline (steady-off = bit-exact passthrough), and
the detector reset on the inactive→active edge.

## Acceptance criteria

- [ ] `vxn-dsp/src/dynamics.rs` exists; `DynamicsBlock` + `DynamicsParams` are
      declared in `lib.rs` and publicly re-exported.
- [ ] The full VXN2 test set is ported and green in `vxn-dsp`: bit-exact
      passthrough off-from-load, switch-on fade-up, switch-off fade-then-settle,
      gain-reduction curve (−20 dB threshold / ratio 4 → −15 dB GR), tanh-drive
      flattening, detector reset on inactive→active, mix=0 is dry.
- [ ] Purely additive — **no** edits to existing `vxn-dsp` module files beyond
      the `pub mod` / `pub use` lines in `lib.rs`.
- [ ] Both suites green (shared crate — `vxn-no-parallel-cargo-test`, run once,
      capture to file, grep): `cargo test -p vxn-dsp` and VXN1's engine/plugin
      suite show no regression from the added module.

## Notes

- No OS stripping needed — the kernel is already rate-agnostic; the epic's
  "delete its dedicated oversampling" refers to *not* carrying over VXN2's
  engine-side `run_dynamics_os()` wrapper, which this ticket doesn't copy.
- **Dynamics-without-OS risk (per epic).** At 1× the saturator can alias on fast
  transients. That's an engine-integration concern for [0207](0207-vxn1b-fx-chain-wiring.md)
  (verify clean at the default 2×, note if 1× needs a caveat) — this ticket only
  lands the rate-agnostic block.
- Related design lineage: [[vxn2-level-mod-pipeline]], [[vxn2-e006-review-remediation]].

## Close-out (2026-07-29)

- **Kernel ported.** `DynamicsBlock` + `DynamicsParams` copied verbatim from
  VXN2 into [dynamics.rs](../../vxn-1/crates/vxn-dsp/src/dynamics.rs). Only dep
  adaptation: `use crate::smoother::…` → `use crate::smoothing::…` (vxn-dsp
  re-exports `Smoothed`/`one_pole_coeff` from `vxn-core-utils` — API identical,
  drop-in); `use crate::math::fast_tanh` unchanged. 8-param surface, comp→sat
  order, wet/dry glide + on/off discipline, detector reset all intact.
- **No OS stage stripped** — the kernel is already rate-agnostic (one stereo
  sample in/out). VXN2's oversampling lives in its *engine* (`run_dynamics_os`
  interp→process→decimate), not the kernel, so nothing to remove; the epic's
  "delete its dedicated oversampling" is a no-op at the kernel level. Ticket +
  epic annotated. "Runs at the global OS rate" is satisfied for free — 0207 just
  calls `process` at whatever rate the engine block loop is at.
- **Module wired.** [lib.rs](../../vxn-1/crates/vxn-dsp/src/lib.rs): `pub mod
  dynamics;` (alphabetical, between `delay_line`/`fdn_reverb`) + `pub use
  dynamics::{DynamicsBlock, DynamicsParams};`. Purely additive — no edits to any
  existing module file beyond these two lib.rs lines.
- **Test helpers inlined.** vxn-dsp has no shared `test_util`; ported the two
  helpers (`assert_bit_exact_passthrough`, `assert_bit_exact_after_settle`) into
  the dynamics `#[cfg(test)]` module so the full VXN2 test set came across
  unchanged.
- **Green** (`vxn-no-parallel-cargo-test`, run once, captured): all 7 dynamics
  tests pass — `dynamics::tests::{off_from_load_is_bit_exact_from_first_sample,
  switch_on_after_load_off_glides_up_from_zero, switch_off_fades_then_settles_to_bit_exact,
  gain_reduction_matches_known_threshold_ratio, tanh_drive_flattens_sine,
  detector_resets_on_inactive_to_active_edge, mix_zero_is_dry}`. Full vxn-dsp
  suite 90/90. VXN1 consumer `vxn-engine` builds clean (no shared-crate
  regression). clippy: only pre-existing `tap`-index warning, unrelated.
</content>
</invoke>
