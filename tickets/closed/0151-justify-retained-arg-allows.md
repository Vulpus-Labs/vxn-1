---
id: "0151"
product: monorepo
title: Justify the retained too_many_arguments allows
priority: low
created: 2026-06-28
epic: E029
---

## Summary

After `0149` / `0150` remove the data-clump allows, five
`#[allow(clippy::too_many_arguments)]` remain — all
defensible, but two are bare. Add a one-line justification to
every retained allow so the next reader knows it is a decision,
not an oversight. Comment-only; no signature or behaviour
change.

## Retained sites

All in vxn-1:

- `crates/vxn-dsp/src/poly/oscillator.rs` `process_pair`
  (line ~410) — already commented
  (`// two waves + two pw/out arrays is the coupled shape`).
  Confirm adequate, leave.
- `crates/vxn-dsp/src/poly/oscillator.rs` `process_sync`
  (line ~510) — **bare, add comment.**
- `crates/vxn-dsp/src/poly/oscillator.rs` `process_pm`
  (line ~599) — **bare, add comment.**
- `crates/vxn-engine/src/lib.rs` `decimate_block`
  (line ~366) — already commented
  (`// one paired decimate step, single caller`). Confirm,
  leave.

Justification to record: these are profiled SIMD lane kernels
(`process_*`) / a single-caller paired-bus step
(`decimate_block`). The args are two `&mut PolyOscillator`
plus disjoint `&mut [f32; N]` in/out arrays (or the L/R bus
buffers); the coupled in/out shape is intrinsic and wrapping
it in a struct would split the borrows and risk de-vectorising
the hot loop. Keep flat by design.

## Acceptance criteria

- [ ] `process_sync` and `process_pm` allows each carry a
      one-line justification matching `process_pair`'s style.
- [ ] `process_pair` and `decimate_block` comments confirmed
      adequate (or tightened).
- [ ] No signature, body, or behaviour change anywhere —
      `git diff` is comments only.
- [ ] `grep -rn too_many_arguments` shows every remaining
      allow has an adjacent justification comment.

## Notes

Comment-only — do not touch the kernel bodies; no asm
re-verification needed. The SIMD-discipline cautions for the
oscillator kernels (NEON `.4s` survival, post-LTO asm) do not
apply here because no kernel code changes. Pairs with `0149`
(voice clump) and `0150` (engine flags) under E029.

## Close-out

Verified — all four retained allows carry a one-line
justification. `process_sync` (`oscillator.rs:510`) and
`process_pm` (`:599`), previously bare, now read
`// coupled SIMD pair kernel: two waves + two pw/out arrays
[+ pm_index]`; `process_pair` (`:410`) and `decimate_block`
(`lib.rs:370`) already carried adequate comments and are left
as-is. The `grep -rn too_many_arguments` sweep over `crates/`,
`vxn-1/crates/`, `vxn-2/crates/` returns exactly these four
allows — zero in `voice.rs` / `engine.rs` after 0149/0150 —
each with an adjacent comment. Comment-only; no signature,
body, or behaviour change (these comments landed with the
earlier keeper-commenting pass, confirmed on HEAD).
