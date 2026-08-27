# ADR 0002 — vxn-core-dsp: the shared DSP component layer

- **Status:** Accepted
- **Date:** 2026-08-27
- **Scope:** Where DSP code lives now that the repo ships four synths.
  Supersedes [ADR 0001](0001-vxn-core-split.md) §2 **for the component layer
  only** — the rest of §2's not-extracted list stands unchanged. Companion to
  epic [E040](../epics/open/E040-vxn-core-dsp-foundations.md).

## Context

ADR 0001 §2 kept DSP primitives synth-local, and gave the condition for
revisiting: *"Revisit if a third synth shows up."* That condition is met three
times over — vxn-1b (matrix-modulation variant of vxn-1, reusing `vxn-dsp`
verbatim), vxn-3 (drum machine, an FX consumer), and vxn-2's FX chain all now
want the same blocks.

§2's reasoning was about **signal models**: vxn-1's analogue osc + OTA ladder +
ADSR and vxn-2's FM operator + 4R/4L EG + SoA stack are genuinely different
numerical contracts (Q32 phase vs f32 phase, SoA stack vs scalar voice), and a
shared "DSP toolbox" would have forced one to compromise. That reasoning was
correct and remains correct.

What it did not anticipate is that the *components downstream of the voice* —
the FX chain, the dynamics block, the HPF, the halfband pair — are not
signal-model-specific at all. They took the fork anyway, by copy-paste:

- `DynamicsBlock` — vxn-1's header literally says "Ported verbatim from vxn-2";
  the measured diff is import path + test helpers, kernel byte-identical.
- `HpfKernel` — TPT one-pole, effectively identical forks.
- The scalar OTA ladder, phaser, chorus, reverb and delay — forks with
  mechanical deltas (mix law, where the bypass fade lives, interpolation order).
- `raised_cosine_rise` — the same crossfade weight written out three times.
- The Q32 phase convention — triplicated inside vxn-2 alone.

Duplication that was cheap at two consumers is not cheap at four, and the copies
have already started to drift in ways nobody chose.

## Decision

### 1. The boundary rule

Three layers, and a piece of code belongs to exactly one:

| Layer | Home | Test |
|---|---|---|
| **Leaf utils** | `vxn-core-utils` | A free function or a plain-data struct. No sample rate, no lifecycle. |
| **Components** | `crates/vxn-core-dsp` | Has a `Params` struct, a sample-rate constructor, **or** an enable/declick lifecycle. |
| **Hot voice kernels** | per-synth | SoA lane loops, allocator-adjacent, signal-model-specific. |

The middle row is what this ADR adds. The rule is deliberately mechanical —
"does it have a `Params` struct, a sample-rate constructor, or a declick
lifecycle" is answerable by reading the type, not by judgement — because the
failure mode we are guarding against is a *plausible* argument for moving
something hot.

### 2. The boundary test

Inherited from ADR 0001 §6 and sharpened. Every shared trait must be
implementable by all consumers **without**:

- a fake parameter — one the synth does not actually have and passes a dummy for,
- a per-block `Box<dyn>` or any allocation on the audio thread,
- a signal-model compromise — changing what the synth *sounds like* to fit.

If an extraction needs any of the three, the abstraction is wrong. Leave the
duplication and record why.

### 3. Nothing SoA-hot moves

`poly/oscillator.rs`, `poly/ladder.rs` bodies and `stack.rs` stay per-synth.
Only the consts and coefficient types they *import* move. This is not a
performance hedge — it is the same signal-model argument ADR 0001 §2 made, and
it still holds for the lane loops even though it no longer holds for the FX
chain.

### 4. SIMD protection regime

Extraction moves code across a crate boundary, and crate boundaries are where
inlining decisions change. The regime for shared DSP:

- Plain `#[inline]` on anything in a sample loop. No `#[inline(always)]`
  cargo-culting, but no bare cross-crate calls in the hot path either.
- **No `dyn`, no enum-match inside sample loops.** Runtime choices resolve once
  at a block edge, to a marker type or an fn-ptr table
  ([[vxn1-soa-match-defeats-simd]] is the war story: a runtime enum match inside
  a poly lane loop dropped NEON to scalar).
- Fat LTO erases the crate boundary in release builds — but *thin* LTO is what
  `[profile.release]` actually sets, so this is a claim to be **verified, not
  assumed**. Ticket 0223's asm-check harness exists for exactly this: NEON
  operand counts per named hot symbol, compared against a recorded pre-extraction
  reference.
- Toolchain stays pinned at 1.95.0. The goldens are codegen-sensitive.

Note when reading asm: ARM64 `llvm-objdump` puts the `.4s` suffix on the
mnemonic, not the operands ([[vxn1-neon-grep-pitfall]]) — a naive
`grep 'v\d+\.4s'` reports zero matches on fully-vectorised code.

### 5. Locked decisions

Settled during E040 planning, recorded so they are not relitigated per-ticket:

