---
id: "0262"
product: vxn-1b
title: "Drop the spread-zero mono fast path from OutputStage"
priority: medium
created: 2026-08-07
epic: E039
depends: ["0248", "0251"]
---

## Summary

The oversampling work (0251) carries a mono optimisation ported from VXN1 0107:
when `Spread == 0` on every sounding layer, `Engine` passes a `spread_zero` hint
and `OutputStage::decimate_block` **skips the R decimator and copies R from L**,
with a mono→stereo state seed on the transition.

That hint is only valid while the *only* thing that can decorrelate L and R is
the Spread param. **0248** breaks it — a layer panned off centre with spread 0
has its pan silently discarded by the copy. **0260** breaks it further: with pan
as a modulation destination, "is this patch mono?" becomes a per-sample question
that a block-rate hint cannot answer at all, so no amount of extra predicates
rescues it.

Delete the optimisation rather than growing its condition.

## Design

Remove:

- the `spread_zero` parameter of `OutputStage::decimate_block` and the
  `spread_zero_last_block` field (plus its `new`/`reset` initialisation),
- the R-decimator skip and the `dst_r.copy_from_slice(dst_l)` branch,
- the mono→stereo `clone_state_from` seed,
- the `spread_zero` computation in `Engine::render_control_block`,
- the `spread_zero_keeps_channels_identical` and
  `mono_to_stereo_transition_is_seeded_not_stepped` tests, and the
  `spread_zero` argument of the tests' `decimate` helper.

Keep the `both_silent` drain-skip — it is a separate optimisation, keyed on
actual silence rather than on a stereo-correlation guess, and pan does not affect
it.

`#[allow(clippy::too_many_arguments)]` on `decimate_block` may become
unnecessary once the argument goes.

## Acceptance criteria

- [ ] `spread_zero` appears nowhere in `vxn1b-engine`; R is decimated
      unconditionally.
- [ ] Test: `spread == 0` with a non-centre `layer_pan` yields `L != R` at OS 8×
      (this is the case the fast path swallowed).
- [ ] A centred, spread-0 patch renders identically before and after the removal
      at OS 1× and 8× — the R decimator now runs, and must produce what the copy
      used to.
- [ ] The `both_silent` drain-skip still works: after `DECIMATOR_DRAIN_BLOCKS`
      silent blocks both channels zero-fill.
- [ ] No OS-change crossfade regression — the fade still applies to both channels
      with identical weights.

## Notes

- Cost: one extra decimator pass per block on centred patches, and only when
  oversampling is on. At OS 1× the decimator is a pass-through copy, so main's
  current behaviour is unaffected either way.
- Sequencing: 0248 (layer pan) and 0251 (oversampling) are independent of each
  other and both are landing separately; this ticket is the join. It cannot be
  written until 0251's `output.rs` is on main.
