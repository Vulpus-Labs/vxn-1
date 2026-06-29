# ADR 0008 — Declick-headroom voice stealing

- **Status:** Accepted
- **Date:** 2026-06-26
- **Supersedes:** ADR 0006 §4 (Poly steal: reuse in place, never retrigger hard)
- **Scope:** How a Poly note is stolen when polyphony is full. Replaces the
  in-place reuse of a sounding stack with declick-the-victim + fresh-onset on a
  spare stack. Changes constants and `note_on_poly`/`pick_*` in
  `vxn2_engine::alloc`; no engine render-loop or DSP changes.

## Context

ADR 0006 §4 made a Poly steal reuse the stolen stack **in place** —
`retarget_pitch` (preserve oscillator phase / LFO2 / mod envelopes) plus either
legato continuation (`Held`) or an amp-EG restart from the current level
(`Releasing`). The reasoning: a steal only happens when no spare voice exists,
so there is nowhere to crossfade, and in-place reuse avoids a hard retrigger.

In practice that path still clicked — most audibly when the sustain pedal was
down and the voice pool was full of pedal-held notes. In-place reuse re-cooks a
*sounding* voice: the oscillator phase stays continuous but every operator's
pitch and EG re-cook, so the FM spectrum **steps** instantly. The engine
compounds it — a reused slot's allocation generation bumps, which the engine
reads as a fresh note and **resets that slot's filter/interp state mid-voice**
(ADR 0004), cutting the resonant tail. Quietest-victim selection (steal the
lowest-carrier-amplitude voice) shrank the amplitude surge but cannot fix the
timbre/filter discontinuity — in-place reuse of a full-level voice is
fundamentally a hard waveform swap.

The key observation (already true in ADR 0006 for the Solo/spare path): a fresh
voice onsetting from silence is **already click-free**, and a victim
`start_declick`ed **in its own slot** rings out cleanly because its filter state
and allocation generation are untouched. The only reason the no-spare Poly steal
could not use that proven path was the absence of a spare slot to put the new
note in.

## Decision

Provide the spare. Run **20 physical stacks** but cap **active** voices at 16:

- `N_ACTIVE = 16` — the polyphony cap (unchanged musical limit).
- `N_DECLICK = 4` — spare stacks above the cap.
- `N_STACKS = N_ACTIVE + N_DECLICK = 20` — what the engine renders and sizes its
  per-stack buffers (filters, interpolators, smoothers) by.

A voice is *active* iff its `VoicePhase` is `Held` or `Releasing`. `Declick`
tails and `Idle` stacks do **not** count, so the ≤5 ms declick fades live in the
headroom without counting against the cap.

On a Poly note-on:

1. **Victim** — a re-press of a pedal-held note targets that voice (avoids
   doubling); otherwise, only once active voices are at `N_ACTIVE`, the quietest
   active voice (lowest summed carrier-op EG level, key-up — `held_by_pedal` or
   `Releasing` — preferred, ties to oldest) is the victim.
2. **New note** — takes a spare **idle** stack (`pick_idle`, nearest-pitch for
   short glides) and onsets **fresh from silence**.
3. The victim is `start_declick`ed **in place**: it keeps its slot, filter, and
   allocation generation, so its tail rings out continuously (the ADR 0006 §3
   declick, now applied to Poly). No stack copy, no filter-state migration.

Because there are always ≥`N_DECLICK` stacks above the cap and a declick lasts
~5 ms, an idle stack is essentially always free for the new note even while a
victim fades. Idle stacks are skipped in the render loop, so the four extra
lanes cost nothing in steady state (no per-sample work until used).

**Burst fallback.** If a steal storm fills every spare with an in-flight declick
(no idle stack — only under pathological note rates, >`N_DECLICK` steals inside
one ~5 ms window), the allocator falls back to the ADR 0006 §4 in-place reuse of
the victim (`retarget_pitch` + key-up `retrigger_eg`). Graceful, bounded, rare.

## Implementation

- `vxn2-engine/src/alloc.rs`: `N_ACTIVE` / `N_DECLICK` / `N_STACKS` constants;
  `active_count`; `pick_slot` split into `pick_idle` (spare for the new note)
  and `pick_victim` (quietest active voice to shed); `note_on_poly` rewritten
  around declick-victim + fresh-onset, with the in-place reuse kept as the
  no-idle fallback. Solo paths use `pick_idle` (monophonic → a spare is always
  free). `Stack::carrier_level` (ADR-adjacent, added earlier) ranks victims.
- `vxn2-engine/src/engine.rs`: **no logic change.** Every per-stack buffer is
  `N_STACKS`-sized, so the four extra lanes are minted automatically and the
  existing fresh-note filter reset / declick handling applies unchanged.

## Consequences

- The no-spare Poly steal is now click-free by construction — it is the same
  declick-to-fresh path ADR 0006 already proved for Solo, not a distinct
  in-place mechanism.
- A stolen voice no longer continues at a new pitch; the new note is a **fresh
  attack** and the stolen note fades out. Musically this is a true note-onset
  for the new key (correct for a key-press) rather than a re-pitched
  continuation. The ADR 0006 §4 in-place continuation survives only as the
  burst fallback.
- Polyphony is held at 16 *active* voices; transient declick tails can bring the
  live stack count up to 20. CPU is unchanged when <16 voices sound (idle lanes
  are skipped); worst case renders 20 stacks for a few milliseconds during a
  steal.
- Quietest-victim selection (the prior commit's anti-"twang" work) is retained —
  it now chooses which voice to **declick** rather than which to re-attack, so
  the fade lands on the least-audible voice.
