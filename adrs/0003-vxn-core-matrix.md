# ADR 0003 — vxn-core-matrix: one modulation routing engine, two rosters

- **Status:** Proposed
- **Date:** 2026-08-30
- **Scope:** Where mod-matrix routing code lives. Companion to epic
  [E049](../epics/open/E049-shared-matrix-routing.md). Follows the precedent set
  by [ADR 0002](0002-vxn-core-dsp.md) for the DSP component layer, and applies
  its test — *"is this signal-model-specific, or did it fork by copy-paste?"* —
  to the routing layer.

## Context

vxn-1b and vxn-2 each have a 16-slot mod matrix. vxn-1b's was written as a port
of vxn-2's and says so:
[matrix.rs](../vxn-1b/crates/vxn1b-engine/src/matrix.rs) opens with *"The routing
model for VXN1b, adapted from VXN2's matrix … with VXN1's source/destination
sets"*, and
[mod_smoothing.rs](../vxn-1b/crates/vxn1b-engine/src/mod_smoothing.rs) with
*"the discontinuity guards VXN2 has and VXN1b's raw per-control-block matrix
apply lacks … ported and trimmed from VXN2's 16 stack-lanes"*.

The port was the right call at the time. What it produced is two hand-maintained
copies of one design, which have since drifted in both directions — each has
picked up improvements the other lacks.

The trigger for writing this down: adding the `abs` polarity, the polarity/shape
split and the scale-VCA bend meant writing the **same ~200 lines twice, by
hand**, once into each synth, in one sitting. Nothing about those lines is
specific to subtractive or FM synthesis.

### What is actually duplicated

Verbatim-or-near, no design content:

| Thing | vxn-1b | vxn-2 |
|---|---|---|
| `Polarity` × `Shape` axes, nine dispatch arms | `matrix.rs`, `eval.rs` | `matrix.rs` |
| `curve_code` / `curve_split` / `CURVE_NAMES` legacy table | same | same |
| `scale_norm` fold-then-bend + `is_bipolar` (ADR 0009) | same | same |
| `cook_depth` cubic taper on semitone dests | `Pitch` | 13 pitch-class dests (7 of the 8 smoothed ones — `Lfo2Phase` passes through — plus the 6 stack-pitch dests) |
| `DEST_GAIN` native-unit scaling | same idea | same idea |
| Slot / table shape, `enabled` + `is_wired` semantics | same | same |
| Sub-block smoothing tiers + two-pole cascade rationale | explicit port | original |
| Name/label/`from_u8` table discipline | `matrix_enum!` | hand-written |

The last row is the clearest signal that this is drift rather than design:
**the same requirement, solved twice, unequally.** vxn-1b's `matrix_enum!`
generates the enum, both string tables, the decoder and `ALL` from one row list,
making a transposed name/label pair *unrepresentable*. vxn-2 maintains five
parallel lists by hand and tests only their lengths.

### What is genuinely different

- **Granularity tiers and coherence.** vxn-2 routes to patch-global FX
  (`delay-mix`, `reverb-mix`) and per-stack dests, so a per-lane source into a
  coarser dest is a lossy collapse worth flagging in the UI. Every vxn-1b
  destination is per-voice. This is a real difference, but it is the
  **degenerate case** of vxn-2's model, not a rival model: all dests `PerLane`
  ⇒ `coherence` is constant `Ok`.
- **Memory layout.** vxn-2's accumulators are lane-major, so its route
  accumulate is a gather/scatter and stays scalar — only the scale-VCA loop,
  which walks a contiguous local, vectorises (2-wide). vxn-1b's
  `eval_dests_bank` is dest-major SoA, const-generic over lane count. Genuinely
  different — and vxn-1b is simply ahead. Converging is ticket
  [0328](../tickets/open/0328-matrix-dest-major-lane-accumulators.md).
- **Route precompilation.** vxn-1b compiles a `RouteList` once per block,
  hoisting the sentinel checks, the zero-depth skip, `cook_depth` and the
  `DEST_GAIN` lookup out of the per-voice loop. vxn-2 hoists only `cook_depth`
  (applied once at table-rebuild time, engine.rs:857) and redoes the rest on
  every eval. Again a gap, not a disagreement — narrower than first written.
- **State encoding policy.** vxn-2 nibble-packs into spare bits because its
  blobs must stay readable; vxn-1b widens its record and bumps the version
  because it rejects older blobs outright. **Correctly divergent** — each
  follows its own format's stated contract, and neither should adopt the
  other's.
- **Rosters, layer count, automatable-depth count.** Per-synth by definition.

## Decision

