# ADR 0001 — VXN3 overall design

- **Status:** Accepted
- **Date:** 2026-06-15
- **Scope:** Architecture of VXN3 — a sample-free synthesis drum machine,
  rhythm-pattern-first, capable of pitched hits, shipped as a Rust CLAP plugin
  in the Vulpus Labs line.

## Context

VXN1 is a subtractive polysynth (Jupiter-8 idiom); VXN2 is a 6-operator FM
synth with first-class voice stacking. VXN3 is a *separate* instrument
targeting a different job: rhythm. It is a drum machine, but an unusual one —
no sample playback, all sound synthesised, and built so that **interesting
rhythmic patterns are accessible** rather than buried in menus. It plays
pitched material as readily as percussion, because percussion vs note is
largely envelope + pitch-tracking over a shared engine roster, not a separate
path.

Constraints carried over from VXN1/VXN2:

- Real-time process callback: allocation-free, predictable, no panics across
  the FFI boundary.
- Permissive licensing only (MIT / Apache-2.0). CLAP via `clack`.
- macOS / Apple Silicon first; Windows/Linux not structurally precluded.
- Hardware-style HTML faceplate UI, not generic host knobs.
- Shared infrastructure with the rest of the line (CLAP shell, preset system,
  faceplate idiom, build/release pipeline via `vxn-core-*` + xtask).

New for VXN3:

- The differentiator is the **pattern engine**, not the timbre engine. A plain
  step grid is table stakes; the value is making generative rhythm accessible.
- The voice side must host a *range* of synthesis techniques (808-style
  kernels, FM, modal/metallic, noise) heterogeneously, one technique per track,
  without sacrificing RT safety or SIMD.

**Genre target for v1: psychedelic / minimal techno.** Repetition + slow
evolution + space + dub throws. This narrows and disciplines every decision
below.

## Decision

### 1. Interaction model: step sequencer + generative sprinkles ("model A")

Familiar per-track step grid as the spine, with high-leverage generative levers
surfaced as few knobs (not a node editor, not a pure-generative black box).
Generative-first and performance-morph models are explicitly *not* v1.

### 2. Pattern engine = N independent track lanes (polymeter)

A pattern is a set of independent tracks, **not one shared grid**. Each track
has its own length and clock divisor, so tracks phase against each other
(track 1 len 16, track 2 len 12, track 3 len 7 → hypnotic drift "for free").
This is the core data-model decision and the largest payoff-per-effort lever
for the target genre.

Per-track / per-trig levers:

- **Retrig n-over-m** (patches tracker model): a trig owns a sub-window of `m`
  steps and fires `n` times within it. Per-trig params: count `n`, span `m`,
  timing curve (even / accel / decel), velocity ramp. A per-trig *macro*.
- **Probabilistic + conditional trigs:** per-trig probability and condition
  groups (1:2, 3:4, fill, prev/neighbour). Cheap, large "alive" payoff.
- **Per-step p-locks:** pin a param value (or a ramp toward one) to a step, with
  hold/latch/ramp behaviour — full semantics in §3a. This *is* the automation
  mechanism: a "lane" is just the per-param view over a track's locks, not a
  separate data model. Slow per-bar evolution comes from sparse ramp/latch locks
  over the (long, polymetric) loop — minimal techno lives on change over many
  bars, so evolution is designed in, not bolted on.
- **Groove + lane shift:** push hits early/late off the grid and shape feel.
  Timing feel is *not* a per-trig attribute — it comes from a **groove**, a
  reusable per-grid-position timing+velocity template assigned per track, edited
  in its own surface (swing is a parametric groove). Per-lane shift is a
  constant per-track offset that phases the whole lane against the others. Full
  semantics in [ADR 0006](0006-vxn3-groove.md), which supersedes
  [ADR 0004](0004-vxn3-micro-timing.md).

### 3. Dub sends as part of the instrument

FX is instrument, not master polish. The headline is the **dub throw**: a
track's send amount is **p-lockable** (§3a), so locking send high on one step
smears that hit into a delay/reverb tail, and rhythmically locking it gates the
lane into the loop (dub *gating*). The full FX system — module roster, the
lane-insert / send-bus / master scopes, internal-vs-external loops, and slot
budgets — is its own design: see [ADR 0002](0002-vxn3-fx-architecture.md).

