---
id: "0390"
product: vxn-3
title: "Remove retrig — the macro presupposed a step grid that no longer exists"
priority: medium
created: 2026-09-12
epic: E050
depends: ["0350", "0354", "0355"]
---

## Summary

Retrig was **withdrawn from the design** by
[ADR 0007's 2026-09-12 amendment](../../vxn-3/adrs/0007-vxn3-continuous-lane-editor.md#amendment-2026-09-12--retrig-is-withdrawn),
which also withdrew [ADR 0001](../../vxn-3/adrs/0001-vxn3-overall-design.md) §2's
"Retrig n-over-m" lever and its listing among the trig attributes in §3a. The code
still implements it. This ticket removes it.

The reasoning, in short: retrig was *"a trig owns a sub-window of `m` steps and
fires `n` times within it"* — a macro over **step indices**. It presupposed the
grid E050 removed. Since 0348 a hit is a free point on a continuous timeline with
its own `f`, `nudge`, `y` and `rgb`, so `n` hits placed where the user wants them
is strictly more expressive than `n` curve-spaced subdivisions of a span measured
in steps. The macro compresses a vocabulary the hit list now has natively, and
costs a parallel scheduling path to keep.

## Design

Retrig is not one field. It is a second way for a hit to become several fire
times, and it reaches into the scheduler, the edit vocabulary and the faceplate:

- [`sequencer.rs`](../../vxn-3/crates/vxn3-engine/src/sequencer.rs) — `Retrig`,
  `RetrigCurve` (+ `position`), `Hit::retrig`, `Pattern::set_retrig` /
  `set_hit_retrig`, `retrig_span`.
- [`lane.rs`](../../vxn-3/crates/vxn3-engine/src/lane.rs) — `expand_retrig`,
  `Pending::from_retrig`, `Window::drop_retrig_tail`, and the window sizing that
  is derived from a retrig's `n` (`MAX_HITS_PER_POSITION`, and `WINDOW_CAPACITY`
  which is built from it).
- [`io.rs`](../../vxn-3/crates/vxn3-engine/src/io.rs) — `EngineCommand::SetRetrig`
  and its hit-keyed sibling, and their routing.
- [`engine.rs`](../../vxn-3/crates/vxn3-engine/src/engine.rs) — the command arms.
- [`vxn3-ui-web`](../../vxn-3/crates/vxn3-ui-web) — the retrig opcode, the
  faceplate affordance, the `.retrig` cell style, and the readback field.

**The window sizing is the part to think about rather than delete.** `WINDOW_CAPACITY`
is `MAX_HITS_PER_POSITION + MAX_HITS`, where the first term exists *only* because
one retrig could expand to 255 fire times. With retrig gone the window holds at
most one fire time per hit, so the capacity argument collapses to `MAX_HITS` — but
that is a claim about the bound, and the `const _` asserts in `lane.rs` that tie
the sizing to the offset invariants must be re-derived, not just made to compile.

**Also decide the budget question the amendment flags.** A retrig was one `Hit`
that expanded to `n` fire times at schedule time; written out it is `n` entries
against `MAX_HITS = 64`. Measure whether a realistic dense pattern still fits, and
either raise the ceiling with a stated reason or record that it was measured and
left alone.

## Acceptance criteria

- [ ] `Retrig`, `RetrigCurve` and `Hit::retrig` are gone; no type in `vxn3-engine`
      carries a retrig field.
- [ ] `expand_retrig`, `drop_retrig_tail` and `Pending::from_retrig` are gone, and
      a hit resolves to exactly one fire time.
- [ ] The retrig edit commands are gone from `EngineCommand`, from the JSON
      opcode vocabulary, and from the faceplate.
- [ ] `WINDOW_CAPACITY` and its `const _` asserts are **re-derived** from the
      remaining invariants, with the reasoning in the comment updated rather than
      trimmed — the old comment justifies a term that no longer exists.
- [ ] The `MAX_HITS` budget question is settled: a dense written-out roll is
      measured against the ceiling, and the outcome is recorded either way.
- [ ] `tests/pattern.rs`'s retrig coverage is removed, not disabled; every
      remaining test passes unedited.
- [ ] The allocation traps in `tests/{groove,pattern,plocks,faceplate_io}.rs` stay
      green.
- [ ] The patch blob rejects a stale version rather than misparsing it, if the
      hit encoding changes width.

## Notes

Sequencing: this must land **after** the E050 wave in flight (0350, 0354, 0355),
all three of which touch files with a large retrig footprint — `lane.rs` alone has
~49 mentions and `sequencer.rs` ~35. Doing it first would conflict with all three
for no gain.

ADRs 0004 and 0006 are superseded and describe retrig's interaction with
micro-timing and with the groove template. They are **left untouched** — they
record what was decided at the time, and rewriting a superseded ADR destroys the
record rather than correcting it. Do not "fix" them as part of this ticket.

A generator that *places* hits (a roll tool that writes `n` hits and then forgets
about them) is the natural replacement affordance and is explicitly **not** this
ticket — that is editor work, and it produces data rather than persisting as a
parallel representation of it.