### 1. Extract `vxn-core-matrix`: mechanism shared, roster per-synth

The seam is between the **roster** (what can be routed) and the **mechanism**
(how a routing is evaluated).

Shared: slot and table types, the polarity/shape axes and their dispatch, the
scale VCA, route compilation, the evaluator, the smoother bank, the coherence
predicate, the preset-name codec.

Per-synth: the source and destination enums and their tables, each dest's native
gain, depth taper, tier and smoothing class — and everything above the engine
(layer ownership, CLAP surface, wire and state encodings).

A synth supplies its roster through one macro-generated declaration; the engine
is generic over it.

### 2. The roster row declares everything keyed on a destination

vxn-1b's `matrix_enum!` moves into the shared crate and gains columns. A
destination row declares its wire name, label, native gain, depth taper, tier
and smoothing class **in one place**. Today those live in four separate
structures that are kept in step by hand — `DEST_GAIN`, the `cook_depth` match,
`PITCH_DESTS` / the smoothing tiers, and (vxn-2) the tier function.

The property this buys is the same one `matrix_enum!` already buys for names:
**you cannot add a destination without deciding**, because the row will not
compile until every column is filled.

#### The `u8` crossing the seam is the storage index, not the wire discriminant

Settled while building the skeleton (0329), recorded here because it decides
what `matrix_enum!` emits and so has to be known before 0332 writes it.

A roster's `u8` is a **storage index**, `0..N_DESTS`, with the `None` sentinel
excluded — so `dest_names().len() == N_DESTS`, one shorter than the
`[&str; N + 1]` wire tables both synths keep for decoding. The alternative, the
wire discriminant with the sentinel at 0, would let the existing tables be
transcribed unchanged, but it leaves a dead row at index 0 in every
accumulator-shaped array — `out[di]`, `DEST_GAIN[di]`, the smoother's state
rows — and puts an off-by-one at every one of those uses. Both synths already
carry an `idx()` that drops the sentinel, so the conversion is not new work;
it is where the conversion already happens.

Two consequences worth stating:

- **0332 generates two tables from one row list**, not one. The synth keeps its
  `[&str; N + 1]` wire table for decode; the roster additionally gets the
  sentinel-free `N`-long one. That is a property of the generator, not a second
  hand-maintained list.
- **It is what makes §3's contiguity affordable.** The smoothing bank wants each
  class to occupy an unbroken run of rows, which collides with vxn-2's frozen
  dest discriminants — but only if wire id and storage row are the same number.
  Holding them apart from the start means 0335 can order storage rows freely
  without touching a wire encoding, and the decoupling costs one compile-time
  lookup per route in `RouteList::compile`, never anything in a lane loop.

### 3. Smoothing is post-sum, per-destination, and declared

*(The question this ADR was asked to settle.)*

**Post-matrix-sum, not per-route.** The smoothers are linear, so filtering each
route and then summing is mathematically identical to summing and then
filtering — at N× the cost and N× the state, for N slots sharing a dest. There
is no case for per-route smoothing.

**Per-destination, not uniform.** The correct time constant is a property of how
click-prone the destination is, not of the source driving it. `delay-mix` never
clicks; pitch stairsteps audibly at every control-block edge (~1.5 kHz at
48 kHz). A uniform per-sample policy across vxn-2's 51 destinations would buy
nothing for most of them and cost real cycles per lane.

**Declared in the roster, not listed elsewhere.** The classes — which cover
every smoother both synths run today, though not every *motion*; see the
exceptions below the Amp paragraph:

| Class | Filter | Ticked | Used today for |
|---|---|---|---|
| `Block` | none — held for the control block | — | the default; most dests |
| `Quantum` | one-pole | every render quantum | vxn-1b PWM, cross-mod amount, Pan |
| `QuantumCascade` | two cascaded one-poles | every render quantum | pitch (both synths), vxn-1b `XModSweep` |
| `PerSample` | one-pole | every frame | vxn-1b non-envelope Amp |

The cascade in `QuantumCascade` is load-bearing and must not be "simplified" to
one pole: a single pole is C0 but C1-broken — at a saw or pulse LFO step the
output *value* is continuous while its *velocity* jumps 0→max, and that velocity
step is the click. Both synths independently arrived at two poles.

**Smoother state is per-lane and resets on voice start.** A stolen or restarted
voice must not glide from the previous note's modulation; the engine exposes
`snap_to` (vxn-2 already has exactly this) and the synth calls it on note-on.

**Layout, and what vectorises.** The smoothers are the one part of the pipeline
that is *already* SIMD: `PitchSmoother::tick` compiles 4-wide post-LTO
(`dup.4s` to broadcast the coefficient, then `fsub.4s` chains). This is worth
stating because it was measured wrong twice — see the note on method below.

