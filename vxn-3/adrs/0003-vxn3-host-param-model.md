# ADR 0003 — VXN3 host parameter model

- **Status:** Accepted
- **Date:** 2026-06-15
- **Scope:** How VXN3 exposes parameters to the host's `clap.params` extension
  (automation, modulation, save/restore) given that each track's engine —
  and therefore its parameter set's *cardinality and semantics* — is swappable
  at runtime (ADR 0001 §4/§5). Does **not** cover the faceplate edit channel
  (custom events, shipped in 0052) or the p-lock system (ADR 0001 §3a, 0050).

## Context

A CLAP host builds automation lanes, modulation routes, and saved-project
references against a **fixed table** of parameters: each has a stable `clap_id`
(`u32`), a name, a range, and flags, declared at instantiation. Two properties
the host relies on:

- **Stable cardinality** — the parameter *set* doesn't change under the host.
  (`params-rescan` exists but see Alternatives — it's a trap.)
- **Stable semantics per id** — a `clap_id` means the same thing for the life of
  a project. Automation drawn against id 17 must still mean the same parameter
  next session, or the automation silently corrupts.

VXN3 breaks both assumptions at the engine layer. A track holds **one** active
engine from a closed roster (`Kick/Tone`, `Metal`, `Noise`, …), swapped off the
audio thread (ADR 0001 §4). Each engine has a *different* parameter set — a
different **count** and different **meaning** (Kick's "decay" is amp decay;
Metal's is a modal ring decay; Noise has a brightness control neither other
engine has). So "track 3's engine parameters" has no fixed cardinality or
semantics — exactly what the host param table cannot be.

This is why VXN3 ships (through 0052) with **no `clap.params` table at all**:
the faceplate drives the engine over a custom-event channel, and p-locks — not
host automation — are the designated automation mechanism (ADR 0001 §3a: "a lane
is just the per-param view over a track's locks"). That is fine for the MVP
groove proof but leaves nothing host-automatable. This ADR fixes the model
before a param/preset epic builds it.

## Decision

Split the parameter surface into a **fixed host layer** and a **faceplate-only
layer**, and never try to host-automate the variable per-engine set directly.

### 1. Fixed host-param table (engine-independent)

The `clap.params` table is a fixed, deterministic layout — same ids every
session, never rescanned. It contains only engine-*independent* parameters:

- **Per track** (× `N_TRACKS`): `level`, `pan`, `mute`, send amount(s)
  (`send A` [/ `B`]) — the mix layer, identical regardless of which engine is
  loaded — **plus a small fixed budget of `K` generic "macro" slots**
  (`macro 1..K`, `K ≈ 3–4`, decided when built).
- **Master/global:** master volume, limiter controls, global send-FX params
  (per ADR 0002, when those land).

Ids are assigned by a fixed positional scheme (`track t · slot s`) so they are
stable across sessions without an append-only discipline — the layout is
computed, not accreted (cf. [[vxn1-id-stability-dropped]] applies to the
*internal* table, not this host-facing one).

### 2. Macro slots: stable id, engine-reinterpreted value

A macro slot is the bridge across the variability gap:

- The slot's **id and name are fixed and generic** (`T3 · M1`). The host
  automates a stable id; nothing is rescanned on engine swap.
- The active engine **reinterprets the normalized slot value** onto its own
  patch. Each engine declares a mapping `macro[0..K] → patch params` (the
  generalisation of today's `TrackEngine::set_knob`). An engine maps the slots
  to its *most important* `K` controls; if it has fewer, a slot maps to a
  sensible secondary; if more, the surplus live only in the faceplate layer (§3).
- The slot's **displayed value is engine-aware** via CLAP `params.value_to_text`
  — "Decay 0.42 s" under Kick, "Ring 1.8 s" under Metal — so a generic *name*
  doesn't mean an opaque *readout*. (CLAP allows dynamic value text under a fixed
  name.)

The accepted cost: an automation lane labelled "T3 · M1" controls a
*different parameter* if the user swaps that track's engine. This is the
standard macro/groovebox trade (Elektron machine pages, drum-rack macros) and is
acceptable because the deep, engine-specific automation lives in p-locks (§3),
not host lanes.

### 3. Faceplate-only layer (the variable depth)

The full, properly-labelled, per-engine control set — every patch param with its
real name/range — stays on the **custom-event channel** (0052) and is **not** in
the host table. It is automatable *internally* via p-locks (0050): a p-lock can
target any continuous engine param, so per-hit timbre evolution is fully
expressible without host lanes. p-locks may target the macro slots and mix
params too, so the internal and host layers compose.

### 4. No rescan-on-swap

Engine swaps **never** call `params-rescan` to relabel ids. Names/info are fixed;
only macro *values* are reinterpreted and *value-text* is dynamic. (See
Alternatives.)

## Consequences

- The host sees a stable, fully-automatable **mix + macro** surface; cardinality
  and ids never change, so saved automation never corrupts.
- Semantic opacity of generic macro names is bounded by engine-aware value-text.
- Deep, variable, per-engine control is reachable from the faceplate and
  automatable via p-locks — not the host — which matches ADR 0001 §3a's stance
  that p-locks *are* the automation mechanism.
- The preset system (future epic) stores, per track: engine kind + patch + the
  macro→patch mapping is engine-defined (lives in code), so presets only persist
  engine + patch + macro slot values.
- `TrackEngine` grows a declared macro mapping (`macro_count`, `set_macro`,
  `macro_display`) — a clean superset of the current `set_knob`.
- Until this is built, VXN3 has no host params (MVP state) — acceptable; the
  groove proof needs none.

## Alternatives considered

- **Union/superset table** — declare every engine's params per track (~13 × 8 ≈
  100+), inactive ones inert for the current engine. True fixed semantics, but a
  bloated automation list dominated by parameters that do nothing for the loaded
  engine. Rejected: poor UX, large RT param-flush surface.
- **`params-rescan` on engine swap** — re-declare names/info when an engine
  changes. CLAP permits it, but (a) host support is uneven and disruptive, and
  (b) silently repurposing a stable id breaks the meaning of automation already
  drawn against it. Rejected as fragile.
- **No host params at all (current MVP)** — everything via custom events +
  p-locks. Correct for the groove proof, but gives the host no automation/
  modulation hook whatsoever. Kept as the MVP state, superseded by this ADR for
  the param epic.
- **Per-track *fixed* engine (no swap)** — would make a flat table trivial, but
  throws away the engine-defined-voicing thesis (ADR 0001 §5). Rejected.
