---
id: E033
product: vxn-2
title: "Mod-matrix per-slot scale source — VCA any route's depth by any source"
status: open
created: 2026-07-03
---

## Goal

Give every mod-matrix slot an optional **secondary scale source** that
multiplicatively gates that slot's depth. Today the matrix is purely
*additive*: a `MatrixSlot { source, dest, depth, curve }` adds `source × depth`
into a dest. There is no way to make one route's amount *depend on* another
control — e.g. an LFO→pitch vibrato whose depth is driven by the mod wheel
(the classic DX7 mod-wheel-vibrato, which our factory translations can't
express — see the E.PIANO 1 investigation).

When this epic closes:

- Each slot gains a `scale_src: SourceId` field. `None` (default) = today's
  behaviour exactly (depth unscaled).
- With `scale_src` set, the slot's per-lane contribution is multiplied by the
  **normalised** scale-source value in `[0, 1]`: `0 → route contributes 0`,
  `1 → full configured depth`.
- Any source is selectable as a scale source (mod wheel, aftertouch, velocity,
  LFO, envelopes, voice sources) — the same roster as the primary source.
- Patch state only (topology, like `source`/`dest`/`curve`); **not** a new
  CLAP-automatable param. Presets and binary state round-trip it, back-compat.
- The multiply is one op per slot·lane in `eval_dests`, reading the scale-source
  value from the **existing** `[lane][source]` table — no new plumbing, no
  allocation on the audio thread.

## Why now

The additive-only matrix can't model performance-controlled modulation depth,
which is table-stakes for expressive patches and a prerequisite for faithfully
translating DX7 voices that arm mod-wheel vibrato (`pms > 0`, `pmd = 0`). The
DSP cost is a single multiply against a value we already compute, so the whole
feature is cheap; the work is in the data model, serde, and UI, not the hot
path.

## Design (locked)

- **Field.** `MatrixSlot.scale_src: SourceId`, default `SourceId::None`. `None`
  is identity — the eval multiplies by `1.0`, so a patch with no scale sources
  is bit-identical to today.
- **Normalisation → [0, 1].** The scale factor is unipolar. Unipolar sources
  (`mod_wheel`, `aftertouch`, `velocity`, `key`) pass through as-is. **Bipolar**
  sources (LFOs, `pitch_eg`, `voice_spread`, `voice_rand`) map `(x + 1) × 0.5`
  then clamp `[0, 1]`, so a centred LFO gates at half and swings the route
  0↔full. One shared `scale_norm(SourceId, value)` helper, documented in the
  ADR.
- **Granularity.** `eval_dests` already holds every source value in a
  `[lane][source]` table at the correct per-lane granularity (engine.rs). The
  scale-source value is read from that table at the slot's own lane — so a
  finer scale source correctly gates a coarser dest with no extra broadcast
  logic. No coherence special-casing.
- **Not automation.** `scale_src` is topology, so it follows `source`/`dest`/
  `curve`: patch state, not a `clap.params` entry. No wire-format param id
  churn; only the patch/state blob grows one field per slot.
- **Back-compat.** Absent TOML key → `None`; unknown source name → `None`
  (mirrors `SourceId::from_u8`). Binary state blob version bump with a
  defaulted read path.

## Planned tickets

Dependency chain: **0175 → 0176 → { 0177, 0178 }**.

- [ ] **0175** — Data model + patch/state serde. Add `scale_src: SourceId` to
      `MatrixSlot` (default `None`); TOML round-trip (`slotN-scale-src` via
      `SOURCE_NAMES`); `default_patch`; versioned binary state blob with a
      back-compat defaulted read. No eval behaviour change yet (`None` = identity
      is already the eval default). Tests: round-trip, absent→None, unknown→None.
- [ ] **0176** — Hot-path scale in `eval_dests`. Add `scale_norm(SourceId, f32)`
      (unipolar passthrough; bipolar `(x+1)×0.5` clamp). Multiply each slot's
      per-lane contribution by the normalised scale-source value from the
      `[lane][source]` table; `None → 1.0`, branch-light. Extend the alloc-trap
      test. Tests: wheel-gated route = 0 at wheel 0, full at wheel 1; bipolar
      mapping; `None` regression vs baseline render hash.
- [ ] **0177** — Faceplate `mod-matrix.js` scale-source column. Per-slot "Scale"
      selector reusing the source dropdown with a `—`/None default; MVC
      discipline (view emits a scale-src change event, never reads the model);
      contract/token tests in the `vxn2-ui-web` suite.
- [ ] **0178** — Integration + docs + demo preset. ADR note (matrix scale-source
      semantics + normalisation table); PARAMETERS.md / README update; a demo
      preset with wheel-gated LFO vibrato (the EP1 case). Confirm `clap.state`
      round-trips through save/reload and `clap-validator` stays clean (no new
      params). Close-out.

## Risks

- **Bipolar scale semantics.** `(x+1)×0.5` is a choice, not the only one (abs,
  clamp-to-positive). Lock it in the ADR and cover with a test so it isn't
  silently re-interpreted later. Unipolar sources (the common case: wheel /
  aftertouch / velocity) are unambiguous.
- **RT discipline.** The multiply must stay allocation-free and out of the lane
  inner loop's curve dispatch. Read the scale value once per slot·lane from the
  table already in hand; extend the alloc-trap test rather than trusting review.
- **State back-compat.** Old patches/projects lack the field — the blob read
  must default to `None` (identity), verified by loading a pre-epic state fixture.
- **UI clutter.** A second per-slot source column on a 16-slot matrix is dense;
  the `—`/None default must read as "off" at a glance so existing patches look
  unchanged.

## Acceptance

- Every slot has a selectable `scale_src`; `None` renders bit-identical to the
  pre-epic engine (regression hash).
- A slot with `scale_src = mod-wheel` contributes 0 at wheel 0 and full depth at
  wheel 1; a bipolar scale source follows the locked `(x+1)×0.5` mapping.
- Scale source is chosen from the full source roster in the faceplate; the view
  never reads the model (MVC parity test).
- Presets (TOML) and `clap.state` round-trip `scale_src` through save/reload; a
  pre-epic state fixture loads with `scale_src = None`.
- Hot path is allocation-free; `clap-validator` reports 0 failures; no new
  `clap.params` ids.
- A demo preset ships wheel-gated LFO vibrato, audibly off at wheel 0.
