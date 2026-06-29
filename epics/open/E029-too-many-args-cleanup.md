---
id: E029
product: monorepo
title: too_many_arguments cleanup — kill the data clumps, justify the keepers
status: open
created: 2026-06-28
---

## Goal

Audit of every `#[allow(clippy::too_many_arguments)]` in the
workspace (nine sites) split them into two piles: genuine
code smell to refactor, and defensible long lists to keep
with a written justification.

The lint fires nine times:

| Site | Args | Verdict |
|------|------|---------|
| `vxn-dsp` `oscillator.rs` `process_pair` | 9 | keep — SIMD pair kernel |
| `vxn-dsp` `oscillator.rs` `process_sync` | 7 | keep — SIMD pair kernel |
| `vxn-dsp` `oscillator.rs` `process_pm` | 8 | keep — SIMD pair kernel |
| `vxn-engine` `lib.rs` `decimate_block` | 7 | keep — paired L/R bus step |
| `vxn-engine` `voice.rs` `note_on` | 7 | **refactor** — per-note clump |
| `vxn-engine` `voice.rs` `mono_voice` | 7 | **refactor** — per-note clump |
| `vxn-engine` `voice.rs` `trigger` | 7 | **refactor** — per-trigger clump |
| `vxn-engine` `voice.rs` `mono_note_off` | 6 | **refactor** — per-note clump |
| `vxn2-engine` `engine.rs` `cook_stacks_block` | 8 | **refactor** — 4 bare bool flags |

This is not behaviour work. Every ticket is
behaviour-preserving; tests stay green and no audio render
changes.

## The two piles

**Refactor — data clumps.** The four `voice.rs` sites thread
the *same* per-note parameter tuple
(`note`, `velocity`, `alloc_tick`, `lfo1`, detune) through
four functions, hand-copied at every call site. That is a
textbook data clump: arg-order drift is easy (transpose
`velocity` / `detune_cents` and nothing type-errors), and
adding a per-note field is a four-signature edit. Group it
into a struct and the lint disappears as a side effect of the
real fix. The `cook_stacks_block` site is a boolean clump —
four positional `*_targeted: bool` flags at the call site,
the canonical unlabeled-positional hazard the lint exists to
flag.

**Keep — coupled kernels.** The three `oscillator.rs` sites
are profiled SIMD lane loops: two `&mut PolyOscillator` plus
disjoint `&mut [f32; N]` in/out arrays. The coupled
osc1/osc2-in/out shape is intrinsic; wrapping it in a struct
would split the borrows badly and risk de-vectorising the hot
loop. `decimate_block` is a single-caller paired L/R bus step
(four buffers + os + two drain bools). These stay — but every
retained allow gets a one-line justification comment so the
next reader knows it was a decision, not an oversight.

## In scope

- Introduce a `NoteOn { note, velocity, alloc_tick, lfo1 }`
  param struct and thread it through `note_on`, `mono_voice`,
  and `mono_note_off`; introduce a `Trigger` struct for
  `trigger`. Remove the four `voice.rs` allows.
- Introduce a `TargetFlags` struct grouping the four
  `*_targeted` bools in `cook_stacks_block`; build it once in
  `process_block`. Remove the allow; supersede the
  "grouping buys no clarity" comment.
- Add a one-line justification comment to every retained
  allow that lacks one (`process_sync`, `process_pm` are
  bare); confirm `process_pair`, `decimate_block`, already
  carry an adequate one.

## Out of scope

- The three `oscillator.rs` SIMD kernels and `decimate_block`
  keep their signatures — comment only, no struct wrap.
- No audio-behaviour or perf changes. No SIMD kernel body
  edits.
- vxn-3.

## Tickets

| # | Ticket | Product | Priority |
|---|--------|---------|----------|
| 1 | [0149 — voice.rs NoteOn/Trigger param structs](../../tickets/open/0149-voice-note-param-structs.md) | vxn-1 | medium |
| 2 | [0150 — cook_stacks_block TargetFlags](../../tickets/open/0150-cook-stacks-target-flags.md) | vxn-2 | low |
| 3 | [0151 — justify retained too_many_arguments allows](../../tickets/open/0151-justify-retained-arg-allows.md) | monorepo | low |

## Dependency order

```text
0149 (voice structs)   ── independent (vxn-1 control path)
0150 (target flags)    ── independent (vxn-2 engine)
0151 (justify keepers) ── independent; comment-only, land any time
```

## Acceptance

- `grep -rn too_many_arguments` over `crates/`,
  `vxn-1/crates/`, `vxn-2/crates/` finds exactly five
  remaining allows: the three `oscillator.rs` kernels and
  `decimate_block` (vxn-1) and zero in `voice.rs` /
  `engine.rs`.
- Every remaining allow has a justification comment on the
  same or preceding line.
- `cargo test --workspace` green; render baselines unchanged
  (the voice path and `cook_stacks_block` are
  behaviour-preserving refactors).
- `cargo clippy --workspace` clean (no new warnings from the
  removed allows).

## Notes

Source: 2026-06-28 audit of `clippy::too_many_arguments`
sites. Where line numbers drift from HEAD, symbol names are
authoritative.

The `voice.rs` control path (note-on / trigger) is event-rate,
not the audio sample loop — no SIMD concern; a plain struct is
free. The `cook_stacks_block` flags are also block-rate. Only
the `oscillator.rs` kernels are sample-hot, and those are
explicitly *not* touched.

Stage explicit paths when committing — `git add -A` pollutes
commits with concurrent vxn-2 working-tree churn (memory
`vxn-concurrent-vxn2-work-no-git-add-all`).
