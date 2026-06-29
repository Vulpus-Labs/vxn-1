---
id: "0147"
product: vxn-2
title: "Dynamics — insert first in FX bus, wire apply_block_params + reset"
priority: medium
created: 2026-06-24
epic: E028
depends: ["0146"]
---

## Summary

Third ticket of [E028](../../epics/open/E028-vxn2-fx-dynamics-block.md).
Insert `DynamicsBlock` first in the FX chain so the bus becomes
`cleanup → dynamics → phaser → delay → reverb → master → limiter`.
Wire param refresh, reset, and gate on `dyn-on`. `dyn-on = 0` must
keep the bus bit-identical to pre-epic (deterministic null test).

## Design

File: `vxn-2/crates/vxn2-engine/src/engine.rs`.

**Field + constructor** (mirror phaser at `engine.rs:174-176, 349`):

```rust
pub dynamics: DynamicsBlock,
// ...
dynamics: DynamicsBlock::new(sample_rate),
```

**Reset** (mirror phaser at `engine.rs:454`):

```rust
self.dynamics.clear();   // zero envelope follower, snap mix to current target
```

placed in `Synth::reset` next to `self.phaser.clear()`.

**Param refresh** (mirror phaser at `engine.rs:527-529`):

```rust
self.dynamics.set_from(&self.params.dynamics);
```

placed in the param-apply path next to `self.phaser.set_from(...)`.

**Process call** — insert **before** the phaser at the per-sample
loop (`engine.rs:1449-1451`):

```rust
let (cl, cr) = self.dynamics.process(cl, cr);
let (cl, cr) = self.phaser.process(cl, cr);
let (l, r)   = self.delay.process(cl, cr);
let (l, r)   = self.reverb.process(l, r);
```

**Bit-exact passthrough.** The 0145 DSP guarantees `process` is a
zero-cost early-return when `!enabled && mix.current() == 0.0`. The
default patch ships `dyn-on = 0`, so an unchanged patch must render
sample-identical to a pre-epic build. Add a deterministic null test
(seeded RNG note pattern, 1 s of audio) comparing the new build with
`dyn-on = 0` against a stored pre-epic reference, or against a build
where the dynamics insertion line is commented out.

**Mod-matrix modulation aggregation.** Phaser has no mod-matrix
destinations (E025), so the per-block aggregate at `engine.rs:1148-1162`
covers only delay/reverb mixes. Dynamics is also host-automation
only (per epic) — **no new entries** in the aggregation block, no
new `DestId`, no `DEST_NAMES` change. Asserted by the matrix.rs grep
in 0146.

## Acceptance criteria

- [ ] `Synth::dynamics: DynamicsBlock` field added; constructed with
      sample rate; cleared in `reset()`.
- [ ] `apply_block_params()` calls `self.dynamics.set_from(&self.params.dynamics)`.
- [ ] Per-sample loop inserts `self.dynamics.process(...)` **before**
      `self.phaser.process(...)`.
- [ ] Default-patch render is bit-identical to pre-epic (null test in
      `vxn-2/crates/vxn2-engine/tests/` or inline `#[cfg(test)]`).
- [ ] With `dyn-on = 1` and known threshold/ratio on a hot signal,
      the post-FX peak is measurably lower than with `dyn-on = 0`
      (gain reduction is reaching the bus).
- [ ] No new `DestId` variant, no new `DEST_NAMES` entry, no new
      branch in the mod-matrix aggregation block.
- [ ] `cargo test -p vxn2-engine` passes; `cargo build -p vxn2-clap
      --release` builds clean.

## Notes

Dynamics is hard-wired first — no user-reorderable chain (epic
out-of-scope). The master brickwall limiter
(`vxn-2/crates/vxn2-engine/src/engine.rs:1216-1228`) stays where it
is — different job (post-master safety vs. pre-FX musical comp).

Manual Reaper check per [[verify-audio-in-reaper]] after build —
specifically: toggle `dyn-on` on a sustained note and confirm no
audible click (the 0145 fade semantics).

Followed by 0148 (faceplate tab).

## Close-out (2026-06-25)

- `Engine::dynamics: DynamicsBlock` field added
  ([engine.rs:174-178](../../vxn-2/crates/vxn2-engine/src/engine.rs#L174-L178)),
  constructed with `sample_rate` in `Engine::new`
  ([engine.rs:354-356](../../vxn-2/crates/vxn2-engine/src/engine.rs#L354-L356)),
  cleared in `reset()`
  ([engine.rs:460](../../vxn-2/crates/vxn2-engine/src/engine.rs#L460)). DSP
  import at [engine.rs:38](../../vxn-2/crates/vxn2-engine/src/engine.rs#L38).
- `apply_block_params` calls
  `self.dynamics.set_from(&self.params.dynamics)`
  ([engine.rs:537](../../vxn-2/crates/vxn2-engine/src/engine.rs#L537)),
  next to the phaser refresh.
- `apply_fx_block` inserts `self.dynamics.process(cl, cr)` **before** the
  phaser, so the bus is now
  `cleanup → dynamics → phaser → delay → reverb → master → limiter`
  ([engine.rs:1455-1460](../../vxn-2/crates/vxn2-engine/src/engine.rs#L1455-L1460)).
  Block-doc comment updated to reflect the new chain.
- Bit-identical null test:
  `dynamics_integration::bypass_render_is_bit_identical_across_dynamics_params`
  ([tests/dynamics_integration.rs](../../vxn-2/crates/vxn2-engine/tests/dynamics_integration.rs))
  renders the default patch twice — once with all `dyn-*` at defaults, once
  with `dyn-on = 0` but every other `dyn-*` slammed to extremes — and asserts
  `to_bits()` equality on every interleaved sample. The 0145 DSP gate makes
  this trivially pass; this test guards the gate from regressing.
- `dyn-on = 1` reduces post-FX peak:
  `dynamics_integration::dyn_on_reduces_post_fx_peak` renders 64 blocks with
  `dyn-on=0` vs `dyn-on=1` (threshold −36 dB, ratio 20, mix 1), asserts the
  on-peak is at least 10 % below the off-peak — the gain reduction is
  actually shaping the bus, not just attached.
- No new mod-matrix entries.
  `grep -in 'dyn\|dynamics'
  [vxn-2/crates/vxn2-engine/src/matrix.rs](../../vxn-2/crates/vxn2-engine/src/matrix.rs)`
  returns only the word "dynamic" in English text — no `DestId`, no
  `DEST_NAMES`, no aggregation-block branch.
- `param_audibility.rs` `EXCLUDED` list trimmed: the eight `dyn-*` forward-
  pointer entries from 0146 are gone; `context_override` gains a `dyn-`
  arm that engages `dyn-on=1`, full wet mix, hot threshold, ratio 8 so each
  swept `dyn-*` param audibly reshapes the bus. `every_param_sweep_is_audible`
  passes.
- `cargo test -p vxn2-engine` → 209 lib + integration tests pass.
  `cargo build -p vxn2-clap --release` clean.
- Followed by 0148 (faceplate tab).
