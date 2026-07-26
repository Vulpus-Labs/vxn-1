---
id: E036
product: vxn-1b
title: "VXN1b core — matrix-modulated engine + all sources (scaffold, MPE plumbing, param table, evaluator, persistence, CLAP)"
status: open
created: 2026-07-25
---

## Goal

Stand up **VXN1b** — the matrix-modulation variant of VXN1 (see
[ADR 0001](../../vxn-1b/adrs/0001-vxn1b-overall-design.md)) — as a
DAW-playable CLAP plugin with **host-generic knobs** (no faceplate yet). The
sound engine is VXN1's, reused verbatim by sharing `vxn-dsp`; the divergence is
that VXN1's fixed per-channel modulation routing is replaced by a generic
**16-slot mod matrix** in the VXN2 idiom, fed by the **full source roster** —
including the two sources VXN1 lacks (**MPE aftertouch**, **note-on random**),
whose novel engine plumbing is done here, early, because it is voice-allocation
architecture the rest of the engine builds on.

When this epic closes:

- A new sibling product exists at `vxn-1b/crates/{vxn1b-engine, vxn1b-clap,
  vxn1b-ui-web}` + `vxn-1b/xtask`, wired into the root workspace, taking a
  direct dependency on VXN1's `vxn-dsp` (zero DSP fork — §1 of the ADR).
- **MPE aftertouch** works: the originating MIDI channel is tracked through
  voice allocation so per-note pressure reaches *that note's* voice; channel
  pressure is the degenerate broadcast case. **Note-on random** is a per-voice
  RNG value latched at note-on.
- Modulation is a generic matrix: `MatrixSlot { source, dest, depth, curve,
  scale_src }`, additive per dest, with the ADR 0009 per-route scale VCA, fed by
  all ten sources.
- The default patch **seeds slots** so it renders **bit-identically to VXN1's
  default** (Env2→Amp @ 1.0; Key→Cutoff at exactly 1 oct/oct — the hardwired
  terms of VXN1 ADR 0004 become default matrix routes).
- Slot **depths are CLAP-automatable params**; slot `source`/`dest`/`curve`/
  `scale_src` are **patch topology** (state + TOML, not automatable).
- Presets (sparse TOML) and `clap.state` (packed topology) round-trip; the
  bundle installs and passes `clap-validator`.

Out of scope for E036 (later epics): the FX section — Dynamics kernel copy +
tabbed serial chain ([[E037]]); the compact faceplate, matrix overlay, factory
bank, release ([[E038]]). The **browser/web port is deferred entirely** (the
VXN1/VXN2 ports set a simple precedent to follow later).

## Why now

