---
id: "0284"
product: vxn-1b
title: "Stack start-phase depth and lane distribution law"
priority: medium
created: 2026-08-24
epic: null
depends: [0283]
---

## Summary

VXN1b's stack voicing hardcodes two things VXN2 exposes as macros.

**Start phase.** `Voices::stack_phase` returns a *fully random* draw the moment a
stack is more than one lane wide, and `None` at width 1
([voice.rs:547-550](../../vxn-1b/crates/vxn1b-engine/src/voice.rs#L547-L550)). It
is all or nothing: a two-lane stack can be phase-locked (width 1's deterministic
`lane_phase`) or fully scattered, with nothing in between. VXN2 has the
in-between as a knob — `StackParams::phase` scales the per-lane random offset,
so 0 is a coherent onset and 1 is today's scatter
([stack.rs:1067-1088](../../vxn-2/crates/vxn2-dsp/src/stack.rs#L1067-L1088)).

**Distribution.** `stack_spread(i, width)` is linear only — evenly spaced in
`[-1, +1]` ([voice.rs:60-66](../../vxn-1b/crates/vxn1b-engine/src/voice.rs#L60-L66)).
VXN2 offers Linear / Geometric / Random over the same quantity
([stack.rs:62-72](../../vxn-2/crates/vxn2-dsp/src/stack.rs#L62-L72)). The one
position feeds both the detune fan and the stereo fan in VXN1b (`pos *
unison_detune` and `stack_pos → spread_pos`), exactly as VXN2's `voice_spread`
does, so the law transfers without reinterpretation.

Add `stack_phase` and `stack_distrib` as per-layer params with faceplate cells
on the Voice panel.

## Design

- **Params.** `f("stack_phase", "Phase", 0.0, 1.0, 1.0, "", Taper::Linear)` and
  `e("stack_distrib", "Distrib", DISTRIB_LABELS, 0.0)` in the `// ── Voice ──`
  block, `PATCH_PARAMS: [ParamId; 73]` → `75`. Phase defaults to **1.0**, not
  0.0: 1.0 is what the engine does today, so no existing patch changes.
  `StackDistrib` enum beside `StackWidth`, labels `["Lin", "Geo", "Rnd"]`.
- **Phase.** `stack_phase(width, amount)` still draws from `phase_rng` on every
  stacked lane whatever the knob reads — scaling *after* the draw keeps the
  stream deterministic, so turning the knob cannot shift the sequence a later
  note sees. Amount 0 lands every lane on 0.0 (coherent), matching VXN2 where
  phase 0 leaves only the per-op static offset. Width 1 keeps returning `None`
  → `lane_phase(v)`, so poly transient decorrelation is untouched.
- **Distribution.** Random needs a fresh draw per lane per note-on, so it cannot
  stay a pure `fn(i, width)`. Replace with `Voices::fill_stack_pos(width,
  distrib) -> [f32; N]` computed once per note-on. Geometric is VXN2's
  `sign(t) · |t|^0.5`; Random is `2·draw − 1` off a **third** RNG stream with its
  own seed — the same rationale the `PHASE_SEED` comment already gives
  ([voice.rs:46-50](../../vxn-1b/crates/vxn1b-engine/src/voice.rs#L46-L50)):
  adding or removing a stacked note must not shift the other two streams.
- **Signatures.** `note_on_stack` / `note_off_stack` already carry four voicing
  arguments; two more makes eight positional. Bundle into `StackVoicing { width,
  mode, unison_detune, legato, phase, distrib }`, the same call `TriggerOpts` made
  in 0283.
- **Faceplate.** A `fader` for Phase and a `buttongroup` for Distrib on the Voice
  panel beside Spread. The panel is already the tallest in its row — check the
  row still aligns under the headless layout probe.
- **Dim.** Phase is inert when *both* oscillators free-run: a free-running
  oscillator ignores the stamped `start_phase` entirely
  ([bank.rs:745-757](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L745-L757)), so
  with both flags on nothing consumes the value. One flag on and the other off
  leaves the knob live on the locked oscillator, so the rule is an AND of two
  watches — which `BUILTIN_DIM_SPECS` cannot express today (one `watch` name per
  spec). Generalise `watch` to accept a list: one rule per watch id sharing a
  predicate that reads the cached values of all of them. `model.lastParam` is
  written before `applyDimRulesFor` runs, so the predicate always sees the
  echo that triggered it.
- **State.** Two more ids lengthen the positional param block: `VERSION` 11 → 12,
  older blobs rejected per the no-migration policy. Preset TOMLs are name-keyed
  and sparse, so they load unchanged and read the defaults.

### Interactions

1. **Random distrib randomises detune *and* pan**, because one `pos` drives both.
   VXN2 has the identical coupling, so this is consistency rather than a
   compromise — but it means Random stacks are not repeatable note to note, and
   `SourceId::Spread` (which reads `stack_pos`) becomes a random source under it.
2. **`spread`** (the existing pan-depth knob) is unchanged and still scales
   `stack_pos`. Distrib decides *where* the lanes sit, `spread` how far out.
3. **Free-run** as above; also means the Phase knob does nothing on a layer whose
   both oscillators drift, which is what the dim rule communicates.

## Acceptance criteria

- [ ] `stack_phase` / `stack_distrib` exist as per-layer params (defaults 1.0 /
      Lin) and appear in `PATCH_PARAMS`; `PARAMS.len() == ParamId::COUNT` and the
      partition test pass with the new count.
- [ ] Phase 1.0 reproduces today's start phases exactly (test: same seed, same
      draws as the pre-change path).
- [ ] Phase 0.0 starts every lane of a stack at 0.0; width 1 still stamps
      `lane_phase(v)` at any phase value.
- [ ] Turning the phase knob does not change the `phase_rng` sequence a
      subsequent note draws from.
- [ ] Linear reproduces the existing positions; Geometric pushes inner lanes
      toward 0 with the outer lanes still at ±1; Random fills `[-1, 1]` from a
      stream independent of `rng` and `phase_rng`.
- [ ] Phase cell dims iff both `osc1_free_run` and `osc2_free_run` are on, per
      layer, and follows a layer tab flip.
- [ ] `clap.state` version bumped, round-trip covers the new params.
- [ ] vxn1b parity oracle green with no rebaseline; `cargo test -p vxn1b-engine`
      and the ui-web vitest suite green.

## Notes

- Related: [[vxn1b-two-layer-param-map]] — each patch param costs two outer CLAP
  ids, so the surface goes 181 → 185; [[vxn-faceplate-layout-probe]] for the row
  alignment check.
- Out of scope: VXN2-style per-op static phase offsets (VXN1b has no ops), a
  matrix destination for either param, and any change to `lane_phase` or to
  `spread` itself.

## Close-out (2026-08-24)

- `StackPhase` / `StackDistrib` added to the `// ── Voice ──` block, per layer
  ([params.rs:672](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L672)). Phase
  defaults to 1.0 and Distrib to `Lin`, so nothing an existing patch does moves.
  `PATCH_PARAMS` 73 → 75 and the outer CLAP surface 181 → 185 (`2 × 75 + 35`).
- `StackVoicing` replaces the four positional voicing arguments on
  `note_on_stack` / `note_off_stack`
  ([voice.rs:100](../../vxn-1b/crates/vxn1b-engine/src/voice.rs#L100)), built once
  per note event by `Synth::voicing()`. The 55 test call sites go through a
  `voicing(width, mode, detune, legato)` helper over the struct's defaults, so
  every pre-0284 test still exercises what it used to.
- Phase scales the draw rather than replacing it, and width — not depth — still
  gates whether a draw happens at all. That is what keeps the stream stable
  across knob positions, which `stack_phase_depth_does_not_shift_the_random_stream`
  pins by dividing the phases back out at two different depths.
- `fill_stack_pos` computes the whole layout once per note-on because Random has
  to draw; Linear and Geometric stay pure. Random pulls from `spread_rng`
  (`SPREAD_SEED`), the third stream — `random_distrib_does_not_disturb_the_other_streams`
  asserts a Random note leaves the start phases *and* the note-random values
  identical to a Linear one.
- Faceplate: Phase fader + Distrib buttongroup after Spread. Dropping two cells
  into the panel at the old width squeezed every column — "DETUNE GLIDE SPREAD
  PHASE" ran together with no gap and "CROSS MOD" wrapped onto its first
  variant. Retuning the grow ratio fixed that but left ~58px of dead panel to the
  right of Cross Mod, so the row abandoned the pure-ratio scheme instead: Voice
  is `flex: 0 1 auto` (natural width) and Scope takes the whole remainder. That
  also takes the tuned number out of the stylesheet — the panel's width has now
  changed twice, and each time the hand-picked ratio was wrong in one direction
  or the other. Probe after: Voice 525 wide with a 0.0px trailing gap, Scope 509,
  both `h=184 bot=606`, no overflow, row still aligned.
- Follow-on from that: a column is exactly as wide as its label's max-content
  size, with no slack, so whether a two-word label sits on one line came down to
  the renderer's font metrics — "CROSS MOD" and "FM AMT" fit in Chrome and
  wrapped in the plugin's web view, and since `--ctl-label-h` is one line's
  worth the second line painted over the control beneath it. `.ctl-label` is now
  `white-space: nowrap`, which settles it in the layout rather than in the font:
  the column asks for the width the text needs, and Voice's natural width
  absorbs it. Stress-checked by forcing `letter-spacing: 1.6px` (3.2× the real
  value) — zero wrapped or overflowing labels across all 17 panels, Voice grows
  525 → 574 and the scope gives up the difference.
- `BUILTIN_DIM_SPECS` learned list-valued `watch`: one rule per watched id, one
  shared predicate. The `stack-phase` spec ANDs both osc free-run flags off the
  `model.lastParam` cache rather than the echoed value, since only one of the two
  ever arrives at a time. `stack-phase-dim.test.js` covers the fan-out, the AND,
  the cache read, the missing-echo case and the layer offset.
- State `VERSION` 11 → 12; `roundtrips_both_layers_independently` sets both new
  params asymmetrically across the layers.
- `gen_parameters_doc`'s group table needed `("Voice", 6)` → `8` — it asserts the
  spans tile the bank, which caught the omission on the first run.
- Tests: 295 engine unit + 27 integration, 292 ui-web JS (287 + 5), parity oracle
  unmoved, no new clippy warnings (the old 9-argument `note_on_stack` warning is
  gone with it).
- Verified by ear in Reaper (2026-08-24): phase 0 vs 1 on a wide detuned stack
  and Geometric / Linear / Random at width 8+ all behave as specced.

## Addendum — Voice panel regroup (2026-08-24)

Not part of 0284's scope but landed with it, since the panel was already being
re-laid out:

- Width runs 3×2 read across (1 · 2 · 4 over 8 · 16 · 32) instead of 2×3 read
  down. The multi-column button grid grew a row-major variant for it
  (`data-flow="row"` beside `data-columns`, with `makeButtonGroup` now setting
  `--ctl-cols` as well as `--ctl-rows` — a row-major grid has to be told the
  count the flow does not derive). The grid keeps **three** template rows for
  its two rows of buttons: the empty third holds the space the third row of
  widths used to occupy, so Poly/Solo and Legato do not slide up into it. They
  are at exactly their old y (515 / 533) with the group still 61 tall.
- Width, Poly/Solo and Legato now stack in one `.ctl-col`. The rocker is
  horizontal there (`data-orient="h"`, a new CSS variant — every other rocker
  stays vertical) and stretches to the column, and the Width grid stretches to
  match it so its two button columns land under POLY and SOLO. Legato sits
  under that at its natural width.
- Legato stopped being drawn by the `detune-legato` composite and became a plain
  `switch` cell. The composite existed only to hold it and to dim it under Poly:
  with the dim moved to a `legato-poly` `BUILTIN_DIM_SPECS` entry, `unison_detune`
  is a plain fader and `makeDetuneLegato` (125 lines), the `detune-legato`
  control kind, its `entry.extras` plumbing, the `.ctl-detune*` CSS and the dead
  `TWIN_TOP_CT` export are all gone. Legato is `dimmed` rather than `disabled`
  now — the house idiom, so it stays clickable while greyed like every other
  gated control.
- Legato also left the panel's bottom-left corner, where it was absolutely
  positioned; nothing is absolutely placed in the Voice panel any more.
- Voice 526 wide (525 before the regroup), Scope 508, row still
  `top=422 bot=606`, no overflow in either axis, trailing gap 0.
- `--tg-box` is now a token (was an 11px literal on `.ctl-tg-box`), because the
  reserved row's height is derived from it rather than guessed alongside it.
- `css_covers_every_control_primitive` listed `.ctl-detune` / `.ctl-detune-legato`;
  it now asserts `.ctl-col`. New suite `legato-dim.test.js` (3 cases) covers the
  dim, its name-not-index variant lookup, and the layer offset the composite used
  to resolve by hand. 295 JS tests, 12 ui-web Rust tests green.
