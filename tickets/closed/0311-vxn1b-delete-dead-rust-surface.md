---
id: "0311"
product: vxn-1b
title: "Delete the dead Rust surface, incl. a second voice-allocation policy that never runs"
priority: medium
created: 2026-08-26
epic: E047
depends: ["0321"]
---

## Summary

Seven unreachable items in `vxn-1b`'s Rust. One of them is a whole allocation
policy; the rest are small but each one is a thing a reader has to rule out.

### 1. The single-lane allocation path (the one that matters)

[`Voices::note_on`](../../vxn-1b/crates/vxn1b-engine/src/voice.rs#L374) and
[`allocate`](../../vxn-1b/crates/vxn1b-engine/src/voice.rs#L226) have no
production caller. `Synth::note_on` routes through `note_on_stack`, which claims
lanes via [`claim_lanes`](../../vxn-1b/crates/vxn1b-engine/src/voice.rs#L211);
the ~20 call sites for the older path are all inside `voice.rs`'s and
`synth.rs`'s own `#[cfg(test)]` modules. `AllocView`'s only non-`worst_stack`
consumer goes with them.

So **two independently-maintained voice-stealing policies exist and one ships**,
with a test suite proving properties of the one that doesn't. `claim_lanes`'s
doc already asserts the two agree at uniform width — which is the argument for
re-pointing those tests at `note_on_stack(..., width 1)` rather than keeping the
code to keep the tests.

### 2. `_PARAM_COUNT` protects nothing

[vxn1b-clap/src/lib.rs:844-846](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L844-L846):
`#[used] static _PARAM_COUNT: usize = TOTAL_PARAMS;` — *"Keep the param count
referenced so a thin-LTO cdylib never drops the table."* But `TOTAL_PARAMS` is a
`const usize` ([params.rs:793](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L793)),
which has no storage to keep alive, and the descriptor table is actually reached
through `desc_for_clap_id` from three live `PluginMainThreadParams` methods.
Cargo-culted from [vxn-clap/src/lib.rs:581](../../vxn-1/crates/vxn-clap/src/lib.rs#L581).

### 3. `Engine::max_frames` — stored, exposed, never read

[engine.rs:227](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L227) +
[:419](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L419). No `.max_frames()`
call exists in the tree; the CLAP shell uses its own `max_frames_count` local
([lib.rs:561](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L561)) to build the
engine.

### 4. Two `set_sample_rate` chains for a lifecycle that doesn't happen

[`OutputStage::set_sample_rate`](../../vxn-1b/crates/vxn1b-engine/src/output.rs#L101)
has zero callers.
[`MotionSmoother::set_sample_rate`](../../vxn-1b/crates/vxn1b-engine/src/mod_smoothing.rs#L178)
is called only by `RenderBank::set_sample_rate`
([bank.rs:496](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L496)), which itself
has zero callers — the engine is rebuilt on a sample-rate change instead. Three
functions maintained for a path nothing takes.

### 5. `last_width` is write-only

[voice.rs:290](../../vxn-1b/crates/vxn1b-engine/src/voice.rs#L290), set in
`Voices::new` and `sync_mode`, never read. Its own doc says so: *"Recorded
rather than acted on."*

### 6. `is_sync_flag`

[sync.rs:50](../../vxn-1b/crates/vxn1b-engine/src/sync.rs#L50) is reached only
from its own test. Ported from vxn-1 where it *is* live; VXN1b's CLAP shell and
web controller both call `rate_partner_clap_id` directly — and
[vxn1b-clap/src/lib.rs:322](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L322)
spells out the same `is_some()` test inline, which is either the call site it
should have or the reason it isn't needed.

### 7. Two small no-ops

- [tests/parity.rs:137-139](../../vxn-1b/crates/vxn1b-engine/tests/parity.rs#L137-L139):
  `#[allow(dead_code)] const _: fn() -> ParamId = || ParamId::MasterVolume;`
  — a no-op const propping up an import that isn't used. Delete both.
- [xtask/src/main.rs:146](../../vxn-1b/xtask/src/main.rs#L146): `--release` is
  *"Accepted and ignored"*, and
  [release.yml:322,386](../../.github/workflows/release.yml#L322) passes it. A
  flag with one live behaviour, kept so three products' CI lines read alike.

## Design

Items 2–7 are independent deletions; do them in one commit each or one commit
total, whichever reads better in the log.

Item 1 needs a decision recorded, not just a delete: the tests attached to
`Voices::note_on` are testing *voice stealing*, which is real behaviour that
still needs coverage. Re-point them at `note_on_stack` with `width = 1` and
confirm they still assert something — if a test only passes because it was
exercising the simpler policy, that is a coverage gap this ticket has found, not
a test to drop.

## Acceptance criteria

- [ ] One voice-allocation policy remains in the tree.
- [ ] Every test formerly covering `Voices::note_on` either runs against
      `note_on_stack` or is recorded as deleted with a reason.
- [ ] Items 2–7 are gone.
- [ ] `--release` is removed from the VXN1b invocations in `release.yml` as well
      as from the xtask, so nothing passes a flag that does not exist.
- [ ] `cargo test -p vxn1b-engine` and `-p vxn1b-clap` green; `tests/alloc_free.rs`
      still passes, which is the one that would notice if the surviving path
      allocates.
- [ ] One manual DAW pass — [[verify-audio-in-reaper]]. Voice stealing has an
      audible signature and no automated test proves the *feel* of it; play a
      dense passage past the voice limit and confirm nothing changed.

## Notes

- Item 1 is the only part of this ticket that can plausibly change audio, and it
  should not — the deleted path does not run. The DAW pass is there because "does
  not run" is exactly the kind of claim that is embarrassing to be wrong about in
  a shipped synth.
- No `cargo fmt` — [[vxn-no-cargo-fmt]]. One `cargo test` at a time —
  [[vxn-no-parallel-cargo-test]]. Stage explicit paths —
  [[vxn-concurrent-vxn2-work-no-git-add-all]].

## Close-out (2026-08-26)

All seven items gone, and the first one cascaded further than the ticket
expected.

### The second allocation policy

`Voices::note_on` and `allocate` are deleted. Every one of their 31 call sites
lived under `mod tests`; production has always gone through `note_on_stack` →
`claim_lanes`.

The tests cover **voice stealing**, which is real behaviour, so they were
re-pointed rather than dropped — a `note_on_1` shim runs one note through the
shipping allocator at width 1, which is exactly what the retired entry point
did. **All 50 pass unchanged**, which is the interesting result: it confirms
`claim_lanes`' claim that the two policies agree lane-for-lane at uniform width,
and means the tests were never testing the dead path's *behaviour*, only its
existence.

Deleting `allocate` then made `AllocView`'s `active` and `alloc_tick` fields
dead — it was their only reader. `steal_tier` (still live, called by
`worst_stack`) only ever read `gate`, so `AllocView` and `Voices::view()` are
gone too and `steal_tier` takes `&[bool; N]` directly. The struct existed to
keep the retired policy pure and testable in isolation; nothing else wanted it.

### The rest

- **`_PARAM_COUNT`** ([vxn1b-clap](../../vxn-1b/crates/vxn1b-clap/src/lib.rs)) —
  a `#[used] static` "keeping alive" a `const usize`, which has no storage to
  keep. Cargo-culted from vxn-1.
- **`Engine::max_frames`** — field, getter **and constructor parameter**, across
  65 call sites. Verified it sized nothing: every buffer `Engine::new` owns is
  `CONTROL_BLOCK * MAX_OVERSAMPLE`. It was not merely unread, it was
  *misleading* — a reader would assume the host's block size mattered here. Note
  the CLAP shell still reads `max_frames_count` for its own `scratch_l`/`_r`,
  which is the honest use the engine's copy was imitating.
- **Two `set_sample_rate` chains** — `OutputStage::` (0 callers) and
  `MotionSmoother::` (only caller was `RenderBank::set_sample_rate`, itself 0
  callers). The engine is rebuilt on a rate change; nothing takes this path.
- **`last_width`** — write-only field. Removing it left `sync_mode`'s `width`
  parameter unused, so that went too: the argument existed solely to feed a
  field nothing read.
- **`is_sync_flag`** — reached only from its own test.
- **`parity.rs`'s no-op const** — and the "unused import" it claimed to prop up
  is not unused (`ParamId` is used at line 55), so it was propping up nothing.
- **`--release`** — dropped from the four VXN1b CI invocations, the help text
  and the module doc. Unknown *flags* are ignored by the parser (only unknown
  subcommands error), so a stray one is still tolerated — verified by hand — but
  it is no longer documented or passed. It existed to make the workflow line
  scan like vxn-1's and vxn-2's, which is not a reason for a flag to exist.
  vxn-1's and vxn-2's own xtasks are untouched.

### Verified

`vxn1b-engine` 305 + `alloc_free` / `zipper_regression` / `parity` /
`cross_mod_dest` / `taper_parity` / `fx_stereo` / `oversampling_limiter`,
`vxn1b-clap` 7, `vxn1b-wasm` 28, `vxn1b-web-controller` 34, `vxn1b-ui-web` 14,
`vxn1b-xtask` 5 — all green, **zero compiler warnings**. `alloc_free` matters
most here: it is the test that would notice if the surviving allocation path
started allocating.

`busy_profile` 16.0× / 15.9× against a ~15.9–16.2× baseline — unchanged, as
expected for deleting code that never ran.

**Manual DAW pass ([[verify-audio-in-reaper]]): done 2026-08-27, no change
heard.** Voice stealing has an audible signature and no test proves the *feel*
of it, so playing past the voice limit was the check that mattered — and it
confirms the claim the whole ticket rested on: the deleted allocation path did
not run. Closed on test evidence a day earlier; this is the confirmation, not a
caveat.