VXN1b's whole identity is the routing model; everything else is reuse. Building
the engine + persistence + CLAP shell first yields a headless, DAW-testable
instrument that proves the matrix evaluator reproduces VXN1's sound before any
UI work. The default-patch render-parity test is the linchpin: if the seeded
matrix doesn't equal VXN1's fixed routing, the variant isn't faithful. **MPE is
in this epic, early**, because threading MIDI channel through note allocation is
genuinely new plumbing (VXN1's allocator is channel-agnostic) that shapes the
voice struct the evaluator reads — retrofitting it later would churn the core.

## Design (locked by ADR 0001)

- **Sources (all ten, this epic):** Env 1, Env 2, LFO 1, LFO 2, Velocity, Key
  (relative C4), Mod Wheel, Pitch Wheel, **Aftertouch (MPE-aware)**, **Note-on
  Random**. The last two need new engine plumbing (below); the first eight exist
  in VXN1 already.
- **MPE aftertouch.** Thread a `channel` field from note-on through the allocator
  into the voice; fold poly-pressure (per-note) and channel-pressure events by
  channel into per-voice pressure state; expose as a per-voice matrix source.
  Channel pressure broadcasts to every voice on that channel. Voice stealing must
  adopt the stealing note's channel.
- **Note-on random.** Per-voice `f32` in `[0,1)` latched at note-on from a cheap
  deterministic-per-voice RNG; fixed for the note's lifetime.
- **Destinations (v1 core):** Pitch (vibrato, both osc), X-Mod Sweep (wide,
  mode-aware per VXN1 ADR 0004), PWM, LP Cutoff, Resonance, HPF Cutoff, Amp
  (VCA), Cross-Mod Amount.
- **Evaluator.** Sources evaluated per control block (sr/32) into a
  `[lane][source]` table; dest application keeps VXN1's consumption-matched
  smoothing (per-sample cutoff coeff interp, per-sample pitch, block-rate
  one-pole gains). `scale_norm` per ADR 0009 (unipolar passthrough; bipolar
  `(x+1)×0.5` clamp `[0,1]`). Alloc-free.
- **Param split.** 16 slot depths = `clap.params`; topology = state/TOML only.
  The stripped fixed-panel depth params of VXN1 (`PitchLfoDepth`,
  `CutoffLfo1Depth`, `VelCutoffDepth`, `ModWheel*`, …) are removed.
- **Persistence.** Sparse TOML `[[matrix]]` table (kebab keys, inactive omitted);
  binary state packs the active-bit + source/dest/curve/scale bytes (VXN2 ADR
  0009 layout). Depths ride the normal param blob.

## Planned tickets

Dependency chain: **0197 → 0198 → 0199 → 0200 → 0201 → 0202 → 0203 → 0204**.
(0198 MPE + 0199 random are the early novel-plumbing tickets, before the matrix.)

- [ ] **0197** — Scaffold `vxn-1b` product. Create `vxn1b-engine`, `vxn1b-clap`,
      `vxn1b-ui-web` crates + `vxn-1b/xtask`; add to root `Cargo.toml` members;
      depend on `vxn-1/crates/vxn-dsp` and the shared `vxn-core-*` crates. A stub
      CLAP that loads in a host (params/state can be empty). No DSP yet.
- [ ] **0198** — MPE voice architecture (novel plumbing, early). Thread a
      `channel` field note-on → allocator → voice; fold poly-pressure (per-note)
      and channel-pressure events by channel into per-voice pressure state; voice
      stealing adopts the new note's channel. No matrix dependency yet — this is
      pure voice-allocation architecture. Tests: per-note pressure reaches only
      that voice; channel pressure reaches all voices on the channel; a stolen
      voice re-parents its channel.
- [ ] **0199** — Note-on random per-voice latch. Per-voice `f32` in `[0,1)`
      latched at note-on from a deterministic-per-voice RNG; stable for the
      note's life. Tests: value constant across a note, differs across voices,
      in range.
- [ ] **0200** — VXN1b param table. Fork VXN1's flat index-addressed table into
      `vxn1b-engine`; keep osc/mixer/filter/env/LFO params; **remove** the fixed
      mod-panel depth params (ADR 0004's `PitchLfoDepth`, `Pwm*Depth`,
      `Cutoff*Depth`, `VelCutoffDepth`, `ModWheel*`, `*EnvSrc`/`*LfoSrc`
      selectors, `FilterKeyTrack`); add **16 slot-depth** params. Keep pitch-bend
      range hardwired. `ParamDesc`/display coverage; enum-index tests.
- [ ] **0201** — Matrix data model + default patch. `MatrixSlot { source: SourceId,
      dest: DestId, depth, curve: Curve, scale_src: SourceId }`; `SourceId`
      (all ten sources incl Aftertouch + NoteRandom) / `DestId` enums;
      `default_patch` seeds Env2→Amp @ 1.0 and Key→Cutoff at 1 oct/oct
      (reproducing VXN1's hardwired VCA + key-track). No eval yet. Tests: enum
      round-trip, default-patch slot contents.
- [ ] **0202** — Matrix evaluator (the spine). Replace VXN1's fixed per-channel
      routing loop with a generic per-block source eval → per-dest accumulate,
      `out[dest] += source·curve(depth)·scale_norm(...)`, reading all ten sources
      (incl. aftertouch pressure from 0198 and random from 0199) into the
      `[lane][source]` table. Preserve VXN1 smoothing (per-sample cutoff coeff
      interp; per-sample pitch). Alloc-free (extend alloc-trap test).
      **Render-parity test:** the seeded default patch renders bit-identical (or
      within float-hash tolerance) to VXN1's default-patch output.
- [ ] **0203** — Persistence. Sparse TOML `[[matrix]]` round-trip (source/dest/
      depth/curve/scale-src kebab keys; inactive slots omitted; absent/unknown →
      None); binary `clap.state` topology packing (VXN2 ADR 0009 byte layout);
      depths via the param blob. Tests: round-trip, sparse omission, back-compat
      default read.
- [ ] **0204** — CLAP shell + bundle. `vxn1b-clap` params/state wiring, stable
      unique plugin id, MPE note/pressure event routing into the engine, no-echo
      mirror + gesture plumbing (reuse VXN1 pattern); `vxn-1b/xtask bundle`
      builds/installs the `.clap`. **DAW-playable with host-generic knobs.**
      `clap-validator` reports 0 failures (incl. the empty-state-load contract
      from 0196).

## Risks

- **Render parity.** If the seeded matrix doesn't equal VXN1's fixed routing
  (smoothing order, key-track curve, VCA path), the variant sounds wrong. 0202's
  parity test against a VXN1 render fixture is the gate — build it first.
- **MPE allocation correctness.** Threading channel through voice stealing is the
  subtle bit — a stolen voice must adopt the new note's channel, and channel
  pressure must not leak to voices on other channels. Cover with allocator tests
  (0198), not just a smoke play. This is the "genuinely novel plumbing", hence
  early placement.
- **Param-table churn.** Removing the fixed-panel depth params and adding slot
  depths reshapes the table; no CLAP id-stability constraint pre-release
  (`vxn1-id-stability-dropped`), but the display/format coverage must stay total.
- **RT discipline.** The generic evaluator must stay allocation-free and keep the
  per-sample cutoff interp — a naive per-sample matrix eval would regress CPU.
  Source eval stays per-block; only dest application is per-sample where VXN1's is.
- **Shared-crate coupling.** Sharing `vxn-dsp` means a VXN1 DSP change can affect
  VXN1b. Acceptable (identical sound is the goal); flagged so DSP edits run both
  synths' tests (`vxn-no-parallel-cargo-test` applies).

## Acceptance

- `vxn-1b` builds in the workspace; `vxn-1b/xtask bundle` installs a `.clap`
  that loads and plays in a DAW with host-generic knobs.
- MPE per-note pressure reaches only its own voice (allocator test); channel
  pressure broadcasts; a stolen voice re-parents its channel. Note-on random is a
  per-voice source, stable across a note.
- Modulation routes through the 16-slot matrix fed by all ten sources; the
  default patch renders bit-identical to VXN1's default (render-parity test).
- Slot depths automate as CLAP params; source/dest/curve/scale_src round-trip
  through TOML and `clap.state` (incl. sparse + back-compat default reads).
- Hot path is allocation-free; `clap-validator` reports 0 failures.
</content>
