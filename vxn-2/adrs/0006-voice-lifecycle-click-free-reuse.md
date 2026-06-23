# ADR 0006 — Voice lifecycle & click-free voice reuse

- **Status:** Accepted
- **Date:** 2026-06-23
- **Scope:** How a voice is reused when a new note needs an already-sounding
  stack — Solo note changes and Poly voice stealing — and the explicit voice
  lifecycle that drives it. Adds `VoicePhase` to `vxn2_dsp::stack::Stack`,
  reworks the Solo/Poly paths in `vxn2_engine::alloc`, and a declick fade built
  on the existing per-op EG. Touches nothing about the algorithm graph, the mod
  matrix, or the audio inner loop.

## Context

Solo lines (legato off) clicked on fast note changes — worst with voice
stacking (density > 1) and stack phase 0.5 (maximal per-lane phase
decorrelation). A run of in-place fixes each removed one click source and
exposed another:

1. Re-pitching the held voice in place (`retarget_pitch`) but **retriggering
   every operator EG** re-attacks modulators that have decayed to sustain 0
   while the carrier is still loud → an unmasked FM-index transient on every
   re-struck note (the FLUTE 2 16th-note case).
2. Retriggering only the **carrier** EGs cleared that for simple patches, but a
   stacked patch at phase 0.5 still clicked: any in-place reuse re-cooks against
   decorrelated per-lane oscillator phases / running LFO2 / feedback, and
   *something* is always discontinuous.

The root realisation: **a fresh voice onsetting from silence is already
click-free** (the engine ramps a fresh voice's level up from ~0 over its first
block — the onset-ramp from the EG-level work). The hard part is not the new
note; it is making the *outgoing* note disappear without a click, and doing so
without resetting oscillator phase or modulation on a still-sounding voice.

A relevant constraint: the CLAP wrapper always drives the engine in fixed
32-sample **control blocks** (`CONTROL_BLOCK`), slicing the host buffer, so any
"one control block" duration is ~0.67 ms regardless of the DAW's buffer size.

## Decision

### 1. Explicit voice lifecycle

Every voice carries a single `VoicePhase` — `Idle / Held / Releasing /
Declick` — as the authoritative liveness state, abstracting over the six per-op
ADSR stages (a `Held` voice can have ops simultaneously in attack, decay, and
sustain, so the allocator must **not** infer liveness by scanning per-op
stages). `is_idle()` reads the phase. `gate` is retained as a synced mirror of
`phase == Held` (read by the allocator and tests). The `Stack` self-retires in
`eg_tick` — when all amp EGs reach Idle it sets `phase = Idle` — so liveness is
self-consistent whether the stack is driven by the engine or standalone.

### 2. Solo: crossfade to a fresh voice, declick the old one

A Solo note change **round-robins to a fresh slot** (via the existing
`pick_slot` machinery) and **kills** the previous voice. The new note onsets
from silence (click-free); the old note fades out, overlapping. The fixed
`SOLO_SLOT = 0` is gone — `solo_slot: Option<usize>` tracks the live voice, and
`solo_last_pitch` stashes the last pitch so detached-note portamento (ADR 0001
glide semantics) survives the slot being freed/reused.

**Exception — legato + previous still `Held`:** re-pitch the same voice in
place (`retarget_pitch`, no retrigger, no kill). A `Releasing` previous note is
not held, so it falls through to the crossfade. The held-note note-off fallback
uses the same crossfade.

### 3. Declick = forced fast EG release, not a parallel gain

Killing a voice does **not** introduce a separate fade multiplier in the render
paths. Instead it drives every op EG into a fast release to 0 over a fixed
`DECLICK_SECS` (~5 ms) and lets the **existing EG path + per-sample level
smoothing** (the 0077 mechanism that already makes a normal note-off release
click-free) do the fade. `EgState::kill_release` overrides the release
target/rate; the allocator's `start_declick` calls it on every op and sets
`phase = Declick`.

Settled details:

- **Proportional rate.** Each op's release rate is `level / DECLICK_SECS`, so
  every op reaches 0 *simultaneously* and the voice fades **proportionally** —
  identical to a uniform gain fade, with the timbre held, but expressed through
  the EG.
- **~5 ms, from the host sample rate.** A one-control-block (0.67 ms) fade is
  measurably too sharp (a 4th-difference transient ~0.06, ~12× the click-free
  floor); ~5 ms clears it (~0.006) while staying far short of polyphony
  exhaustion. Because the control block is fixed at 32 samples, 5 ms
  necessarily spans several blocks, so the fade time is wall-clock (derived from
  the sample rate via `dt` in the EG tick), not a block count.
