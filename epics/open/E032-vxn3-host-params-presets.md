---
id: E032
product: vxn-3
title: "vxn-3 host params + preset groundwork — fixed mix/master table + macro slots"
status: open
created: 2026-07-02
---

> **Anchors ticket 0072.** Design is fixed in `vxn-3/adrs/0003-vxn3-host-param-model.md`
> (host param model) with `vxn-3/adrs/0001` §3a/§4/§5 (voicing + p-locks) and
> `vxn-3/adrs/0002` (master/send FX). This epic *is* the build-out of 0072, which
> is folded in as the design anchor. Post-MVP breadth — E021 shipped the groove
> proof with **no `clap.params` at all**; this epic gives vxn-3 a host-facing
> parameter surface and the state format that a preset epic will build on.

## Goal

Give VXN3 a `clap.params` table so the host can **automate / modulate / save**
parameters, **without** exposing the variable per-engine param set (whose
cardinality and semantics change on engine swap). The surface is split into a
**fixed host layer** (this epic) and the existing **faceplate-only layer** (0052,
p-lockable via 0050) — never host-automate the variable engine set directly.

When this epic closes:

- VXN3 declares a **fixed, deterministic** `clap.params` table — same ids every
  session, **never rescanned** on engine swap.
- Per track (× `N_TRACKS`): `level`, `pan`, `mute`, delay `send`, and **3 generic
  macro slots** the active engine reinterprets onto its patch.
- Master: volume, limiter, global delay (fb / time / return).
- A macro slot has a **fixed generic id/name** (`T3 · M1`) but **engine-aware
  value-text** ("Decay 0.42 s" under Kick vs "Ring 1.8 s" under Metal).
- Faceplate + p-lock edits **echo back** to the host so automation lanes track.
- `clap.state` **saves/restores** the fixed table + per-track engine kind + patch
  blob; state round-trips **through** an engine swap.
- Param flush + value→engine routing is **allocation-free** on the audio thread;
  `clap-validator` reports 0 failures.

## Why now

E021 proved the thesis but left nothing host-automatable or host-saved: the
faceplate drives the engine over a custom-event channel and p-locks are the only
automation. That is fine for a groove proof, wrong for a shippable instrument.
This epic closes the gap with the model 0072/ADR 0003 already settled, and pins
the **state blob format** so the future preset epic (which persists engine + patch
+ macro values) has a stable foundation.

**Decisions (locked):** `K = 3` macro slots per track (1:1 with today's
`Knob { Decay, Tone, Pitch }`). `clap.state` is **in scope** here — the ADR wants
the blob format decided alongside the table regardless.

## Planned tickets

Dependency chain: **0170 → 0171 → { 0172, 0173 } → 0174**.

- [ ] **0170** — Macro-slot trait generalization + fix dead mappings. Replace
      `Knob`/`set_knob` with a declared macro mapping on `TrackEngine`
      (`macro_count`, `set_macro`, and a **pure, `EngineKind`-dispatched**
      `macro_display` free fn — callable on the main thread). Rename
      `EngineCommand::SetKnob` → `SetMacro`. Update the 3 engines + app + `ui-web`.
      Fix the dead mappings (Tone no-op on Kick/Metal; Pitch absent from faceplate).
      Engine/app/UI refactor — **no clap**. Foundation.
- [ ] **0171** — `clap.params` fixed table + host→engine automation writes. Add
      `PluginParams` to the clap shell; declare the fixed positional-id table
      (per-track mix + 3 macros × `N_TRACKS` + master); implement `count` /
      `get_info` / `get_value`; route incoming param events → `EngineCommand`.
      Generic `value_to_text` stub (engine-aware text is 0172).
- [ ] **0172** — Engine-aware `value_to_text` + main-thread engine-kind tracking.
      Record each track's active engine kind on the main thread (on the `SetEngine`
      swap). `value_to_text` for a macro dispatches to 0170's pure `macro_display`
      keyed by that track's kind; mix/master params render normally.
- [ ] **0173** — Host echo pump (UI / p-lock → host), alloc-free flush. Adapt
      vxn-2's dirty-bitset pump: faceplate + p-lock writes to any host-facing param
      emit an output param-value event + `request_flush` so lanes track internal
      edits. p-locks may target macro + mix params (0050). Flush + routing stays
      allocation-free on the audio thread.
- [ ] **0174** — `clap.state` save/restore + integration pass. Serialize the fixed
      table + per-track engine kind + patch blob; define the blob format (ADR 0003
      §Consequences) so state round-trips through an engine swap. Integration: id
      stability across re-instantiation & project reload; `clap-validator` clean.

## Risks

- **`value_to_text` cross-thread.** The live engine is on the audio thread; text
  is a main-thread call. Mitigated by making `macro_display` a **pure**
  kind-dispatched free fn (0170) + main-thread engine-kind tracking (0172) — no
  reach into the audio-thread engine.
- **Echo feedback loop.** Host write → engine → echo → host could oscillate;
  the dirty-bitset pump must echo only *internally-originated* changes, not host
  writes (the vxn-2 discipline). 0173.
- **State through a swap.** The blob must carry engine kind + patch so restore
  rebuilds the right engine before applying macro/mix values. Format frozen in
  0174, consumed by the preset epic.
- **RT discipline.** Param flush must not allocate; extend the existing alloc-trap
  test rather than trusting review.

## Acceptance

- Fixed `clap.params` table; ids stable across re-instantiation and project reload;
  **no rescan** on engine swap.
- Host automation of a per-track macro drives the active engine's mapped param;
  swapping the engine changes what the slot does **without** changing its id.
- `value_to_text` renders macros engine-aware; mix/master render normally.
- Mix (level/pan/mute/send) + master are automatable and engine-independent.
- A p-lock on a macro / mix param round-trips and is reflected to the host.
- `clap.state` round-trips the table + engine kind + patch through a reload.
- Param flush + value→engine routing is allocation-free; `clap-validator` 0 failures.
