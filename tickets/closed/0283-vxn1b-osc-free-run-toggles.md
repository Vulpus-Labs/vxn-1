---
id: "0283"
product: vxn-1b
title: "Osc 1 / Osc 2 free-run toggles"
priority: medium
created: 2026-08-23
epic: null
depends: [0281]
---

## Summary

Both oscillators are hard-reset at note-on. `trigger_lane` calls
`PolyOscillator::reset(v)` on each and stamps a start phase — `lane_phase(v)`
under Poly/Solo/Twin, or the caller's `start_phase` for unison decorrelation
([bank.rs:712-718](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L712-L718)). Every
note therefore begins at a known phase, which is what makes attacks repeatable
and Twin's two lanes beat predictably.

It also means the oscillators can never *drift* — the thing that makes a real
divider-less analog poly sound alive on repeated notes, and the thing a player
reaches for when a plucked attack sounds too identical each time. LFO 1 already
has this escape hatch as a per-layer switch (`lfo1_free_run`,
[params.rs:654](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L654), consumed at
[bank.rs:713-716](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L713-L716)); the
oscillators do not.

Add `osc1_free_run` / `osc2_free_run` as per-layer boolean params with toggle
buttons on the Osc 1 and Osc 2 faceplate panels. When on, that oscillator's
phase accumulator is left alone at note-on and free-runs across notes.

## Design

- **Params.** Two `b(...)` descriptors beside the other osc params, `ParamId`
  variants `Osc1FreeRun` / `Osc2FreeRun` added to the `// ── Osc / mixer ──`
  block and to `PATCH_PARAMS` (17 → 19 in the osc group, `PATCH_PARAMS: [ParamId; 71]`
  → `73`). Default `0.0` — reset-at-note-on stays the behaviour you get without
  touching anything. Per-layer, like every other osc param, so L1 can drift while
  L2 stays locked.