The bank form is nonetheless a win, for a reason that has nothing to do with
adding SIMD. The current tick fuses both cascade stages in one loop body, so
stage 2 reads the stage-1 value just written and the vectoriser must interleave
with `zip2`/`uzp2` shuffles. Splitting into two flat passes — stage 1 across the
whole span, then stage 2 across it — removes the shuffles: **16.9 ns → 9.1 ns,
−46%**, measured on the linked binary.

That makes the layout requirement concrete. A bank is a flat contiguous span of
`[member][lane]` floats sharing one coefficient, and both synths already satisfy
the coefficient half — vxn-1b's smoother says so outright ("the coefficient is
*not* a field: it belongs to the tier ... rather than to the quantity"). The
span half needs the roster to order destinations so that each smoothing class
occupies a contiguous run of rows, which collides with vxn-2's frozen dest
discriminants — but only if wire id and storage row are the same number.
Decoupling them costs one compile-time lookup applied once per route at
`RouteList::compile`, never in a lane loop.

**On measuring any of this.** `cargo rustc --emit asm` on a library crate in this
workspace runs **no loop vectoriser at all**: with `lto` set, cargo passes
`-C linker-plugin-lto` and the pipeline defers to link time. Every vectorisation
claim in this ADR was re-derived with `llvm-objdump` on a linked bench binary
after the per-crate method was found to report scalar code for a canary loop that
plainly vectorises. Tickets under [E049](../epics/open/E049-shared-matrix-routing.md)
that assert anything about SIMD must measure the same way.

**One acknowledged exception, which stays synth-side.** vxn-1b smooths only the
*non-envelope* part of the Amp coefficient — the envelope part is per-frame
exact, and smoothing it would smear the attack. That factoring is a property of
vxn-1b's VCA, not of routing. The shared engine owns the smoother; the synth
decides what to feed it. `Amp` is therefore declared `Block` in vxn-1b's roster
and its bank applies `PerSample` smoothing to the part it chooses. If vxn-2 ever
needs the same split, it gets the same escape hatch — this is a deliberate limit
on the abstraction, not an oversight.

vxn-2 in fact already exercises that escape hatch in three more places, all
engine-side motion applied *after* the matrix and none of it a smoother in the
bank's sense: the op level/pan/phase dests **ramp per-sample linearly** to each
block's target; `StackDetune`/`StackSpread` take a **block-rate one-pole**
(`STACK_MACRO_SMOOTH`, snap-on-fresh); and the nine EG-rate dests are consumed
**once, at note-on** — consumption-time semantics, not smoothing at all. All of
these declare `Block` in the roster and keep their motion where it lives today,
in the synth's target application. Migrating any of them into the bank would be
a behaviour change and is out of E049's scope.

### 4. Two transports, split by what changes — not one

The UI reaches the audio thread three different ways today: vxn-2 writes per-slot
atomics that the engine re-reads every block; the web path posts events into an
SPSC ring; vxn-1b takes a `Mutex` and raises a reload flag. The first two are
each internally coherent. The third is not — a mutex on the audio thread is a
priority-inversion risk, and it is the only place in either synth where the
render can block on another thread (ticket
[0338](../tickets/open/0338-vxn1b-topology-ring-delete-the-mutex.md)).

The fix is **not** "SPSC everywhere". The right axis is what kind of change is
being communicated:

| Channel | Carries | Why |
|---|---|---|
| **Value store** — atomics, latest-wins | depths and every CLAP param | Idempotent, so a knob drag coalesces for free with no backpressure. The store must exist anyway: the host reads `get_value` off the main thread. Queueing these means draining values already superseded. |
| **Topology ring** — SPSC, ordered | source, dest, polarity, shape, scale source, scale bend, enabled | Discrete, multi-field, human-rate. Ordering carries meaning; atomicity across fields does not come free from per-field atomics. |
| **Epoch + snapshot** — underneath both | preset load, state restore | A bulk update of ~500 params and 32 slots is the case a snapshot handles better than a burst of events. Doubles as the ring's overflow backstop. |

Standardising on one mechanism would make continuous parameters worse to buy
uniformity. Standardising on *two, chosen by semantics*, removes the unsound
mechanism and still collapses the duplication that matters — today the same
logical edit has two encodings, a native JSON op writing atomics and a 16-byte
codec event on web.

