---
id: E034
product: vxn-3
title: "vxn-3 voice roster + editable flavours — make it a playable toy"
status: open
created: 2026-07-04
---

> **Roadmap pivot: toy before instrument.** E021 proved the pattern engine; E032
> gave it a host surface. Rather than march straight to a complete instrument
> (arrangement, kits, mixing), this epic expands the **voice roster early** so the
> thing is fun to *play with* — and so playing it reveals what it should become.
> The reference is the sibling `patches-drums` repo (17 modules, 7 categories);
> the model that keeps them RT-safe and editable is
> [ADR 0005](../../vxn-3/adrs/0005-vxn3-voice-families-flavours-macros.md).

## Goal

Turn VXN3's three thin engines into a **four-family roster** whose drum sounds are
**editable "flavours"** — points in each family's parameter space, with editable
base values *and* editable macro bindings, and the ability to define new flavours
by hand. When this epic closes:

- VXN3 hosts **four voice families** (ADR 0005): **Driven**, **Noise**, **Metal**,
  **Struck** (new BridgedT resonator school) — the three existing engines enriched
  to cover their category range, plus the resonator family.
- A **flavour** = base param vector + `K=3` macro-binding table (+ default macro
  values), authored as data. Kick / Tom / Claves are flavours of one family.
- Macro bindings are **additive-from-base** and **editable** on the faceplate; a
  player can retarget a macro, change its depth, and **save a new flavour**.
- The deep patch (base + bindings) **round-trips** through `clap.state` and an
  engine swap (built on **0179**).
- MIDI **free-play** note-in lets you audition/jam voices by hand, not only via the
  sequencer.
- A starter set of **factory flavours** ships so the toy is fun on first load.

## Why now

The sequencer is strong (E021) and host-automatable (E032), but the *sound* palette
is three sparse engines with fixed, partly-dead macro maps — not enough to play
with, and nothing you discover can be shaped or saved. Expanding voices now is the
cheapest path to a jammable instrument and the honest way to spec the rest of the
roadmap (arrangement, kits, mixing) from experience instead of guesswork. It also
converts the roadmap's Phase 5 (sound breadth) into the immediate next step and
pulls MIDI free-play forward from Phase 3.

**Decisions (locked by ADR 0005):** four families, closed roster; flavour = base +
binding table + default macro values, authored as data; macro binding is a
constrained additive-from-base mod (not the vxn-2 general matrix); base + bindings
are the faceplate-only deep patch, serialised via the reserved 0179 bytes; host
still sees only 3 macros/track (ADR 0003 unchanged).

## Planned tickets

Dependency chain: **0179 → 0180 → { 0181, 0182, 0183, 0184 } → 0185**; **0186**
parallel; **0187** last. (Ids 0180+ reserved here; 0179 already open as groundwork.)

- [ ] **0179** — *(open, groundwork)* Per-engine patch serialization; fill the
      reserved `clap.state` bytes so a deep patch (→ flavour) round-trips through
      save/load + swap. Finishes roadmap Phase 0.
- [ ] **0180** — **Flavour runtime + macro-binding core.** The mechanism from ADR
      0005: family param space with metadata (id/range/default/curve); `Flavour`
      = base vector + binding table + default macro values; additive-from-base
      evaluation on trig, allocation-free; flavour load/apply onto an engine.
      No new synthesis, no UI yet — the model plumbing all four families build on.
- [ ] **0181** — **Enrich Driven family + author flavours.** Add `sweep-start`,
      `drive`, `click` to the driven param space; author flavours: kick, tom,
      snare-body, claves. Prove two flavours of one family morph via base edits.
- [ ] **0182** — **Enrich Noise family + flavours.** Add bandpass freq/Q, `snap`
      transient, multi-tap burst gate; author flavours: snare-noise, clap.
- [ ] **0183** — **Enrich Metal family + flavours.** Add XOR-pair + modal-bank
      options + shimmer LFO + open/closed decay; author flavours: closed hat, open
      hat, ride, cymbal (open/closed choke handled here or deferred to Phase 1).
- [ ] **0184** — **New Struck family (BridgedT).** Port the resonator school:
      pitch-droop, Q-as-decay, selectable excitation shape (dirac / exp / half-sine
      / filtered-click). Author flavours: kick2, tom2, claves2, modal cymbal.
- [ ] **0185** — **Flavour editor + save-as-flavour (faceplate).** Base sliders +
      macro-binding assignment surface (target param, depth, curve per slot) +
      save a new flavour to the user store. `value_to_text` (0172) becomes
      flavour-aware. The "playable-with" payoff.
- [ ] **0186** — **MIDI free-play note-in.** Add a CLAP note input port; map
      incoming notes → track/voice so voices can be auditioned/jammed by hand.
      Independent of the flavour chain — land early for immediate playability.
- [ ] **0187** — **Factory flavour/kit starter bank.** A `include_dir!` set of
      factory flavours across all four families (+ a handful of assembled kits) so
      the toy is fun on first load. Mind [[vxn2-include-dir-no-rerun]].

## Risks

- **Flavour = the 0179 blob.** The base+binding layout must be right before flavours
  are authored, or every flavour is format debt. 0180 freezes it; 0179 wires it.
- **Param-space bloat.** Enriching a family invites a kitchen-sink `P`. Discipline:
  the minimum params needed to reach the family's flavours, discovered by authoring
  them (0181–0184), not specified up front.
- **RT discipline.** Binding eval must stay allocation-free (per-trig sum); extend
  the alloc-trap tests. Per-sample kernels stay untouched SoA.
- **Faceplate scope creep (0185).** A binding editor is real UI. Keep it minimal —
  target/depth/curve per slot + save — resist a full modular patcher.
- **`value_to_text` cross-thread + now flavour-aware.** Keep the 0172 pure
  kind/flavour dispatch; no reach into the audio-thread engine.

## Acceptance

- Four families live; the three existing engines enriched, Struck added; roster
  closed.
- A flavour (base + 3 macro bindings + defaults) loads onto its family, evaluates
  additive-from-base allocation-free, and round-trips through `clap.state` + a swap.
- Two flavours of the same family morph into each other via base edits; a macro can
  be retargeted/re-depthed and a **new flavour saved** from the faceplate.
- MIDI notes audition/jam voices by hand.
- A factory flavour/kit bank ships; VXN3 is fun on first load.
- `clap-validator` 0 failures; `cargo test -p vxn3-engine -p vxn3-clap` green;
  alloc-trap tests pass.
