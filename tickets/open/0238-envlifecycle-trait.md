---
id: "0238"
product: monorepo
title: "EnvLifecycle trait — shared envelope lifecycle, impls in place, AdsrCore adapter"
priority: medium
created: 2026-08-02
epic: E044
depends: ["0222"]
---

## Summary

First ticket of [E044](../../epics/open/E044-envelope-lifecycle-swap-readiness.md).
Four envelope families share one lifecycle with different param shapes. Name
it without moving numerics:

```rust
pub trait EnvLifecycle {
    type Params;
    fn cook(&mut self, p: &Self::Params, rate_mult: f32);
    fn scale_rates(&mut self, scale: f32);
    fn note_on(&mut self);
    fn note_off(&mut self);
    fn tick(&mut self, dt: f32) -> f32;
    fn is_idle(&self) -> bool;
}
```

- In-place impls: `EgState` / `PitchEgState` / `ModEnvState`
  ([eg.rs](../../vxn-2/crates/vxn2-dsp/src/eg.rs),
  [envelope.rs](../../vxn-2/crates/vxn2-dsp/src/envelope.rs)) — already
  exactly this shape, mechanical.
- Adapter impl for `AdsrCore`
  ([adsr.rs:132](../../vxn-1b/crates/vxn-dsp/src/adsr.rs#L132),
  `tick(triggered, gate_high)`): `note_on` latches triggered, `note_off`
  drops gate_high, `tick(dt)` ignores dt (fixed-fs core) —
  semantic-preserving.

## Acceptance criteria

- [ ] Trait in `vxn-core-dsp::env`; four impls compile; the marchers
      (`eg.rs`'s log-domain march, `AdsrCore`'s curves) do NOT move.
- [ ] Adapter-equivalence unit tests: `AdsrCore` via trait vs direct calls,
      bit-exact over full ADSR cycles incl. retrigger.
- [ ] All goldens untouched (nothing rewired in engines yet — trait adoption
      by render loops is future work when a consumer needs it).

## Notes

Per-op EG stays self-contained for budget reasons (`envelope.rs:3-6`) — the
trait names the boundary, it does not unify the marcher. The
"params-changed" cook boundary matters: vxn-1's `EnvSnapshot` caching exists
to avoid `set_params` exp() cost; the trait's `cook` is that boundary made
explicit.
