# ADR 0005 — Stack pitch modulation

- **Status:** Accepted
- **Date:** 2026-06-18
- **Scope:** Add a pitch-only mod-matrix destination that targets an operator
  **and its whole ratio-coherent FM stack** at once, so a single pitch route
  bends a carrier and every operator feeding it by the same amount. Epic E022
  (tickets 0067–0071). Extends the mod matrix of
  [ADR 0001 §6](0001-vxn2-overall-design.md); changes nothing about the
  algorithm graph itself.

## Context

Per-op pitch modulation already exists (`OpNPitch` dests). But modulating a
single op inside an FM stack detunes it *relative to* its neighbours: the FM
ratio breaks, so the timbre smears instead of the pitch bending cleanly. To
bend a whole tone in tune you must apply the **same** pitch delta to every op
in the branch. Done by hand that is N routes kept in lock-step. This makes it
one route.

## Decision

Six new per-lane pitch destinations `Op1StackPitch..Op6StackPitch`, appended
after `Resonance` in `DestId` (matrix surface, 0068). Encoding the target op in
the dest enum keeps routes plain `(source, dest, depth)` — no extra per-route
field. At cook time each `OpNStackPitch` route's modulated value is **scattered**
into the per-op pitch of every op in op N's connected component of the
algorithm's modulation graph (0067 resolver + 0069 scatter).

Settled semantics:

1. **Pitch only, by intent.** Ratio-lock is the entire point; the *same* mod
   domain / curve / depth / cubic taper / ±24 st gain as the existing per-op
   pitch dest. Pure target fan-out — no new mod-value code. Stack mod for any
   non-pitch dest (level, pan, …) is explicitly **out of scope**: those have no
   ratio to preserve, so the feature would be meaningless for them
   ([[vxn2-architecture]]).

2. **Equal delta, not depth-scaled.** Ratio-lock *requires* every op shift by
   the same number of semitones — frequency then scales by one common factor
   `2^(Δst/12)` and the ratios are invariant. Depth-scaling or weighted
   propagation would break that, so the simplest law is the correct one.

3. **Whole connected component, undirected.** Traversal over modulation edges
   is undirected: targeting a mid-stack op propagates both up (to its
   modulators) and down (to its carriers). The resolver
   (`vxn2_dsp::algo::pitch_stack_component`) is a pure, allocation-free integer
   flood-fill returning a 6-bit op mask.

4. **Fixed-frequency ops are connectivity walls.** A fixed-Hz op does not track
   key, so tuning coherence genuinely stops there. It is removed as a graph
   *node* (not merely excluded from the result), so a fixed op mid-chain splits
   the graph into independent components. A route whose **target** op is fixed
   resolves to an empty component → clean no-op.

5. **Shared modulators legitimately produce large components.** DX7 algorithms
   share modulators across carriers (e.g. algo 22's op6 fans into carriers
   3/4/5). Targeting a shared modulator therefore sweeps every carrier it
   feeds. This is correct ratio-lock behaviour, not a bug — but it is a
   user-facing surprise worth documenting: one route can bend most of a patch.

6. **Re-resolve only on topology change.** The component masks are a pure
   function of `(algo, ratio-mode × 6)`. They are precomputed once per cook,
   cached, and re-resolved **only** when the algorithm changes or an op crosses
   the Ratio↔Fixed boundary — folded into the dirty-bitset pump
   ([ADR 0003](0003-dirty-bitset-diff-pump.md)) alongside the existing
   `set_algo_live` re-cook. A ratio-*value* tweak (2.0→3.0) keeps the op
   tracking key, so the component is unchanged and must **not** re-resolve;
   the cache is gated on the `(algo, wall_mask)` key, not the continuous ratio
   param.

## Implementation

The scatter runs in the block-rate cook, before the per-stack `PitchSmoother`
captures its targets: each stack-pitch accumulator is added into the per-op
pitch columns of its component, so the existing pitch smoothing + per-op
`phase_inc` recompute carry it with **no audio-inner-loop change** — SoA op
packing ([[vxn2-stack-soa]]) is untouched. The un-targeted path is gated to
stay bit-identical to pre-E022. Wire format: the matrix-trailer dest is a `u8`,
so the dest space widens with no byte-layout change; the blob version is
nonetheless bumped (v9 → v10) purely as a forward-compat guard so a pre-E022
build rejects a patch that uses a stack-pitch dest rather than silently
dropping the route.

## Consequences

- One new input to an existing cook step; the resolver stays a pure function
  `(algo, [ratio_mode; 6], target_op) → op bitset`, unit-tested against the
  published DX7 chart.
- The mod-target resolver now reads op freq-mode in addition to the algorithm,
  so the dirty-bitset pump gains two trigger sources: `algo` and per-op
  `ratio-mode` (toggle only).
- Ratio-lock, wall-split, shared-mod spread, fixed-target no-op, and recook
  gating are locked by integration tests over the engine cook + a render
  (0069 / 0071).
