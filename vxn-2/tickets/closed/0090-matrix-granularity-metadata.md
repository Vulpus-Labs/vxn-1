---
id: "0090"
title: "Granularity-tier metadata + coherence predicate + descriptor export"
priority: high
created: 2026-06-12
epic: E008
depends: []
---

## Summary

First ticket of [E008](../../epics/open/E008-mod-matrix-completeness.md). Encode
the **granularity tier** of every matrix source and destination, add a
`coherent(src, dst)` predicate that implements the epic's coherence rule, and
export both into the matrix descriptor the UI reads. This is metadata + a pure
function — no audio behaviour changes here. It is the shared source of truth for
the validator ([0095](0095-matrix-row-coherence-validation.md)) and the docs,
and pins down which sources each newly-wired dest (0091-0093) should honour.

## Design

Tiers in [matrix.rs](../../crates/vxn2-engine/src/matrix.rs):

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Tier { PatchGlobal = 0, PerStack = 1, PerLane = 2 } // coarse → fine
```

- `SourceId::tier()`:
  - `Lfo1, ModWheel, Aftertouch` → `PatchGlobal`
  - `PitchEg, ModEnv, Velocity, Key` → `PerStack`
  - `Lfo2, VoiceIdx, VoiceSpread, VoiceRand` → `PerLane`
- `DestId::tier()`:
  - `Lfo1Rate, DelayMix, ReverbMix` → `PatchGlobal`
  - `Lfo2Rate, StackDetune, StackSpread, Cutoff, Resonance` → `PerStack`
  - `Op*{Pitch,Level,Pan}, GlobalPitch, Feedback, Lfo2Phase` → `PerLane`

Coherence predicate (epic rule: source coarser-or-equal to dest, minus
special-cases):

```rust
/// Why a routing is degenerate/incoherent, or `Ok` if sound.
pub enum Coherence {
    Ok,
    /// Finer source into a coarser dest: lossy collapse.
    TierCollapse,
    /// LFO modulating its own rate.
    SelfRate,
    /// Tier-legal but the lane-0 read makes it a constant (voice-idx).
    Degenerate,
}

pub fn coherence(src: SourceId, dst: DestId) -> Coherence;
```

- `TierCollapse` when `(src.tier() as u8) > (dst.tier() as u8)`.
- `SelfRate` for `(Lfo1, Lfo1Rate)` and `(Lfo2, Lfo2Rate)`.
- `Degenerate` for `VoiceIdx` into any dest read at lane 0 (`Cutoff`,
  `Resonance`, `DelayMix`, `ReverbMix`) — `voice_idx[0]` is always 0
  ([stack.rs:598](../../crates/vxn2-dsp/src/stack.rs#L598)).
- `None` source / `None` dest → `Ok` (empty slot, nothing to flag).

Descriptor export (the JSON the UI hydrates as `window.__vxn.matrix`): add a
`tier` field to each entry of `sources` and `dests`, and a flat
`coherence[srcId][dstId]` lookup (or expose the predicate's result table) so the
UI doesn't reimplement the rule. Find the descriptor builder that currently
emits `sources`/`dests`/`curves` (grep `matrix.sources` producers in
`vxn2-ui-web` / the controller bridge) and append `tier` + the table.

Document the tier table + rule in `PARAMETERS.md` (matrix section) and the
[matrix.rs](../../crates/vxn2-engine/src/matrix.rs) module doc, replacing the
ad-hoc "Audio wiring status" prose with the tier framing.

## Acceptance criteria

- [x] `Tier` enum + `SourceId::tier()` / `DestId::tier()` cover all 11 sources
  and 29 dests; exhaustive `match` (no `_` arm) so a future source/dest forces a
  tier decision at compile time.
- [x] `coherence(src, dst)` returns the documented variant for every pair; unit
  test walks the full 12×30 grid (incl. `None`) and asserts the rule + the three
  special cases (tier-collapse, self-rate, degenerate).
- [x] Descriptor exposes `tier` on every source/dest entry and a coherence
  lookup; a `vxn2-ui-web` test asserts the hydrated `window.__vxn.matrix` carries
  tiers and that e.g. `voice-rand → lfo2-rate` reads `TierCollapse`,
  `voice-rand → lfo2-phase` reads `Ok`.
- [x] No audio path touched — `process_block` output bit-identical for all
  factory patches (this ticket is metadata only). Existing engine matrix tests
  pass unchanged; no edits to `eval_dests` / process path.
- [x] `PARAMETERS.md` + matrix module doc carry the tier table and the
  coherence rule.

## Notes

This ticket deliberately ships no behaviour change so it can land first and
unblock both the validator and the wiring tickets in parallel. The predicate is
the *canonical* definition — 0095 must consume the exported table, not
re-derive the rule in JS, so the two never drift.
