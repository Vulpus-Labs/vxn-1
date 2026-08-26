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