There is also a forcing function on vxn-2's side, recorded here so it is not
rediscovered under pressure: its packed topology word is **exactly full** —
`source 8 | dest 8 | scale_shape 4 | curve 4 | scale_src 7 | active 1 = 32`. The
scale bend consumed the last nibble. One more per-slot field and "one atomic word
per row" stops working, at which point the choice is a seqlock or the topology
ring above.

### 5. The test surface splits in two

Today's tests conflate mechanism with roster. vxn-1b asserts
`out[Cutoff] == 24.0`, which is really three claims at once: that the evaluator
multiplies correctly, that `DEST_GAIN[Cutoff]` is 48, and that `Cutoff` takes no
depth taper. When one changes, an unrelated-looking test fails.

**Mechanism tests live in the shared crate, against a synthetic roster** — a
small fixed set of sources and dests with all gains 1.0 and no taper, so a
number in an assertion is the evaluator's arithmetic and nothing else.

The form is the one this ADR was asked for: *these routes at these depths, these
source values, these modulation amounts.* A case is a declarative record —

```text
routes:  [(source, dest, depth, polarity, shape, scale_src, scale_shape, enabled)]
sources: {name: value}
expect:  {dest: value}
```

— run through **every** evaluator path (scalar and banked) with the results
required to agree bit-exactly, since float addition is not associative and
"same routes in the same order" is already vxn-1b's stated contract between its
two paths. The cases are data, so covering all nine polarity×shape pairs, both
scale polarities, all three scale bends, the on/off switch, slots sharing a dest
and inert-slot compaction is a table rather than a test function each.

**Roster tests stay per-synth** and assert only roster facts: this dest's gain
is 48, these dests take the cubic taper, this dest is per-stack, this dest
smooths as `QuantumCascade`. Plus the property tests that already exist —
variant order matches the tables, every dest is reachable, the factory patch
drives the amp.

**vxn-2 keeps its render-hash baseline, and vxn-1b gains one in 0329** — it has
none today. The pair then pins "nothing changed" through the extraction, as a
tripwire under the null-test bar.

## Consequences

- Adding a routing feature is one implementation and two roster rows, not two
  implementations. The work that prompted this ADR would have been ~200 lines
  once instead of twice.
- vxn-2 gains vxn-1b's route precompilation, its generated roster tables, and
  (via 0328) its vectorised layout. vxn-1b gains vxn-2's coherence surface,
  which is free for it — all-`PerLane` makes every verdict `Ok`, but the
  machinery is there the moment vxn-1b adds a global destination.
- **The evaluator cannot be shared before the layouts converge.** 0328 is a
  prerequisite for the evaluator ticket, not an optional cleanup. The curve
  vocabulary and the roster declaration have no such dependency and can land
  first.
- Every step must prove it changed nothing audible on either synth — which is
  **not** the same as bit-identical output. Several steps legitimately reorder
  float operations, and float addition is not associative, so the render hash
  will move without anything changing that a listener could detect. The bar is a
  null test at −100 dBFS against the pre-step render, with the hash kept as a
  free tripwire; two pure-movement steps stay strictly bit-exact because there a
  moved bit means a mistranscription. E049 §"The bar" has the detail. Unlike
  [E041](../epics/open/E041-shared-fx-unification.md), which unifies genuinely
  different declick idioms and accepts flagged re-baselines, this extraction has
  **no** intended behaviour change — but "no intended change" is enforced by
  measuring the difference, not by freezing the bits.
- A third synth wanting modulation routing (vxn-3 has none today) inherits the
  engine by declaring a roster.
- The abstraction is deliberately bounded: it owns routing and the smoothers,
  not what a destination *means*. Applying a dest total to a filter coefficient,
  a phase increment or a VCA stays in the synth.

## Alternatives considered

**Leave them forked.** Cheapest today, and defensible while the copies were
young. Rejected because the copies are no longer young and the drift is now
bidirectional — each synth has improvements the other lacks, so every future
feature costs two implementations *and* a decision about which synth's version
to copy from.

**Share only the curve vocabulary.** The lowest-risk slice, and it is the first
ticket either way. Rejected as the *whole* answer because it leaves the
evaluator, route compilation and smoothing duplicated — which is where the
subtle bugs live. Kept as the first step, not the last.

**Unify the wire and state encodings too.** Rejected. The two formats have
different, explicitly stated compatibility contracts; forcing one on the other
would either break vxn-2's saved patches or saddle vxn-1b with a legacy encoding
it has no reason to carry.

**One roster with a superset of sources and dests, feature-gated.** Rejected:
the rosters are the genuinely synth-specific part, and a union of an FM operator
matrix and a subtractive voice matrix is a table where most cells are invalid
for any given synth — exactly the thing the tier/coherence machinery exists to
avoid.
