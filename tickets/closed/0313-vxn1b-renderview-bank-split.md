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

## Close-out (2026-08-26)

Closed with **step 1 shipped and steps 2–3 declined on measurement**. The
defect this ticket existed for — a signature where transposing two of five
consecutive `&[f32]` compiled, ran, and produced plausible-but-wrong audio — is
gone. What remains was readability, and the numbers said no.

### Shipped

- **`render` takes a `LaneView`** (6ae87b7): 11 positional arguments → 4.
  [`LaneView`](../../vxn-1b/crates/vxn1b-engine/src/voice.rs#L823) is one bank's
  window onto the `Voices` arrays; `RenderView::banks(lanes)` yields them,
  consuming the view because `active` is `&mut` and each window takes a disjoint
  piece. `render` destructures in one line, so the 450-line body is untouched.
  The call site in `synth.rs` went from a 15-line indexed slicing loop to three
  lines — it had been taking a `RenderView` apart positionally to make the call,
  so the safe thing already existed and was being deliberately unbuilt.

- **`lane_sources` lost its `#[allow(clippy::too_many_arguments)]`**: 8 params →
  4, via a `SourceLanes` record grouping the four same-typed slices, built once
  outside the lane loop. (The step-1 commit claimed this fell out "by extension";
  it did not — caught while verifying acceptance for this close-out.)
  Measured free: busy +0.31%, route −0.07%.

- **`build_ctx` reads `cross_mod_type()` once**, not twice (4dd4637) — review
  finding #20, a get + round + min + from_index each time.

- **19 test call sites** collapsed: `book()`'s six-tuple became a `Book` struct
  with all eight lanes and a `view()` method, so a test that wants one field
  different sets that field instead of re-listing eleven arguments.

### Declined, with numbers

**Step 2** (derive `sync`/`ring_mode`/`pm_index`, slim `BlockCtx`) costs
**−1.32% on the routed path** over 13 interleaved pairs, 11 of 13 negative, with
`busy_profile` flat at 0.00%. Not noise-shaped and concentrated exactly where
cross-mod work happens — most likely `BlockCtx` layout, since dropping two
`bool`s and an `f32` shifts every later field's offset and the frame loop reads
a lot of `ctx`. The prize was an unrepresentable state (`sync: true,
cross_mod_type: Off`) that nothing constructs.

**Step 3** (split the body at its phase banners) was not attempted, because the
premise was wrong. This ticket said "the seams are already written into the code
as banner comments". They are a narrative, not a decomposition:

- Phase 2 leaks ~15 locals into phases 3 and 4.
- Phase 3 is **not** a pure function of phase 2's output, which is what the
  planned `block_plan(&LaneBlock, ctx) -> BlockPlan` assumed. It mixes `any()`
  reductions, configuration written into `self` (ladder response + ramp, HPF
  coefficients, draining `trigger_pending`), and per-lane pan/amp gains — plus
  it declares phase 4's scratch buffers.

So the split would bundle 15 arrays into a record and move self-mutation into
helpers: a strictly larger perturbation than the one that already cost 1.32%,
for readability alone on the only hot path in [[E047]]. The banners were
corrected instead (1469e51) so the next reader is not misled the same way, with
the 1.32% cited where they would start cutting.

`render` is therefore still ~470 lines. That is a deliberate outcome, not an
unfinished one.

### Measurement method (worth reusing)

Sequential before/after profiling in this repo is **not trustworthy** — a second
session was building throughout, load ~2.5, and sequential sampling read
anything from 0% to −4.4% for a change ultimately measured at −0.5%. Every
number here comes from two separately-built binaries run alternately, which puts
the noise band at ±3% (busy) and ±1.4% (route).

Also learned: byte-comparing two release binaries is a valid codegen check here
(a control build proved identical source → identical bytes), but a comment
change that alters line counts changes the bytes anyway, via panic-location
metadata. Equal size with differing bytes means metadata only.

Verified: `vxn1b-engine` 305 pass, plus `alloc_free`, `zipper_regression`,
`parity`, `cross_mod_dest`, `taper_parity`, `oversampling_limiter` — the suites
that would catch a transposed slice while the change removes the ability to
introduce one.

**Manual DAW pass ([[verify-audio-in-reaper]]): done 2026-08-27, no change
heard.** A stacked, detuned, cross-modulated patch — the one that exercises
every per-lane slice this ticket reordered (velocity, pressure, note_random,
detune_cents, stack_pos). That is the check the parity suites cannot make: a
transposed pair would still have rendered, just wrongly, and only stacked
detuned notes make the difference audible.
