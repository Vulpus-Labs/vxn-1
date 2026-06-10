---
id: "0064"
title: "Solo note-off: dispatch on AssignMode in note_off_patch"
priority: high
created: 2026-06-10
epic: E006
depends: []
---

## Summary

Fourth ticket of [E006](../../epics/open/E006-review-remediation.md).
`Engine::note_off`
([engine.rs:225-227](../../crates/vxn2-engine/src/engine.rs#L225)) calls
`PolyAlloc::note_off_patch`, which hardwires `note_off_poly` regardless
of assign mode
([alloc.rs:149-151](../../crates/vxn2-engine/src/alloc.rs#L149)):

```rust
pub fn note_off_patch(&mut self, _patch: &Patch, note: u8) {
    self.note_off_poly(note);
}
```

`note_off_solo` — held-note fallback, legato re-pitch, glide — is fully
implemented and unit-tested but unreachable from the engine. Solo mode
IS live (param-selectable,
[shared.rs:927-929](../../crates/vxn2-engine/src/shared.rs#L927)), so in
a host today releasing a solo note while holding another kills the
voice instead of falling back. The alloc tests miss it because they
call `alloc.note_off(...)` with explicit `AllocParams`, which does
dispatch correctly.

Fix: `note_off_patch` extracts `AllocParams` / `StackParams` /
`VoiceParams` from the patch (mirroring whatever `note_on_patch` does)
and delegates to the dispatching `note_off`
([alloc.rs:153-164](../../crates/vxn2-engine/src/alloc.rs#L153)).

## Acceptance criteria

- [ ] `note_off_patch` no longer ignores `_patch`; it dispatches Solo →
  `note_off_solo`, Poly → `note_off_poly` via the existing `note_off`.
- [ ] Engine-level integration test (in `engine.rs` tests or
  `param_sweep.rs`): set `assign-mode = Solo` via params, note-on 60,
  note-on 64 (legato), note-off 64 → output continues at the pitch of
  note 60 (assert non-silence + fundamental near 60's frequency, or
  assert via alloc state through a test accessor). This is the
  regression test the review found missing — it must go through
  `Engine::note_off`, not `alloc.note_off`.
- [ ] Existing poly-mode tests unaffected.

## Notes

Two adjacent review findings to sweep up here if trivial, otherwise
leave with a comment:

- `note_off_solo` doesn't clear an in-flight glide when
  `glide_from == 0.0` (inconsistent with `note_on_solo`'s explicit
  clear at [alloc.rs:319-321](../../crates/vxn2-engine/src/alloc.rs#L319));
  harmless today because `block_tick` expires it, but make the two
  paths symmetric.
- Solo fallback re-triggers the held note with the *released* note's
  velocity (`held` stores notes only). Decide: store velocities in
  `held`, or document the current behaviour as intentional.

## Close-out (2026-06-10)

- `note_off_patch` now takes `AllocParams` (mirroring `note_on_patch`)
  and delegates to the dispatching `note_off`; `Engine::note_off`
  passes its alloc snapshot through. No other callers existed.
- Both adjacent findings swept up:
  - glide-clear symmetry: the no-glide fallback branch in
    `note_off_solo` now clears any in-flight glide, matching
    `note_on_solo`.
  - fallback velocity: documented as intentional — the fallback reuses
    the sounding stack's velocity (classic mono-synth behaviour);
    `held` keeps storing notes only.
- Regression test `solo_note_off_falls_back_to_held_note_via_engine`
  goes through `Engine::note_off` (held-note fallback + audible output
  + final release), exactly the gap the review identified.