- **Declick unifies on the vxn-2 idiom.** vxn-2's `WetFade` (internal fade, wet
  path only) becomes the shared enable/disable mechanism; vxn-1's outer
  `BypassXfade` around the whole FX is retired *for per-FX enables*. It survives
  as the primitive for **whole-span** switches — vxn-1's oversample-change
  crossfade, vxn-2's span fades — which are a different thing wearing a similar
  shape.
- **Re-baselining is allowed, in flagged commits only.** Most of E040 is
  bit-exact and the goldens must not move. Where a later epic deliberately
  changes a mix law (0230's reverb, 0231's delay), the commit is marked
  `REBASELINE` in its subject and carries the A/B rationale in its body. A
  golden that moves in an unflagged commit is a bug, not a new baseline — fix
  it, never recapture it.
- **Design for runtime swapping, wire nothing user-facing.** Oscillator, envelope
  curve and filter-model swaps are plausible future features. The dispatch
  pattern is documented now (0239) so that when one lands it has a recipe that
  provably keeps dispatch out of lane loops. No swap params ship as part of this.
- **Delays unify on a vxn-2-superset kernel** with optional feedback damping,
  rather than two kernels or a lowest-common-denominator one.

### 6. Not extracted

Stays per-synth, and the decision is recorded so it is not relitigated:

- **`tick_ops` / cook order.** The per-synth order in which parameters are
  cooked into coefficients is the synth's identity, not a shared concern.
- **Voice allocators.** Unchanged from ADR 0001 §2 — vxn-1's dual-layer
  `VoiceBank` and vxn-2's SoA `PolyAlloc` with first-class stacking have
  different shapes.
- **Mod routing.** vxn-1 fixed routes, vxn-1b and vxn-2 generic matrices,
  vxn-3 macro bindings. Four topologies, no shared abstraction that earns its
  keep.
- **Smoothing policies.** The *primitive* is shared (`Smoothed`, and 0237's
  `BlockRamp`/`CoeffRamp`); *which* params smooth at which rate is per-synth.
- **The `OsSpan` FSM.** vxn-2's oversampled-span state machine stays in vxn-2;
  0233 extracts its mechanics (`OsRegion`, `SpanDelay`), not its policy.
- **vxn-3's send delay.** Single consumer, and its semantics are pattern-clocked
  rather than time-clocked.
- Everything in ADR 0001 §2 not explicitly moved by this ADR — notably
  `SharedParams` and the param tables.

### 7. Crate shape

`crates/vxn-core-dsp`: `[lib]` rlib, dependencies **`vxn-core-utils` only**. It
must never depend on a synth crate, and no synth crate may depend on it until a
ticket explicitly repoints one — 0222 is a pure addition.

Modules: `control`, `declick`, `fx`, `env`, `os_region`, `test_util`.

Extraction preserves module paths on the consumer side: `vxn-dsp` and
`vxn2-dsp` keep their `dynamics`/`hpf`/etc. modules as **re-export shims**,
following the established `vxn2-dsp/src/smoother.rs` pattern. Call sites do not
churn.

## Consequences

**Positive:**

- One place to fix an FX bug, for four synths. Today a phaser fix has to be
  applied two or three times and provably is not.
- vxn-3 and any future synth get the FX chain, dynamics and HPF for the cost of
  a `Params` mapping.
- The forks stop drifting. Several already have, silently.
- The asm-check harness (0223) is reusable well past this epic — it makes
  "vectorisation unchanged" a checkable claim rather than a hopeful comment.

**Negative:**

- Every extraction is a chance to perturb floating-point order, and four
  products' goldens are watching. This is why E040 is scoped to pure moves and
  why 0223 lands before any kernel does.
- A shared crate makes it *easier* to make a change that helps one synth and
  quietly costs another. The boundary test in §2 and the REBASELINE discipline
  in §5 are the mitigations; neither is automatic.
- Cross-crate inlining is a real risk under thin LTO, and the mitigation is a
  measurement harness rather than a guarantee.

**Neutral:**

- ADR 0001 §2 is not retracted. Its reasoning about signal models was right;
  this ADR narrows its *scope*, and §6's rule ("duplication is the default,
  extract only when both sides improve together") is the rule this ADR applies
  rather than one it replaces.

## Out of scope

- Moving any SoA voice kernel. See §3.
- Changing what any synth sounds like. E040 is bit-exact by construction;
  deliberate sonic changes belong to E041 and carry the REBASELINE flag.
- Publishing `vxn-core-dsp` to crates.io.
- Shipping any runtime-swap parameter. See §5.

## References

- [ADR 0001 — vxn-core-* shared crate split](0001-vxn-core-split.md), §2 and §6
- Epic [E040 — vxn-core-dsp foundations](../epics/open/E040-vxn-core-dsp-foundations.md)
- Epics [E041](../epics/open/E041-shared-fx-unification.md) (FX unification),
  [E042](../epics/open/E042-oversampled-region.md) (oversampled region),
  [E043](../epics/open/E043-param-schema-control-vocabulary.md) (control
  vocabulary), [E044](../epics/open/E044-envelope-lifecycle-swap-readiness.md)
  (envelope lifecycle)
- vxn-2 ADR 0001 — the divergent DSP/voicing decisions behind §3 and §6