**External send/return over CLAP audio ports.** FX buses can be routed
externally so the host's outboard / DAW FX sit in the send path. The plugin
exposes two extra stereo **send** output ports + two matching stereo **return**
input ports (`send A/B`, `return A/B`) via the CLAP `audio-ports` extension
alongside the main stereo out; a bus configured external sends to its pair and
folds the return back into the mix, sequenced and p-locked exactly like an
internal bus. Port layout is fixed at activation, no allocation in `process`;
hosts that don't wire the returns see silence on those inputs and the internal
buses remain the default. (Bus count and how many may be external: ADR 0002 §5.)

### 3a. Parameter locks — semantics

A p-lock is a per-step override of a *continuous* track param (engine param,
send amount, pan, level…). It is distinct from **trig attributes** (retrig
n/m, probability, condition, velocity/accent), which live *on the trig*
and have no base to revert to. (Micro-timing was formerly a trig attribute;
[ADR 0006](0006-vxn3-groove.md) moves timing feel out to the groove template.
Per-step **velocity/accent** stays on the trig — compositional accent, distinct
from the groove's feel-based velocity contour.) p-locks subsume what would otherwise be a
separate automation lane: a "lane" is the per-param view over a track's locks.

A lock record is:

```text
{ param, value, shape: step | ramp, curve, N, termination: revert | latch }
```

Two orthogonal axes give the four behaviours:

- **shape** — `step` (jump to value) or `ramp(curve, N)` (interpolate to value
  over `N` ticks).
- **termination** — `revert` (release the override after) or `latch` (hold the
  final value until the next lock on this param).

|            | revert                       | latch                        |
| ---------- | ---------------------------- | ---------------------------- |
| **step**   | hold `N` ticks, then release | latch                        |
| **ramp**   | ramp over `N`, then release  | ramp over `N`, then latch    |

Resolution per tick is layered: `effective = base`, then the active p-lock
override on top. *Revert* stops contributing → fall back to base. *Latch* keeps
contributing the held value until a later lock supersedes it. (A pure momentary
spike is `step` + `revert` with `N = 1`.)

Resolved edge cases:

- **Ramp start = live.** A ramp interpolates from the current effective value at
  the moment it fires, not a stored start — glide from wherever we are.
- **Preemption.** A new lock on a param immediately supersedes any in-flight
  ramp/hold from a prior lock on that param; an aborted ramp's live value
  becomes the new ramp's start. No queue.
- **Loop wrap.** A latched value persists across the pattern loop boundary until
  a lock changes it. So loop 1 (cold — base until the first lock) differs from
  steady-state loops; that is intended.
- **`N` is in the lane's own ticks.** Time base is per-track/per-lane
  (polymeter, §2): lane A in triplets, lane B in semiquavers → a ramp's length
  tracks that lane's grid, not a global clock. `N` may exceed step spacing (a
  ramp spanning several steps).
- **Curve is a named type**, not a continuous knob: `linear`, `fast-start /
  slow-finish`, `slow-start / fast-finish`, `S-shaped`.

RT/storage: per track, a sparse `(param, step) → lock` table; the resolver keeps
a small per-locked-param state struct (current value, ramp progress, ticks-left,
termination). Bounded by engine param count → preallocated, alloc-free in
`process`.

### 4. Track = one active engine + patch; heterogeneous across tracks

Each track is a polymorphic slot holding **exactly one** active engine instance
plus its patch/param state. Engines are **not** all instantiated per track
(that wastes RAM and burns dead CPU). RT discipline:

- **Engine swap happens off the audio thread.** Build/init on the main thread,
  pre-allocate storage, hand the pointer to the audio thread over the existing
  lock-free channel. The audio thread never allocates or blocks; swaps must not
  click.
- **Dispatch per-block, not per-sample.** Match the engine type once per track
  per block, then run its kernel. (Per-sample dispatch would defeat SIMD — see
  the VXN1/VXN2 "runtime match inside the lane loop drops NEON to scalar"
  lesson.)
- **Only the active engine ticks.**
- Cross-track SIMD is abandoned by design: tracks are heterogeneous and mono-ish
  (~8–16 of them, not 16×8 poly). Scalar per-track dispatch is the right
  altitude; vectorisation happens *inside* an engine.

