---
id: "0328"
product: vxn-2
title: "Mod-matrix eval doesn't vectorise: transpose lane accumulators to dest-major"
priority: medium
created: 2026-08-30
epic: E049
depends: ["0329"]
---

## Summary

Prerequisite for [E049](../../epics/closed/E049-shared-matrix-routing.md): the
shared evaluator ([0334](0334-share-the-evaluator.md)) cannot be built until
both synths agree on accumulator layout, and this is where vxn-2 converges on
vxn-1b's. Opened standalone (at priority low) before that epic existed; now on
E049's critical path — 0334 waits on it — hence medium. It depends on
[0329](0329-vxn-core-matrix-crate-skeleton.md) for one reason only: the null
test below is unmeasurable until 0329's harness exists, and the reference
renders must be captured before this ticket reorders any arithmetic.

[`eval_dests`](../../vxn-2/crates/vxn2-engine/src/matrix.rs#L1201) leaves its
main accumulate loop scalar. Measured on the **linked** bench binary
(`llvm-objdump` on `target/release/deps/matrix-*`, post-LTO): 895 instructions,
277 scalar FP ops, and 16 vector ops — all of which are 2-wide and all of which
are in the *scale-VCA* loop (`fold_bipolar`'s `+1.0` / `×0.5`, `clamp_unit`'s
`fmaxnm`/`fminnm`). The route accumulate itself contributes none.

> **Measure post-LTO or not at all.** An earlier revision of this ticket claimed
> "zero NEON arithmetic ops, 1592 instructions" from `cargo rustc --emit asm` on
> the library crate. That number was meaningless: with `lto` set, cargo passes
> `-C linker-plugin-lto` and rustc defers the optimisation pipeline — **the loop
> vectorizer included** — to link time. Under that method a 1024-element
> `d[i] = s[i] * 2.0` also compiles scalar, which is the tell. Use
> `llvm-objdump` from the rustup toolchain on a linked artifact. See
> [[vxn-per-crate-asm-has-no-vectoriser]].

That the *same function* vectorises its contiguous loop and not its strided one,
under one compiler invocation at one optimisation level, is the cleanest
available evidence for the diagnosis below.

The cause is the accumulator layout. Both matrix buffers are **lane-major**:

```rust
pub type LaneSourceVals = [[f32; N_SOURCES]; STACK_LANES];  // matrix.rs:1127
pub type LaneDestVals   = [[f32; N_DESTS];   STACK_LANES];  // matrix.rs:1130
```

Every per-slot inner loop walks lanes with the source and dest indices fixed:

```rust
for k in 0..STACK_LANES {
    out[k][di] += $shape($pol(sources[k][si])) * depth * scale[k];
}
```

`sources[k][si]` strides `N_SOURCES` floats per lane and `out[k][di]` strides
`N_DESTS` floats per lane, so the loop is a gather/scatter — LLVM can't form a
contiguous 4-wide load or store and falls back to scalar. Every branch and match
is already hoisted out of these loops (ticket work on 2026-08-30 took a
fully-scaled 16-slot eval from 253ns → 125ns by hoisting `scale_norm`'s
dispatch), so the loop bodies are as straight-line as they can get. Layout is
what's left.

Transposing to **dest-major** — `[[f32; STACK_LANES]; N_DESTS]` — makes the
lane loop contiguous and 4-wide-able across the 8 lanes.

## Design

The transpose is not speculative: [`PitchSmoother`](../../vxn-2/crates/vxn2-engine/src/matrix.rs#L1275)
**already** stores its state dest-major (`[[f32; STACK_LANES]; N_PITCH_DESTS]`),
and [`targets_from`](../../vxn-2/crates/vxn2-engine/src/matrix.rs#L1311) exists
purely to transpose `LaneDestVals` into that shape on every block. So the engine
already pays for one transpose per stack per block, and dest-major would delete
it rather than add one.

What resists the change is the consumer side. `dest_vals` is
[declared lane-major in the engine](../../vxn-2/crates/vxn2-engine/src/engine.rs#L395)
and read as `dest_vals[i][k][idx]` in **17 places** — the stage-5 pitch
resolution and the stage-6 per-op level/pan/phase target projection
([engine.rs:1496-1531](../../vxn-2/crates/vxn2-engine/src/engine.rs#L1496-L1531))
being the dense ones. Those reads mostly walk *one dest across all lanes*, which
is exactly the access pattern dest-major favours, so several of them should get
faster too — but each has to be re-derived and re-checked, not blind-swapped.

`scatter_stack_pitch` and the stack-pitch masks also index the block and need
the same treatment.

Suggested order: flip `LaneDestVals` first (bigger win, and it's the one
`PitchSmoother` already wants), measure, then decide whether `LaneSourceVals`
is worth flipping separately — the source side is read once per slot per lane
rather than read-modify-written, so it may matter less.

## Acceptance criteria

- [ ] `LaneDestVals` is `[[f32; STACK_LANES]; N_DESTS]`; `eval_dests` writes it
      contiguously across lanes.
- [ ] The route accumulate in `eval_dests` contains 4-wide arithmetic.
      **Measure on a linked binary** (`cargo bench --no-run`, then `llvm-objdump
      -d --disassemble-symbols=…`) — per-crate `--emit asm` runs no vectoriser
      at all and will show scalar code either way. Grep the **mnemonic** for
      `.4s`, not the operands ([[vxn1-neon-grep-pitfall]]).
- [ ] `matrix_eval_full` and `matrix_eval_scaled` (vxn2-osc-bench `matrix`)
      both improve, or the ticket is closed as "measured, not worth it" with the
      numbers recorded.
- [ ] `PitchSmoother::targets_from` is gone or reduced to a borrow — the
      transpose it performs is the thing this ticket removes.
- [ ] Null test against the pre-ticket render passes: difference peak ≤ −100
      dBFS. Transposing the accumulators reorders float additions, so the render
      hash will very likely move and that alone is not a regression — but this
      is a pure *layout* change, so anything audible is a bug rather than a
      re-baseline.
- [ ] `cargo test --workspace` green.

## Notes

- **This is an optimisation with no user-visible payoff on its own** — its
  priority comes from being on E049's critical path, not from the cycles.
  Context for sizing it: at engine level the matrix is
  already noise — `matrix_gated` (full render) shows no measurable difference between a
  baseline table and one with routes live, all three cases sitting at ~402µs.
  The eval is ~125ns per stack per control block; against a 1.33ms control
  block at 48kHz that's well under a percent even at 16 stacks. Do this for the
  layout coherence with `PitchSmoother` as much as for the cycles.
- Measure before believing. Three of the four perf guesses on this code were
  wrong last time round — see [[vxn2-matrix-hot-loop-lessons]] (`clamp` carries
  a panic path; `copysign` lost to the branch it replaced) and
  [[vxn1-tanh-branchless-only]].
- Related but distinct: [[vxn1-soa-match-defeats-simd]] is about a runtime match
  in the lane body defeating SIMD. That problem is already fixed here — the
  dispatch is hoisted into per-slot macro arms. This ticket is purely about
  memory layout.
- Per-crate asm is misleading pre-LTO ([[vxn1-ota-filter-perf]]). `eval_dests`
  is `#[inline]`, so it emits no standalone symbol; the 2026-08-30 measurement
  used a temporary `#[no_mangle]` wrapper to force one, then removed it.
- Out of scope: changing what the matrix computes, the slot roster, or the
  `(polarity, shape)` curve encoding. Bit-identical output is the constraint.

## Close-out (2026-08-31)

- `LaneDestVals` is `[[f32; STACK_LANES]; N_DESTS]`
  ([matrix.rs:801](../../vxn-2/crates/vxn2-engine/src/matrix.rs#L801)) and
  `eval_dests` writes it contiguously across lanes.
- **`LaneSourceVals` was flipped too**, to `[[f32; STACK_LANES]; N_SOURCES]`
  ([matrix.rs:793](../../vxn-2/crates/vxn2-engine/src/matrix.rs#L793)), rather
  than left lane-major as the Design section suggested. Two reasons:
  `vxn_core_matrix::storage::SourceLanes` is already source-major and its own doc
  names this ticket as where vxn-2 converges, so
  [0334](0334-share-the-evaluator.md) needs both halves; and the accumulate only
  reaches 4-wide with both rows contiguous.
- **4-wide arithmetic in the route accumulate: confirmed.** Post-LTO
  `llvm-objdump` on the linked `matrix` bench binary, via a temporary
  `#[no_mangle]` wrapper removed before commit. Instructions 895 → 569, scalar FP
  ops 383 → 112, `.4s` ops 23 → 74, `.2s` ops 32 → 84. The `.4s` ops are no
  longer confined to the scale-VCA loop: the curve arms carry `fabs.4s` /
  `fneg.4s` / `fcmlt.4s` (from `pol_abs`, `shape_exp`, `shape_log`), with the
  accumulate itself as `ldr q3` → `fmul.4s` → `fadd.4s` → `stur q1`. Homebrew
  LLVM 22 was used — the rustup toolchain here has no `llvm-tools` component.
- **Both named benchmarks improved**, matched pair measured back-to-back in a
  serialised window, all p = 0.00: `matrix_eval_full` 110.70 ns → **91.482 ns**
  (−18.0%), `matrix_eval_scaled` 124.02 ns → **110.32 ns** (−10.9%),
  `matrix_eval_empty` 27.272 ns → **21.976 ns** (−19.4%). No "measured, not worth
  it" fallback needed.
  - Honest attribution: `matrix_eval_empty` has no route accumulate, so its
    −5.30 ns is the source-major rewrite of `eval_sources` (11 row splats
    replacing 88 scalar stores) plus the changed zeroing loop. Subtracting that
    floor, the accumulate itself accounts for ≈ −13.9 ns of `full`'s −19.22 ns
    and ≈ −8.4 ns of `scaled`'s −13.70 ns. `scaled` improves less in absolute
    terms because its scale-VCA loop already walked a contiguous local and was
    already vectorised; only its `sources[sc]` read got better.
  - Measurement note for anyone re-running this: run the three cases in separate
    criterion processes. Sharing one process widened the CIs badly (`scaled`
    spanned 126–151 ns) — that was the noise source, not machine load.
- `PitchSmoother::targets_from` is gone, along with the engine's `pitch_targets`
  capture buffer. `tick` / `snap_to` / `converged` index `dest_vals` directly
  through a new `PITCH_DEST_ROWS` const
  ([matrix.rs:1519](../../vxn-2/crates/vxn2-engine/src/matrix.rs#L1519)); `Engine::reset`
  uses a new `PitchSmoother::clear()`. Only doc references to the old name remain.
- **Bit-exact, not merely within tolerance.** Accumulation order across slots is
  untouched, so both synths' null tests report `-inf dBFS` — the epic had
  budgeted for a hash move here and did not get one. Identical samples mean the
  render hashes cannot have moved, so no CI re-capture is needed.
- Consumer reads were re-derived rather than blind-swapped: stages 5b/6/10 hoist
  a row reference out of the lane loop, level and pan become whole-row copies,
  and `scatter_stack_pitch` copies the source row once per component.
- `cargo test --workspace` green on the merged tree: 91 test binaries, 0
  failures. Code review (high effort) found 4 issues, all in the test module and
  all fixed — two index-out-of-bounds panics where a transpose regex missed
  `sources[1][si]` / `sources[2][si]`, one assertion that would have gone
  silently dead, one overreaching doc claim. It explicitly cleared the
  `scatter_stack_pitch` zero-skip removal and the `pitch_targets` / `reset`
  equivalence.
- **Follow-up left open:** the module doc's inner-loop note still cites
  "`matrix_eval_full` is unchanged at ~111 ns across the split" and "253 ns → 133
  ns" for the earlier scale hoist. Both are correctly-dated historical
  observations rather than false claims, but "~111 ns" now sits beside code that
  runs at 91.5 ns and reads as current. Comment-only refresh, not done here.
- Landed via `e049/0328-dest-major` @ `fe4dc91`, merged to `main` in `c58167c`.
  That merge resolved four take-both conflicts against
  [0330](0330-share-curve-vocabulary.md)'s `PolarityKind`/`ShapeKind` rename.
