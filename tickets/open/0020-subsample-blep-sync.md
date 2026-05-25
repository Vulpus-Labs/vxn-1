---
id: "0020"
title: Sub-sample BLEP-softened hard sync
priority: high
created: 2026-05-25
epic: E006
---

## Summary

Make hard sync **band-limited**. Today [`vxn-dsp::poly::process_pair`] resets the
slave on the sample where the master wraps — sample-accurate, so the reset
jitters up to ~1 sample and the discontinuity sprays aliasing. Port the
sub-sample + polyBLEP sync path that already exists in `patches-dsp` so the slave
resets at the exact fractional crossing and the reset edge is BLEP-softened
(which also gives the mild analog "rounding").

Removes the `TODO(E002 follow-up): band-limited (minBLEP) sync correction` note
in [poly.rs].

## Reference (patches-dsp::oscillator)

- `advance_all_wrap_frac() -> [f32; N]` — advance + return the master's
  sub-sample wrap fraction `t ∈ (0,1]` per voice (`0` = no wrap this sample).
  `t = 1 - phase/dt` from the post-wrap remainder.
- `sync_reset(voice, frac)` — slave phase = `(1 - frac) · inc`, accounting for
  the remainder of the current sample.
- `sync_blep_residual(post_phase, post_dt, delta) = polyblep(post_phase,
  post_dt) · 0.5 · delta` — add to the post-reset slave sample. `delta = pre −
  post` is the slave waveform's value jump across the reset.

## Design

In `process_pair`, the master is osc1 with the **cross-mod-modulated** increment
`inc1 = self.inc[v] · exp2(xmod · o2)` — that modulated `inc1` is the master
`dt` for the wrap-fraction maths (it's already the polyBLEP `dt` today).

Per voice, per sample:

1. Compute `o2` (slave) and `o1` (master) as now.
2. Advance the master capturing the wrap **and** its fraction: when `np1 ≥ 1.0`,
   `frac = 1 - (np1 - 1.0)/inc1` (clamped to `(MIN_POSITIVE, 1]`).
3. Advance the slave normally; if `sync` and the master wrapped, set the slave
   phase to `(1 - frac) · slave.inc[v]` (fractional reset) instead of hard 0.
4. Compute the slave's value jump `delta = pre − post` across the reset and add
   `sync_blep_residual` to `o2[v]` (and the master's own wrap keeps its existing
   polyBLEP via `osc_sample`).

Keep the lane loop branchless/vectorisable: the reset and residual are
mask-scaled by `wrapped · sync_f`, as the current masked reset already is.
`xmod`-only patches (sync off) must stay byte-identical to today; sync-off,
xmod-0 must stay bit-identical to the independent fast path
(`coupled_xmod_zero_matches_fast_path` must still pass).

Decide whether to lift the patches-dsp helpers into shared functions or inline
the maths in `process_pair`; the kernel is SoA `[f32; N]`, patches' poly variant
is `[f32; 16]`, so a small inline port is likely cleaner than a dependency.

## Acceptance criteria

- [ ] Slave resets at the sub-sample fractional crossing (phase `(1-frac)·inc`),
      not at the sample boundary.
- [ ] polyBLEP residual is applied to the slave across the reset; measured
      aliasing (out-of-band energy) for a synced saw is materially lower than the
      sample-accurate version at the same settings.
- [ ] `synced_slave_locks_to_master_period` still holds (slave periodic at the
      master period).
- [ ] `coupled_xmod_zero_matches_fast_path` still bit-identical.
- [ ] `synced_pair_all_lanes_finite` still passes (mixed waves, frozen lanes,
      sync + heavy xmod together).
- [ ] No RT allocation; lane loop still vectorises.

## Notes

- DSP-only; no param/UI change. Independent of 0022 — the sync flag stays a
  bool here; 0022 later swaps the *source* of that bool to `CrossModType::Sync`.
- Validation: `cargo test -p vxn-dsp`. Add an aliasing assertion modelled on
  `patches-integration-tests/tests/hard_sync_aliasing.rs`.
