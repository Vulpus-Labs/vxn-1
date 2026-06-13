---
id: E008
title: Mod matrix completeness, coherence & UX — wire all sane dests, validate routings, sanify units, bipolar depth fader
status: closed
created: 2026-06-12
closed: 2026-06-13
---

## Goal

Close the gaps in the mod matrix surfaced by the 2026-06-12 review. Three are
correctness/coverage, one is UX:

1. **All sane routings supported.** Five destinations are routable in the UI but
   never consumed in audio — `lfo2-phase`, `lfo1-rate`, `lfo2-rate`,
   `stack-detune`, `stack-spread` (see [matrix.rs:185-196](../../crates/vxn2-engine/src/matrix.rs#L185-L196)
   and [engine.rs:437-439](../../crates/vxn2-engine/src/engine.rs#L437)). A slot
   pointed at any of them cooks a depth, runs the smoother, and drops the result
   on the floor. Wire each one where the routing is *coherent* (see the
   granularity rule below).
2. **Routings validated in the table.** The UI lets a user pick any of 11×29
   source/dest combinations; many are incoherent (a per-lane source can't drive
   a per-stack target — which lane wins?). The matrix table must compute
   coherence per row and render the offending source/dest text **red**, with a
   tooltip saying why, while still letting the slot be set (so old patches load).
3. **Units sanified.** Source outputs mix ranges (`[-1,1]` bipolar, `[0,1]`
   unipolar, raw semitones for `pitch-eg`) and `pitch-eg → *-pitch` double-scales
   (raw semitones × the pitch dest's 24× gain × cubic depth taper). Normalize
   source outputs to documented ranges and recalibrate `DEST_GAIN` so `depth = 1`
   means a sensible full-scale in each dest's native unit.
4. **Depth control is a real fader.** The matrix depth is a bare
   `<input type="range">` ([mod-matrix.js:143-150](../../crates/vxn2-ui-web/assets/panels/mod-matrix.js#L143-L150)) —
   no center bar, no value readout, no double-click entry. Make it a **bipolar**
   fader (fill from center, value-pop on hover/drag, double-click numeric entry,
   shift-drag fine) consistent with every other slider ([fader.js](../../crates/vxn2-ui-web/assets/panels/fader.js)).

When this epic closes: every coherent source→dest pair modulates audio; the
table flags incoherent ones in red; depth is musically uniform across dests; and
the depth control matches the rest of the UI.

## The coherence rule

Sources and destinations each have a **granularity tier**:

| Tier | Sources | Destinations |
| --- | --- | --- |
| **patch-global** (1 value/patch) | `lfo1`, `mod-wheel`, `aftertouch` | `lfo1-rate`, `delay-mix`, `reverb-mix` |
| **per-stack** (1 value/voice) | `pitch-eg`, `mod-env`, `velocity`, `key` | `lfo2-rate`, `stack-detune`, `stack-spread`, `cutoff`, `resonance` |
| **per-lane** (1 value/unison lane) | `lfo2`, `voice-idx`, `voice-spread`, `voice-rand` | `op{1..6}-{pitch,level,pan}`, `global-pitch`, `feedback`, `lfo2-phase` |

A routing is **coherent iff the source tier is coarser-or-equal to the dest
tier** — a coarser source broadcasts unambiguously to a finer dest; a finer
source into a coarser dest is a lossy collapse (which lane/stack wins?).

```text
                 → per-lane dst   → per-stack dst   → patch-global dst
patch-global src      ✓                ✓                  ✓
per-stack src         ✓                ✓                  ✗
per-lane src          ✓                ✗                  ✗
```

Plus two special-case exclusions: an LFO modulating **its own** rate
(`lfo1→lfo1-rate`, `lfo2→lfo2-rate`) is self-referential and excluded even
though tiers permit it. `voice-idx → {lane-0-collapsed dest}` is degenerate
(`voice_idx[0] = 0`, [stack.rs:598](../../crates/vxn2-dsp/src/stack.rs#L598)) —
flagged but tier-legal.

This rule is the single source of truth shared by the wiring (which dests are
worth wiring, and from which sources), the validator (what to paint red), and
the docs. It is formalised in [0090](../../tickets/closed/0090-matrix-granularity-metadata.md);
an ADR (0007) may follow if the unit recalibration needs a recorded decision.

## Scope

**In:**

- Granularity-tier metadata on `SourceId`/`DestId` + a `coherent(src, dst)`
  predicate, exported in the matrix descriptor the UI reads.
- Wire `lfo2-phase` (per-lane phase offset — the deferred supersaw-shimmer route).
- Wire `lfo1-rate` / `lfo2-rate` (one-block-latency to sidestep rate-on-self
  ordering).
- Wire `stack-detune` / `stack-spread` (per-block re-cook, gated so the cost is
  paid only when a slot targets them).
- Normalize source units + recalibrate dest gains; document each dest's native
  unit and full-scale.
- Matrix-table coherence validation: red text + tooltip on incoherent rows.
- Bipolar depth fader (center fill, readout, double-click entry, shift-drag).
- Factory-preset re-audit for dead/incoherent routes + the unit recalibration's
  level impact; matrix tests + benches for the newly-wired dests.

**Out (later / not this epic):**

- Matrix-routing a slot's *depth* from the matrix itself (cycle detection —
  still out per [matrix.rs:53-56](../../crates/vxn2-engine/src/matrix.rs#L53)).
- True per-sample (vs one-block-latency) LFO-rate modulation.
- Per-lane cutoff/resonance (physically precluded by post-lane-fold filter
  placement — ADR 0004; the lane-0 collapse stays, the validator flags per-lane
  sources into it).
- Hard-blocking incoherent routings at set time — they're flagged, not refused,
  so existing patch blobs still load.

## Tickets

- [x] [0090 — Granularity-tier metadata + coherence predicate + descriptor export](../../tickets/closed/0090-matrix-granularity-metadata.md)
- [x] [0091 — Wire `lfo2-phase` per-lane phase-offset dest](../../tickets/closed/0091-wire-lfo2-phase-dest.md)
- [x] [0092 — Wire `lfo1-rate` / `lfo2-rate` dests (one-block latency)](../../tickets/closed/0092-wire-lfo-rate-dests.md)
- [x] [0093 — Wire `stack-detune` / `stack-spread` dests (gated per-block re-cook)](../../tickets/closed/0093-wire-stack-detune-spread-dests.md)
- [x] [0094 — Unit sanification: normalize source outputs + recalibrate dest gains](../../tickets/closed/0094-matrix-unit-sanification.md)
- [x] [0095 — Matrix-table coherence validation: red text on invalid routings](../../tickets/closed/0095-matrix-row-coherence-validation.md)
- [x] [0096 — Bipolar depth fader: center fill, readout, double-click entry](../../tickets/closed/0096-bipolar-depth-fader.md)
- [x] [0097 — Factory-preset re-audit + matrix tests & benches](../../tickets/closed/0097-preset-reaudit-matrix-tests.md)

## Dependency order

```text
0090 (tier metadata + coherence) ──┬─> 0095 (table validation, red text)
                                   └─> (informs which routes are sane)

0091 (lfo2-phase) ──┐
0092 (lfo-rate)    ─┼─> 0097 (preset re-audit + tests/benches)
0093 (stack d/s)   ─┤
0094 (units)       ─┘

0096 (bipolar fader) ── independent UI, no engine dep
```

- 0090 is the metadata foundation; 0095 consumes its exported tiers; the wiring
  tickets (0091-0093) use its coherence definition to know which sources to
  honour for each newly-live dest.
- 0091, 0092, 0093 are independent engine wiring jobs and can run in parallel.
- 0094 (units) touches `eval_sources` + `DEST_GAIN` and is independent of the
  wiring, but lands before 0097 because it changes the sound of existing routes.
- 0096 is pure UI on the depth control and depends on nothing else.
- 0097 is the keystone validation: it needs every sound-affecting change
  (0091-0094) in place to re-audit presets and bench the re-cook cost.

## Acceptance

- Every destination is consumed in `process_block` for at least its
  coarsest-coherent source: `velocity → cutoff`, `mod-env → lfo2-rate`,
  `key → stack-detune`, `voice-rand → lfo2-phase`, `mod-wheel → lfo1-rate` all
  audibly modulate. No dest silently cooks-and-drops.
- `voice-rand → lfo2-phase` produces the documented per-lane phase scatter
  (supersaw shimmer) — the route [lfo.rs:319-323](../../crates/vxn2-dsp/src/lfo.rs#L319)
  claims but never delivered.
- The newly-wired rate/re-cook dests cost **zero** extra when no slot targets
  them (gated); when targeted, the added per-block cost is measured and
  documented, and the off-path bit-identity for unrelated patches holds.
- The matrix table renders incoherent source/dest combinations (per the rule
  above, incl. self-ref and degenerate `voice-idx → lane-0` cases) with **red**
  text + an explanatory tooltip; coherent rows render normally; setting an
  incoherent slot is still permitted (no hard block) and old blobs load.
- `depth = 1` produces a musically comparable full-scale excursion across dest
  kinds; `pitch-eg → global-pitch` no longer double-scales (a 1-semitone EG
  reaches ~1 semitone of global pitch at unity depth, not 24). Each dest's
  native unit + full-scale documented in `PARAMETERS.md` / matrix docs.
- The depth control is a bipolar fader: fills from center, shows the depth
  amount on hover and during drag, resets/sets via double-click, fine-adjusts on
  shift-drag — routing through the existing slot-1-8 CLAP / slot-9-16 opcode
  dispatch unchanged.
- Every factory preset re-audited: no slot points at a dead or incoherent dest;
  unit recalibration's effect on levels reviewed (documented DAW A/B is the
  user's check).
- No RT allocations / `unwrap` / `expect` / panics added to the process
  callback. Each ticket's own acceptance criteria met.