- **Trigger path.** `RenderBank::trigger_lane` takes the two flags; `Synth::fire`
  reads them once per trigger batch alongside `Lfo1FreeRun`
  ([synth.rs:197-203](../../vxn-1b/crates/vxn1b-engine/src/synth.rs#L197-L203)).
  The signature reaches 6 args — bundle the LFO shape/free-run and the two osc
  flags into a small `TriggerOpts` rather than growing the positional list.
- **Reset semantics.** Free-run must still clear the oscillator's *sync* state:
  `sync_resid` / `sync_pending` describe a deferred sub-sample reset owed to the
  previous note, and carrying one across a note-on emits a stale residual on the
  first sample. `PolyOscillator::reset` zeroes those **and** the phase, so add
  `reset_keep_phase(v)` in `vxn-dsp` next to it and call that when the flag is
  set. New method, no existing caller changes — vxn-1 is untouched.
- **Faceplate.** A `panel-strip` on each of the Osc 1 / Osc 2 panels holding
  `<div class="ctl-strip" data-control="switch" data-param="oscN_free_run"
  data-label="Free">`, mirroring LFO 1's Free
  ([faceplate.html:195-198](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.html#L195-L198)).
  Neither Osc panel has a strip today, so both grow by one strip's height —
  check Row 1 still aligns (Mixer and Filter sit beside them) with the headless
  layout probe before calling it done.
- **State.** The `clap.state` param block is positional (`f32 × ParamId::COUNT`),
  so two new ids change the layout: bump `version` 2 → 3 and let older blobs be
  rejected, per the no-migration-pre-release policy
  ([state.rs:18-35](../../vxn-1b/crates/vxn1b-engine/src/state.rs#L18-L35)).
  Preset TOMLs are name-keyed and sparse, so existing presets load unchanged and
  simply read the default.

### Interactions

1. **Sync.** Under `CrossModType::Sync` osc1 is the slave and osc2's wraps reset
   it regardless, so `osc1_free_run` is close to inert there while
   `osc2_free_run` is the one that bites. Worth dimming Osc 1's Free when the
   mode is Sync — the `BUILTIN_DIM_SPECS` `free-run` kind already models exactly
   this shape ([dispatch.js:189-195](../../vxn-1b/crates/vxn1b-ui-web/assets/dispatch.js#L189-L195)).
2. **Unison / Twin.** A free-running oscillator ignores the `start_phase` the
   allocator hands it, so Twin's two lanes stop starting phase-locked. That is
   the point of the switch, but it means Twin's beating becomes non-repeatable
   with it on — note it in the close-out rather than trying to preserve both.
3. **Sub-osc.** Nothing. Since [0281](0281-sub-osc-own-phase-accumulator.md) the
   sub owns its accumulator and reads only the source's increment, so its onset
   stays deterministic whatever the oscillators do. Before 0281 this ticket would
   have made sub attacks random — hence `depends: [0281]`.

## Acceptance criteria

- [ ] `osc1_free_run` / `osc2_free_run` exist as per-layer bool params, default
      off, and appear in `PATCH_PARAMS`; `PARAMS.len() == ParamId::COUNT` test
      still passes.
- [ ] Flag off: note-on resets that oscillator's phase exactly as today (test:
      two notes on the same lane render bit-identically).
- [ ] Flag on: note-on leaves the phase where it was (test: trigger a lane twice
      with a gap of a non-integer number of cycles, assert the second note's
      first sample differs and the phase is continuous across the gap).
- [ ] Free-run still clears `sync_pending` / `sync_resid` (test: engage sync,
      trigger mid-reset, assert no residual on the first sample of the new note).
- [ ] Toggles render on both Osc panels and drive the right per-layer CLAP id on
      each layer tab; Row 1 still aligns under the layout probe.
- [ ] `clap.state` version bumped to 3, round-trip test covers the new params.
- [ ] Default patch unchanged: vxn1b's parity oracle
      ([parity.rs](../../vxn-1b/crates/vxn1b-engine/tests/parity.rs)) stays green
      with no rebaseline.
- [ ] `cargo test -p vxn-dsp -p vxn1b-engine` and the ui-web vitest suite green.

## Notes

- **vxn-1 not in scope.** The `reset_keep_phase` addition is shared (`vxn-dsp`),
  but vxn-1 keeps its own param table and faceplate and gets no toggle here. If
  it wants one later it is a separate ticket and a separate param-table edit.
- Related: [[vxn1b-two-layer-param-map]] for the outer L1/L2 CLAP surface the two
  new patch params fan out into (each patch param costs two outer ids), and
  [[vxn-faceplate-layout-probe]] for the headless Row-1 alignment check.
- Out of scope: a *global* free-run switch, per-oscillator start-phase randomise
  (the follow-up 0281's notes describe for ring-mod tonality), and any change to
  `lane_phase` itself.

## Close-out (2026-08-23)

- `Osc1FreeRun` / `Osc2FreeRun` added to the osc block, per layer, default off
  ([params.rs:598](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L598)).
  `PATCH_PARAMS` 71 → 73, so the outer CLAP surface is 181 (`2 × 73 + 35`) —
  `patch_and_global_partition_every_param`'s literal updated to match.
- `TriggerOpts` carries the LFO shape, LFO free-run and `osc_free_run: [bool; 2]`
  into `trigger_lane` ([bank.rs:355](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L355)),
  read once per trigger batch in `Synth::fire` so a chord's notes cannot disagree.
  A `TriggerOpts::retrig(shape)` constructor covers the 33 bank tests that just
  want the default.
- Free-run branch keeps the accumulator and ignores `start_phase`; the locked
  branch resets and stamps `lane_phase(v)` exactly as before.
- `PolyOscillator::reset_keep_phase(v)` clears `sync_resid` / `sync_pending`
  without touching phase; `reset` now delegates to it
  ([oscillator.rs:299](../../vxn-1/crates/vxn-dsp/src/poly/oscillator.rs#L299)).
  Tested at the DSP level rather than through the engine — the fields are private
  to `vxn-dsp`, so `reset_keep_phase_clears_sync_state_but_not_phase` arms a real
  deferred reset via `process_sync` and asserts phase survives while both
  residuals zero.
- Faceplate: `Free` switch in a new `panel-strip` on both Osc panels, LFO 1's
  idiom. Row 1 still aligns — headless probe reports all four panels at
  `top=118, h=144, bot=262`; the Osc panels had slack the strip filled, Filter's
  `TUNED` strip was already setting the row height.
- `sync-slave` dim spec greys Osc 1's Free under Cross Mod = Sync, where osc1 is
  the slave and osc2's wraps reset it regardless
  ([dispatch.js:196](../../vxn-1b/crates/vxn1b-ui-web/assets/dispatch.js#L196)).
  Osc 2's Free stays live.
- State `VERSION` 10 → 11 (the ticket's "3" read a stale doc comment);
  `roundtrips_both_layers_independently` now sets the two flags asymmetrically
  across layers so it cannot pass by symmetry.
- Tests: `free_run_off_stamps_the_lane_phase`,
  `free_run_leaves_that_oscillators_accumulator_running` (per-osc independence +
  `start_phase` ignored), `free_run_does_not_reach_the_sub`, and
  `osc-free-run-dim.test.js` (3 cases, incl. layer offset). 17 Rust suites and
  287 JS tests green; vxn1b parity oracle unmoved, no rebaseline.
- Twin's two lanes no longer start phase-locked when the flag is on, as designed
  — its beating becomes non-repeatable with free-run engaged.