### 5. Per-track 4-wide SoA state block; lane semantics are engine-defined

A track may own an SoA state block whose width is **the engine's choice** —
this restores SIMD *within* a track even though tracks are heterogeneous. The
common case is width 4 (one NEON `f32x4`), but the engine decides: some engines
need no such block at all (a single scalar voice, a trivial noise source);
others are wider (modal/metallic). And the key abstraction on top of width:
**the engine decides what a lane means.**

- **Poly engine** (kick, tom, clap, tonal stab): lanes = up to 4 voices. Trig =
  voice allocation (round-robin / oldest-steal). Independent overlapping tails.
- **Resonator / modal engine** (hat, cymbal, ride, metallic / noise bed): lanes
  = modes/partials of *one* physical body. Trig = inject excitation into the
  persistent state; a re-hit rides on the decaying state rather than spawning a
  parallel copy (the real 909 open→closed-hat behaviour).

Same `f32x4` storage and SoA codegen in both cases; only the lane *axis*
differs. Consequences:

- **Voicing model is an engine property**, exposed via the engine trait
  (`on_trig` = allocate-voice vs inject-excitation).
- **Choke is engine-defined:** poly → voice kill/steal; resonator → damping
  (raise loss / collapse decay), e.g. closed hat chokes open hat = same body,
  more damping. No single global choke rule.
- **Lane budget is engine-declared**, from zero upward. A poly engine caps its
  *voice* count at 4 (the agreed poly ceiling) → width 4. A simple engine may
  need no block (scalar / width 1). Metallic / modal engines may want 8–16
  partials for a convincing cymbal. The storage is a max-sized / const-generic
  block and the engine declares how many lanes it actually drives; the host
  loop must not assume a uniform width across tracks.
- **No enum match inside the lane loop** — engine type is fixed per track per
  block; dispatch via marker/macro outside the loop (VXN2 `WaveKind` hoist
  pattern).

### 6. Engine roster: closed, hand-written SoA kernels

Engines are a curated, hand-written closed set of lane-parallel (SoA) kernels.
This is what makes the SIMD and the "accessible = curated knobs" goals
achievable.

- Port the `patches-drums` 808-style kernels (Kick, Snare, Hat, …) *in* (taking
  a `patches` runtime dependency is rejected for v1). Whether a kernel is
  rewritten 4-wide SoA or stays scalar is **per the engine's voicing model**
  (§5): a kernel that wants independent overlapping tails — or later trigs that
  choke but don't terminate earlier ones — needs the parallel-voice SoA rewrite;
  a kernel where retrig simply restarts the envelope stays scalar (width 1) and
  the port is close to a straight copy.
- Reuse the VXN2 6-op FM core as an FM body / tonal engine where it earns its
  place.
- Add modal/metallic and noise engines for the resonator class.
- **`patches`-graph-as-engine is reserved as a future scalar escape hatch** (a
  single engine variant), not part of v1 — it runs per-voice through the graph
  scheduler and will not fold into one `f32x4`.

## Consequences

- The load-bearing novelty is the engine trait with engine-defined voicing,
  trigger, and choke semantics over a uniform SoA block. Get this right before
  writing kernels.
- Drum kernels must be authored 4-wide from the start; porting `patches-drums`
  is a rewrite, not a copy.
- Polymeter + p-locks + slow automation are required for the genre even though
  the model is "just" a step sequencer.
- Crate layout, parameter model, and patch format are TBD (follow-up ADRs),
  but reuse the `vxn-core-*` shell/preset/build infrastructure.

## Alternatives considered

- **Generative-first or performance-morph interaction (models B/C):** richer
  but less accessible and harder to scope; deferred.
- **Every engine instantiated per track:** trivial to switch but wastes RAM and
  CPU; rejected.
- **One global voicing/choke model:** cannot represent both struck-independent
  and single-evolving-body percussion; rejected in favour of engine-defined
  semantics.
- **`patches`-graph as the primary engine substrate:** maximum flexibility (we
  own `patches`) but couples VXN3 to the graph runtime, is heavier per track,
  won't vectorise the lane pool, and is harder to make "accessible"; kept only
  as a future escape hatch.
