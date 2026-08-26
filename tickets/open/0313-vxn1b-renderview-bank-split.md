---
id: "0313"
product: vxn-1b
title: "RenderBank::render — 452 lines, 11 args, five consecutive &[f32] that transpose silently"
priority: high
created: 2026-08-26
epic: E047
depends: ["0321"]
---

## Summary

[`RenderBank::render`](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L831) is the
largest function in the product by a wide margin — **452 lines, 11 parameters,
seven levels of nesting** — and its signature carries a live hazard.

### The hazard

[bank.rs:833-843](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L833-L843) takes
five consecutive `&[f32]` with no type distinction: `velocity`, `pressure`,
`note_random`, `detune_cents`, `stack_pos`. Swap any two and it compiles, runs,
and produces *plausible* audio — velocity landing where stack position was
expected does not crash, it just sounds subtly wrong on stacked notes.

A `RenderView` struct holding exactly these eight fields **already exists** at
[voice.rs:810](../../vxn-1b/crates/vxn1b-engine/src/voice.rs#L810), and
[synth.rs:306-318](../../vxn-1b/crates/vxn1b-engine/src/synth.rs#L306-L318)
unpacks it *positionally* to make the call. The safe thing is already built and
is being deliberately taken apart at the call site.

The same shape recurs at
[`lane_sources`](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L683) — 8 params
including 4 `&[f32]`, carrying `#[allow(clippy::too_many_arguments)]`.

### `BlockCtx` encodes one enum four times

[bank.rs:186-241](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L186-L241) has 39
public fields. Four of them are the same fact: `sync: bool`, `pm_index: f32`,
`ring_mode: bool` are all derived from `cross_mod_type` in
[synth.rs:362-367](../../vxn-1b/crates/vxn1b-engine/src/synth.rs#L362-L367) —
and `cross_mod_type` is passed as well. Nothing prevents constructing
`sync: true, cross_mod_type: Off`. `render` then **re-derives them anyway**
(`pm_mode` at [:907](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L907),
`ring_on` at [:1084](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L1084)), so
the three fields are not even saving the work they exist to save.

### The length

The seams are already in the file as banner comments: *"Phase 1: bank-wide block
setup"*, *"Phase 2: per-lane block-start resolution"*, *"Phase 3: bank-wide
decisions"*, *"Phase 4: the frame loop"*. Somebody wrote the extraction plan and
did not execute it.

## Design

Three steps, in this order, each independently revertible:

**1. Pass the view.** Change `RenderView`'s fields from `&'a [T; N]` to
`&'a [T]`, add `RenderView::chunk(b, lanes)`, and make the signature
`render(&mut self, ctx, view, out_l, out_r)`. This alone kills the transposition
hazard, fixes `lane_sources` by extension, and **deletes ~200 lines of test
boilerplate** — the test module has 18 duplicated 11-argument call sites.

**2. Derive the cross-mod fields.** Drop `sync` / `ring_mode` / `pm_index` from
`BlockCtx`, keep `cross_mod_type` + `cross_mod_amount`, derive inside `render`
where it already happens. 39 fields → 36, and an unrepresentable state becomes
unrepresentable.

**3. Split at the banners.** Extract Phase 2 as
`resolve_lanes(&mut self, ctx, book) -> LaneBlock` and Phase 3 as
`block_plan(&LaneBlock, ctx) -> BlockPlan`, both returning plain records. Phase
4 — the frame loop — **stays inline**; it is the poly hot path and this ticket
must not touch its codegen.

Step 1 makes step 3 mechanical, which is why it goes first.

## Step 2 was tried and rejected (2026-08-26)

Deriving `sync` / `ring_mode` / `pm_index` inside `render` and slimming
`BlockCtx` to `cross_mod_type` + `cross_mod_amount` **costs ~1.3% on the routed
path** and is not worth it.

Measured by interleaving two separately-built `route_profile` binaries — the
machine had a second session on it, and sequential sampling was reading ±4% of
pure noise:

| | before | after | delta |
|---|---|---|---|
| `route_profile`, 13 pairs | 49.38× | 48.72× | **−1.32%** |
| `busy_profile`, 5 pairs | 15.86× | 15.86× | 0.00% |

11 of 13 route pairs negative — not noise-shaped, and concentrated exactly where
cross-mod work happens while the plain poly path is untouched. Most likely
`BlockCtx` layout: removing two `bool`s and an `f32` shifts every later field's
offset, and the frame loop reads a lot of `ctx`. Chasing that is the rabbit hole
this ticket warns about, and the prize was only hygiene — the invalid
`sync: true, cross_mod_type: Off` state, which nothing constructs.

Kept from the attempt: `build_ctx` now reads `cross_mod_type()` **once** instead
of twice (it is a get + round + min + from_index), which was finding #20 of the
review and is free.

If someone retries this, the bar is a measurable win or parity — not "it is
tidier". Same lesson as [[vxn1-ota-filter-perf]]'s stage-split.

## Acceptance criteria

- [ ] `render` takes a `RenderView`; no caller unpacks it positionally.
- [ ] `lane_sources`'s `#[allow(clippy::too_many_arguments)]` is gone, not
      suppressed differently.
- [ ] `BlockCtx` no longer carries state derivable from `cross_mod_type`.
- [ ] `render`'s body is under ~150 lines with the frame loop intact.
- [ ] **No codegen regression in the frame loop.** Run `busy_profile` and
      `route_profile` before and after and record both numbers in the close-out.
      The bar is the one from [[vxn1-render-loop-optimized]] — dry_4x ~51× RT,
      sync_4x ~41× RT, idle ~1100× RT. A drop means step 3 went too far.
- [ ] `zipper_regression`, `parity`, `cross_mod_dest`, `taper_parity` and
      `oversampling_limiter` all green — these are the tests that would catch a
      transposed slice if one is introduced while removing the ability to
      introduce one.
- [ ] One manual DAW pass — [[verify-audio-in-reaper]] — with a stacked,
      detuned, cross-modulated patch, since that exercises every slice this
      ticket reorders.

## Notes

- This is the one ticket in [[E047]] that touches the poly hot path. Everything
  about it is arranged so that the risky part (step 3) is last and separately
  revertible from the valuable part (step 1).
- Watch for the SoA/NEON trap while splitting: a runtime `match` moved *into* a
  lane loop drops NEON to scalar ([[vxn1-soa-match-defeats-simd]]), and ARM64
  `llvm-objdump` puts `.4s` on the mnemonic, so the obvious grep for
  vectorisation returns zero matches on vectorised code
  ([[vxn1-neon-grep-pitfall]]). If in doubt, dump the asm rather than trusting
  the profile alone.
- No `cargo fmt` — [[vxn-no-cargo-fmt]]. One `cargo test` at a time —
  [[vxn-no-parallel-cargo-test]].
