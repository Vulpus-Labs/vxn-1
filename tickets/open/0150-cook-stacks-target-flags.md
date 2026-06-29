---
id: "0150"
product: vxn-2
title: cook_stacks_block TargetFlags — group the four bare bool flags
priority: low
created: 2026-06-28
epic: E029
---

## Summary

`vxn-2/crates/vxn2-engine/src/engine.rs` `cook_stacks_block`
takes eight args, four of which are positional booleans —
`lfo2_rate_targeted`, `stack_detune_targeted`,
`stack_spread_targeted`, `stack_pitch_targeted` — and carries
an `#[allow(clippy::too_many_arguments)]`. Four bare bools at
a call site is the unlabeled-positional hazard the lint exists
to flag (a transposed pair is silent). Group them into a
named struct built once in `process_block`.

Behaviour-preserving — block-rate plumbing only, no audio
render change.

## Proposed shape

```rust
struct TargetFlags {
    lfo2_rate: bool,
    stack_detune: bool,
    stack_spread: bool,
    stack_pitch: bool,
}
```

`process_block` builds one `TargetFlags` before the loop;
`cook_stacks_block(n, dt, filter_enabled, flags, patch_sources)`
drops to five args. Field accesses inside the body
(`flags.stack_detune` etc.) read clearer than the bare bools.

## Acceptance criteria

- [ ] `TargetFlags` struct exists; `cook_stacks_block`
      consumes it and the `#[allow(clippy::too_many_arguments)]`
      is removed.
- [ ] The superseded "grouping them buys no clarity over named
      args" comment above the fn is removed or rewritten to
      match.
- [ ] `cargo clippy -p vxn2-engine` clean.
- [ ] `cargo test --workspace` green; vxn-2 `tests/baseline.rs`
      render hash unchanged (behaviour-preserving).

## Notes

The existing comment argued grouping buys no clarity; this
ticket disagrees specifically for the four-bool subset (named
fields beat four positional bools at the call site) and leaves
the non-bool args (`n`, `dt`, `filter_enabled`,
`patch_sources`) flat. No stage reordering, no value change —
the 12-stage `cook_stacks_block` ordering is preserved exactly
(see the stage-ordering doc table above the fn).
