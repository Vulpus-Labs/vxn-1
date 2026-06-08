---
id: "0107"
title: Spread=0 fast path — skip R decimator, alias R bus to L
priority: medium
created: 2026-06-07
epic: 
---

## Summary

After E019, every block pays for two parallel decimators even when
`spread = 0` makes the L and R buses bit-identical. The R decimator's
input matches L's, its state evolves identically, its output is the
same — pure waste at the default spread.

Restore the pre-E019 dry-path cost on the common default-spread path:

1. **Decimation**: when both layers have `spread = 0`, skip the R
   decimator. Run L only.
2. **Downstream**: copy the decimated L into the R bus before the
   master-volume / FX chain. From the FX entry onward the chain is
   unchanged (stereo throughout); the FX themselves produce identical
   L/R output from identical L/R input so the parallel cost there is
   not the focus.

Activation is per-block, engine-wide: both layers' spread values must
both be 0 for the fast path. If either layer is spread > 0 the
full-stereo path runs.

**Out of scope** for this ticket: a scalar voice-sum mono path inside
`VoiceBank::render_block`. The voice sum's dual-accumulator overhead
at spread = 0 is tiny (a few hundred extra writes per block) vs the
decimator's thousands of FIR ops, and extracting a mono variant from
render_block hurts maintainability more than the perf gain justifies.
Split out as a separate ticket if benches show it ever matters.

## Acceptance criteria

- [ ] `Synth::process` checks both layers' spread at block start.
      When both are zero:
      - Banks render normally into L + R OS buses (no bank-side
        change; voice sum still dual-writes identical content).
      - Only the L decimator runs.
      - The base-rate R buffer is copied from L (memcpy) before
        master-volume / FX entry.
- [ ] `Oversampler::clone_state_from(&Oversampler)` (and the
      underlying `HalfbandFir::clone_state_from`) added to vxn-dsp,
      copying tap-delay buffer + write position verbatim.
- [ ] Bit-identity test (baseline render at default patch) still
      passes with the fast path engaged.
- [ ] Mono→stereo transition: copy the L decimator's tap state into
      the R decimator the first block after the fast path
      deactivates. Without this, the R decimator starts with stale
      (or all-zero, post-reset) state while L is fully converged,
      producing a brief L-biased transient as R settles. Expose an
      `Oversampler::clone_state_from(&Oversampler)` helper (or
      equivalent) that copies the underlying half-band FIR tap
      buffers verbatim.
- [ ] Stereo→mono transition needs no state handling — L is the bus
      we keep running; R's stored state is just discarded and will
      be re-seeded from L on the next mono→stereo transition.
- [ ] Smoke test: stepping `spread` from 0 to a non-zero value and
      back produces no clicks at the transition (the L→R state copy
      makes the mono→stereo edge bit-identical to having run the
      stereo path all along; the stereo→mono edge is trivially
      continuous since L is uninterrupted).
- [ ] Bench `dry_4x` recovers most of the 25% slowdown introduced
      by E019 (target: within 5% of the pre-E019 ~51× RT baseline at
      `spread = 0`).
- [ ] `cargo test --workspace` green.

## Notes

Follow-up to E019 (sibling of 0106). The decimator-drain optimisation
in 0106 handles the silent path; this one handles the loud-but-default
path. They compose: at idle + spread=0, both kick in and only the L
decimator runs, draining to zero, then skipping entirely.

The linear pan law from 0104 means `pan_l[v] = pan_r[v] = 1.0` at
`spread = 0` — so the mono fast path is arithmetically equivalent to
the stereo path with spread = 0, not an approximation. Sample-for-
sample identity holds.

If spread ever becomes smoothed (per 0104 notes), the fast path
predicate is `smoother.spread(upper) == 0 && smoother.spread(lower)
== 0` on the per-block smoothed value, not the raw param. During
ramp blocks the stereo path runs; once the smoother settles at zero
the fast path kicks back in. Use an exact-zero check, not an epsilon
— the smoother converges to exact zero in finite time for a zero
target.

FX-chain dedup (skipping the parallel R cascade in StereoPhaser /
StereoChorus when L=R) is out of scope. The dual-chain state already
exists in those structs (per [[vxn1-fx-dual-chain-internally]]); a
shortcut would mean adding a "mono input" path back, undoing 0101 +
0102. The decimator win is the bigger one anyway.
