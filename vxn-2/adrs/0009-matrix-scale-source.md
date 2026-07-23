# ADR 0009 — Mod-matrix secondary scale source

- **Status:** Accepted
- **Date:** 2026-07-21
- **Scope:** Give every mod-matrix slot an optional **secondary scale source**
  that multiplicatively gates the slot's depth — a per-route VCA. Epic E033
  (tickets 0175–0178). Extends the mod matrix of
  [ADR 0001 §6](0001-vxn2-overall-design.md); adds no new destinations, no new
  CLAP params, and does not touch the algorithm graph.

## Context

The matrix was purely **additive**: a `MatrixSlot { source, dest, depth, curve }`
adds `source · curve(depth)` into a destination, and multiple slots to the same
dest sum. There was no way to make one route's *amount* depend on another
control — the classic case being a mod-wheel-controlled vibrato: `lfo → pitch`
whose depth rides the mod wheel. On a DX7 this is `pms > 0, pmd = 0` (LFO armed,
depth driven by the wheel); the additive-only matrix could not express it, so
our DX7 factory translations (e.g. E.PIANO 1) dropped it.

The obvious "route a source into the slot's depth" was deliberately excluded in
v1 because routing a *dest output* back into a depth invites cycles and forces
cycle detection into the matrix engine.

## Decision

Add one field, `MatrixSlot.scale_src: SourceId` (default `None`). When set, the
slot's per-lane contribution is multiplied by the **normalised** value of the
scale source:

```
out[dest] += source · curve(depth) · scale_norm(scale_src, scale_value)
```

Key properties:

- **Leaf value, no cycles.** The scale source is read from the *same*
  `[lane][source]` table as the primary source — a value computed **before**
  matrix evaluation. It is never a dest output, so it can never form a cycle.
  This is why a secondary *source* is safe where a matrix-routed *depth* was not.
- **Identity default.** `None` multiplies by exactly `1.0`, so a patch with no
  scale sources renders bit-identically to the pre-E033 engine (guarded by a
  render-equality test).
- **Unipolar VCA, `[0, 1]`.** `scale_norm` maps the scale value to a `[0, 1]`
  gain — `0` = route contributes nothing, `1` = full configured depth:

  | Polarity | Sources | Mapping |
  | --- | --- | --- |
  | unipolar (`[0, 1]`) | `mod_wheel`, `aftertouch`, `velocity`, `key`, `mod_env`, `voice_idx`, `voice_rand` | `x` |
  | bipolar (`[-1, 1]`) | `lfo1`, `lfo2`, `pitch_eg`, `voice_spread` | `(x + 1) · 0.5` |

  Both clamped to `[0, 1]`. A centred bipolar source therefore gates at half and
  swings the route `0 ↔ full`.
- **Granularity is free.** The scale value is read at the slot's own lane from
  the existing per-lane source table, so a finer scale source correctly gates a
  coarser dest with no extra broadcast logic and no coherence special-casing.
- **Patch state, not automation.** `scale_src` follows `source`/`dest`/`curve`:
  it is patch topology, **not** a `clap.params` entry. No new automatable ids;
  `clap-validator` sees zero param-count change.

### `voice_rand` polarity (deviation from the planning doc)

The E033 planning doc grouped `voice_rand` with the bipolar sources. Its actual
runtime range is `[0, 1)` (unipolar); applying `(x + 1) · 0.5` would compress it
to `[0.5, 1)` and never gate to zero. It is therefore classified **unipolar**
(passthrough) here — polarity follows each source's real range, decided
exhaustively in `SourceId::is_bipolar` so a new source forces a decision at
compile time.

## Persistence

`scale_src` round-trips through both persistence paths with full back-compat:

- **TOML presets** — a per-slot `scale-src` kebab key, omitted when `none`
  (sparse); an absent key or unknown name decodes to `None`.
- **Binary `clap.state`** — packed into the previously-reserved low-byte bits of
  the slot's `matrix_meta` word (bit 0 stays `active`; the scale source rides
  bits 1–7). The blob stays byte-for-byte the same size, so a **pre-E033 blob**
  (those bits zero) decodes to `scale_src = None` with **no version bump** — an
  explicit choice over bumping `BLOB_VERSION`, since the one-version-only load
  policy (ticket 0120) would otherwise *reject* older blobs outright.

The web wire (`EV_MATRIX_ROW`) carries the scale source in the slot's reserved
byte 12; the controller view snapshot appends one `u8` per row.

## Consequences

- One extra multiply per slot·lane in `eval_dests`, against a value already in
  hand — allocation-free, out of the curve dispatch. Negligible CPU.
- Performance-controlled modulation depth is now expressible (mod-wheel vibrato,
  aftertouch-swept filter, velocity-scaled anything) — table stakes for
  expressive patches and DX7 fidelity.
- The *EP Wheel Vibrato* factory preset (Keys) ships as the demo/acceptance
  artefact: dead-flat at wheel 0, ~0.65 st vibrato at wheel up.