- **One-block idle grace.** A killed voice's block-rate EG reaches 0 one control
  block *before* the per-sample smoothing has ramped the last residual to 0.
  Retiring the voice immediately would skip that final block and leave a step.
  So `eg_tick` waits one extra tick after the EGs first all reach Idle
  (`idle_grace`) before flipping to `Idle`, giving the engine one more render
  block to settle. Negligible for slow natural releases; essential for the fast
  declick.

### 4. Poly steal: reuse in place, never retrigger hard

Poly only steals when **no spare voice exists** — there is no free slot to
crossfade into, so the crossfade does not apply. A steal is also rare and
usually lands on a *decaying* (Releasing) note. The stolen voice is therefore
reused **in place with no oscillator-phase or modulation reset**
(`retarget_pitch`, which preserves phase / LFO2 / pitch-EG / mod-env / per-lane
spread):

- stolen voice **`Held`** → legato (do not restart the amp EG);
- stolen voice **`Releasing`** (the common case) → restart the amp EG from its
  current level (`retrigger_eg`, continuing the level — no jump to 0).

`pick_slot` will not steal a `Declick` voice (it is already dying). A *free*
(idle) slot still takes a fresh `note_on` (onset from silence) as before.

**Steal ordering under a held sustain pedal.** A note released while the pedal
(CC64) is down keeps its gate high and stays `Held` (`held_by_pedal`), so by EG
stage alone it is indistinguishable from a key the player is still pressing. The
steal picker therefore prefers voices whose *physical key is already up* —
`held_by_pedal` or `Releasing` — over actively-held keys, oldest-then-lowest
within that set, falling back to the oldest key-down voice only when no key-up
voice exists. So a steal under a held pedal sheds a pedal-sustained note before
one the player is holding. Stealing clears the slot's `held_by_pedal` flag
(`claim_slot`), so a later pedal-up does not try to re-release the reused voice.

## Implementation

- `vxn2-dsp/src/stack.rs`: `VoicePhase` + `phase`/`idle_grace` fields;
  `silence`, `start_declick`, `retrigger_eg` (all ops), `eg_all_idle`;
  `is_idle()` reads `phase`; `eg_tick` self-retires with the one-block grace.
- `vxn2-dsp/src/eg.rs`: `EgState::kill_release(secs)` — fast linear release to 0.
- `vxn2-engine/src/alloc.rs`: `solo_slot` / `solo_last_pitch`; `note_on_solo`
  and `note_off_solo` rewritten (legato-Held in place, else crossfade +
  `start_declick`); `note_on_poly` steal branch (Held/Releasing reuse);
  `pick_slot` skips `Declick`; `free_slot`/`block_tick` keyed on the phase.
- `vxn2-engine/src/engine.rs`: zero `prev_eg_level` when a slot goes Idle so a
  reused slot's onset rebases from silence (not a stale level). No render
  inner-loop changes — the declick rides the existing per-sample smoothing.

## Alternatives rejected

- **In-place retrigger (any variant).** Re-cooking a sounding voice always
  discontinues decorrelated lane phase / modulation; never fully click-free
  with stacking + phase 0.5. Superseded for Solo.
- **One-control-block declick.** Tempting (it could ride the single-block level
  ramp), but 0.67 ms is too sharp; the fade must span several blocks.
- **A parallel per-voice declick gain** (a multiply in the render paths). Works
  and is click-free, but adds a second fade mechanism alongside the EG and edits
  every render path. The EG-release route reuses one mechanism (EG + 0077
  smoothing) and the lifecycle enum, at the cost of the one-block grace — judged
  the cleaner whole.

## Consequences

- Solo is no longer monophonic in *slot* terms: a note transiently overlaps its
  predecessor's ~5 ms declick (≤ 2 live stacks), trivially within the 16-voice
  pool.
- Solo retrigger no longer re-articulates the modulator structure mid-phrase
  (no per-note FM re-attack); the line re-onsets cleanly instead. Patches that
  *wanted* a hard per-note FM re-pluck would need a future opt-in.
- Poly stealing changed from a hard retrigger to in-place continuation; a stolen
  decaying voice restarts its amp envelope from the current level rather than
  snapping. Rare path, but audibly smoother.
- The 4th-difference click probe is a weak proxy here (it under-reports the
  perceived stacking click), so the authoritative check remains a manual DAW
  listen; regression tests (poly-steal lifecycle, FLUTE 2 solo 16ths at density
  1 and density 4 + phase 0.5, declick-to-idle) guard the structure.
