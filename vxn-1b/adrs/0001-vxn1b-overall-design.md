# ADR 0001 — VXN1b overall design (matrix-modulation variant of VXN1)

- **Status:** Proposed
- **Date:** 2026-07-25
- **Scope:** Architecture of **VXN1b** — a variant of VXN1 (the Jupiter-8-style
  subtractive polysynth) whose *sound engine is unchanged* but whose entire
  modulation surface is replaced by a **generic mod matrix** in the VXN2 idiom.
  Outcomes sought: a simpler, more compact faceplate; much more flexible
  routing; at the cost of more opaque patch programming.

## Context

VXN1 ships a subtractive polysynth with a deliberately **idiomatic, fixed**
modulation surface. [ADR 0004](../../vxn-1/adrs/0004-vxn1-osc-interaction-and-fixed-panels.md)
*ripped out* VXN1's original generic 6×4 matrix and replaced it with fixed,
labelled JP-8/Juno panels (Pitch Mod, PWM Mod, Filter Mod, Mod Wheel, Pitch
Wheel). That decision optimised for "reads like the hardware panel" and
accepted the loss of arbitrary source→dest routings as low-value *for that
instrument*.

VXN2 took the opposite stance: [ADR 0001 §6](../../vxn-2/adrs/0001-vxn2-overall-design.md)
makes a fixed-slot mod matrix **the only** routing mechanism, with dedicated
knobs reduced to macros that write into slots.
[ADR 0009](../../vxn-2/adrs/0009-matrix-scale-source.md) later gave each slot a
secondary scale source (a per-route VCA) so performance controls (mod wheel,
aftertouch) can gate a route's depth — the expressive move the additive-only
matrix could not express.

VXN1b explicitly **reverses VXN1's ADR 0004 decision** for a *separate* product.
The bet: the same DSP that sounds like a Jupiter-8 becomes a substantially
different, more programmable instrument when its routing is a matrix rather than
a panel of fixed faders — and the freed faceplate real estate makes the front
panel simpler and more compact. This is not a change to VXN1; VXN1 continues to
ship its fixed-panel surface. VXN1b is a sibling that reuses VXN1's kernels.

Constraints carried over unchanged from VXN1:

- Real-time process callback: allocation-free, predictable, no panics across FFI.
- Permissive licensing only (MIT / Apache-2.0); CLAP via `clack`.
- macOS / Apple Silicon first, Windows/Linux not structurally precluded.
- Hardware-style HTML faceplate in a wry WebView, not generic host knobs.

## Decision

### 1. Sound engine reused verbatim — only routing diverges

VXN1b's DSP is **bit-identical to VXN1**: 16-voice poly, 2 osc + noise → mixer
→ 4-pole ZDF ladder + HPF → VCA, hard-sync / PM / ring cross-mod, two ADSR
envelopes, two LFOs, oversampling, the whole per-sample kernel set. VXN1b takes
a **direct crate dependency on VXN1's `vxn-1/crates/vxn-dsp`** rather than
copy-adapting it (the DSP reuse policy of VXN1 ADR 0001 §9 exists to allow
*divergence*; here we want *zero* divergence, so sharing the crate is correct
and prevents drift).

What diverges from VXN1 lives in three forked crates:

- **`vxn1b-engine`** — the parameter table and the block-render *routing* loop.
  The fixed per-channel resolution of VXN1 ADR 0004 §4 (`cutoff_mod =
  lfo1·d_lfo1 + lfo2·d_lfo2 + …`) is replaced by a generic matrix evaluator.
- **`vxn1b-clap`** — CLAP shell with VXN1b's own stable plugin id and param set.
- **`vxn1b-ui-web`** — the compact faceplate + a mod-matrix overlay.

Shared root crates (`vxn-core-clap`, `vxn-preset`, `vxn-core-ui-web`,
`vxn-core-app`) are reused as-is.

> **Open decision (see Alternatives):** this ADR recommends VXN1b as a **new
> sibling product** (`vxn-1b/crates/*`) reusing `vxn-dsp`. The alternative — a
> cargo *feature flag* inside VXN1 that swaps the routing layer and faceplate —
> is rejected below.

### 2. Modulation is a generic fixed-slot matrix (the only routing mechanism)

Adopt VXN2's matrix model (ADR 0001 §6 + ADR 0009) essentially wholesale, with
VXN1's source/destination sets. **16 slots** in v1 (matches VXN2 code and is
ample for a two-osc subtractive voice; most patches use far fewer). Each slot:

```text
(source, destination, depth, curve, scale_src)
out[dest] += source · curve(depth) · scale_norm(scale_src)
```

Slots to the same destination **sum** (additive). `scale_src` is the per-route
VCA of VXN2 ADR 0009 (leaf-value, no cycles, `[0,1]` gain, identity default) —
included from day one because mod-wheel-controlled vibrato is table stakes for
this instrument too.

**Sources (v1):** Env 1, Env 2, LFO 1, LFO 2, Velocity, Key (octaves relative to
C0 = MIDI 12, VXN1's key-track pivot), Mod Wheel, Pitch Wheel, **Aftertouch**, **Note-on Random
(per-voice humanisation)**. The last two do not exist in VXN1 today and are new
engine additions for VXN1b; they are intended v1 sources, not candidates.

- **Aftertouch is MPE-aware.** Rather than channel-pressure-only, VXN1b tracks
  the originating **MIDI channel through voice allocation** so per-note
  (per-channel) pressure from an MPE controller reaches *that note's* voice as a
  per-voice source. Channel-mode aftertouch (all voices on a channel) is the
  degenerate case of the same path. This threads a `channel` field from note-on
  through the allocator into the voice, and folds pressure events by channel —
  a real but contained change in `vxn1b-clap` (event routing) and
  `vxn1b-engine` (allocator + per-voice pressure state).
- **Note-on Random** is a per-voice RNG value latched at note-on in
  `vxn1b-engine`; decorrelates stacked/unison-adjacent voices.

**Destinations (v1 core):** Pitch (both osc, vibrato-range), X-Mod Sweep (wide,
osc2/mode-aware — inherits VXN1 ADR 0004's mode-gated target table), PWM, LP
Cutoff, Resonance, HPF Cutoff, Amp (VCA), Cross-Mod Amount.
**Destinations (candidates):** Osc1/Osc2/Noise level, per-effect FX wet/mix
(chorus / phaser / delay / reverb / dynamics — see §8), LFO rate. Added if
patches want them; each is one term in the dest apply loop.

### 3. Terms that were hardwired in VXN1 become matrix routes

VXN1 ADR 0004 hardwired several routes into the DSP. In VXN1b they become
ordinary (default-seeded) matrix slots — that *is* the flexibility gain:

- **VCA amp env.** VXN1 hardwired VCA = Env2. VXN1b makes **Amp** a destination
  and the default patch seeds **Env2 → Amp @ depth 1.0** (this mirrors VXN1's
  *original* ADR 0001 §5 matrix, where the VCA gain *was* the Amp column). The
  amp env therefore stays click-free via the same per-sample smoothing; a patch
  may add/replace amp modulation but the factory default behaves like VXN1.
- **Filter key-track.** VXN1's "1 octave of cutoff per octave of key relative to
  C0" fader becomes a **Key → Cutoff** slot; the factory init patch pre-wires the
  route at depth 0.0 (VXN1's `filter_key_track` default), and
  `KEY_CUTOFF_UNITY_DEPTH` = 0.25 is the depth reproducing VXN1's `amt = 1.0`
  exactly — same slope *and* same C0 pivot — but the amount is now free (and can
  go past 1 oct/oct, which VXN1 cannot).
- **Velocity → cutoff, LFO/env → everything.** All the fixed depth faders of ADR
  0004 (`CutoffLfo1Depth`, `VelCutoffDepth`, `PitchLfoDepth`, …) collapse into
  matrix slots.

**Stays hardwired (not a matrix route):** pitch-bend. Host pitch-bend → global
pitch with a **bend-range** param remains a dedicated always-on term (players
expect the wheel to bend regardless of patch). Pitch Wheel is *additionally*
exposed as a matrix *source* for other destinations.

### 4. Application granularity unchanged from VXN1

Source evaluation is **per control block** (control rate ≈ sr/32), exactly as
VXN1 computes its modulators. Destination application keeps VXN1's
consumption-matched smoothing (ADR 0001 §7): per-sample coefficient
interpolation for **cutoff**, per-sample for **pitch** where needed, block-rate
one-pole for gain-like destinations, snap for none (matrix targets are all
continuous). The matrix evaluator produces the same per-dest modulation totals
the fixed loop produced; only the *routing* is generic, so the smoothing story
and the CPU profile are essentially unchanged.

### 5. Parameter model — automatable depths, topology in state

Follow VXN2's split (ADR 0009 "Persistence"):

- **Slot depth** (16 of them) **is** a `clap.params` entry — automatable, so a
  host can automate modulation amount. This matches VXN1's original matrix,
  where depth cells were params, and VXN2's macro pattern.
- **Slot `source` / `destination` / `curve` / `scale_src`** are **patch
  topology**, *not* CLAP params: they live in the state blob and the TOML
  preset, and are not host-automatable. This keeps the automatable surface
  small (the fixed-panel VXN1 exposed ~24 depth params; VXN1b exposes 16 slot
  depths + the retained direct params — comparable, not larger).

The rest of the flat, index-addressed table (VXN1 ADR 0001 §6) is unchanged:
`ParamId` = CLAP id = table index, plain-unit `f32`, one `SharedParams`/
`LocalParams` no-echo mirror, gesture-bracketed UI edits.

### 6. Persistence — sparse TOML + packed binary, VXN2 conventions

- **TOML preset:** a sparse per-slot table (`[[matrix]]` entries with
  `source`/`dest`/`depth`/`curve`/`scale-src` kebab keys; inactive slots
  omitted). Name-keyed, per VXN1 ADR 0005.
- **Binary `clap.state`:** slot topology packed per VXN2 ADR 0009 (active bit +
  source/dest/curve/scale bytes); depth values ride the normal param blob.

VXN1b shares **no preset bytes** with VXN1 — different routing model, different
param set — only the file-format conventions and the shared preset I/O crate.

### 7. UI — one compact faceplate + a mod-matrix overlay

The point of the variant. VXN1 ADR 0004's faceplate row 3 (**Pitch Mod, PWM
Mod, Mod Wheel, Pitch Wheel**) and the **Filter Mod** panel are
**removed** from the front panel entirely. Their function moves into a single
**Mod Matrix overlay**, triggered from the preset bar (`Mod Matrix · N`, the
VXN2 idiom — ADR 0001 §9). Source-shaping panels (LFO1/LFO2 rate+shape, Env1/
Env2 ADSR) stay on the faceplate — they define the *sources*; they just no
longer carry per-destination depth faders.

**Amended (0242):** the **Cross Mod** panel stays. It was removed with the rest
of VXN1's row 3, which over-read this decision: what moves to the overlay is
*modulation routing* — a source, a destination and a depth. Cross-mod type and
amount are **patch topology**, the wiring between the two oscillators (hard
sync / FM / ring), in the same category as Osc 2's octave or the filter's mode.
Dropping them left sync, FM and ring reachable only by host automation. The
amount is *also* a matrix destination (Cross-Mod Amt), applied per voice on top
of the panel's value — the panel sets the wiring, the matrix modulates it, which
is exactly the split this section intends.

Proposed compact faceplate (fewer, denser rows than VXN1's four):

1. Osc 1, Osc 2, Mixer, Filter
2. LFO 1, LFO 2, Env 1, Env 2
3. Cross Mod, Voice, **FX** (one tabbed section — see §8), Master

The matrix overlay is a scrollable list of the 16 slots: per row a source
selector, dest selector, bipolar depth fader, curve selector, and an optional
scale-source selector — reusing VXN1b's ported widgets and the VXN2 matrix-panel
layout. Macro convenience knobs (a "vibrato" knob that writes LFO1→Pitch depth)
are a **post-v1 candidate**, not v1: v1 exposes the matrix directly.

**Accepted cost:** routing is invisible on the main panel. A player reading the
faceplate cannot see what modulates what without opening the overlay — the
"somewhat more opaque" outcome the product brief names. This is the deliberate
trade for compactness and flexibility.

### 8. FX — a tabbed Chorus/Phaser/Delay/Reverb section + a standalone Dynamics panel

> **Amended (0208/0209):** two divergences from the original §8, both driven by
> the faceplate build. (1) **Dynamics is broken out into its own bottom-row
> panel** (6 rotary dials + a Mix fader, VXN2 shape) rather than a fifth FX tab —
> seven controls read better as knobs than as a cramped tab pane; the tabbed
> section is therefore **four** effects (Chorus/Phaser/Delay/Reverb). (2) The
> **serial chain runs Dynamics FIRST** — `dynamics → chorus → phaser → delay →
> reverb` — so input compression/drive sits ahead of the modulation + time
> effects (matches VXN2's FX bus and the faceplate order, Dynamics left of FX).

VXN1 scatters its effects across separate faceplate panels (Chorus, Delay). In
keeping with the compact-panel goal, VXN1b collapses the four time/modulation
effects into **one FX section with tab switching** — a single panel whose tab
strip selects which effect's controls are shown — and gives **Dynamics its own
panel**. Five effects total:

- **Chorus, Phaser, Delay, Reverb** — all four kernels already exist in the
  shared `vxn-dsp` crate (`chorus.rs`, `phaser.rs`, `delay.rs`, `fdn_reverb.rs`)
  and are reused verbatim. VXN1 routes only Chorus + Delay today; VXN1b exposes
  all four.
- **Dynamics** — the one new effect. **Copy VXN2's dynamics block**
  (`vxn-2/crates/vxn2-dsp/src/dynamics.rs`) into the shared `vxn-dsp` crate. The
  copy is *additive* to `vxn-dsp` — VXN1 does not route it, so VXN1 is
  unaffected — consistent with §1's share-the-crate strategy.

**Dynamics carries no dedicated oversampling.** VXN2's dynamics block may run its
own internal oversampling stage; VXN1b **drops that logic**. Dynamics — like the
rest of the instrument — runs at the **single global oversample rate** (the one
1×/2×/4× control from VXN1 ADR 0001 §3). There is no per-effect or per-block
oversampling decision: the whole instrument path is governed by that one rate.
This is a deliberate simplification over VXN2's per-block dynamics OS.

Each effect keeps a header on/off toggle (VXN1 idiom: orange title-bar switch +
body dim). Serial chain order and per-effect wet/mix are engine parameters; the
tab strip is pure UI (it selects *which* effect's params are visible, not signal
routing). Per-effect wet is a **candidate matrix destination** (§2), letting the
matrix modulate FX send amounts — but the FX blocks themselves ship in v1
regardless.

## Consequences

**Positive**

- Arbitrary source→dest routing returns (velocity→PWM, key→reso, LFO2→amp, …) —
  the flexibility VXN1 ADR 0004 gave up.
- The faceplate is materially simpler and more compact (three rows vs four; five
  fixed mod panels deleted).
- Near-total kernel reuse: sharing `vxn-dsp` means VXN1b inherits every DSP fix
  VXN1 lands, and vice-versa, with zero porting.
- The matrix + scale-source is proven code in VXN2; VXN1b adapts a working
  evaluator rather than inventing one.
- FX consolidate into one compact tabbed section; four of five kernels already
  ship in `vxn-dsp`, only Dynamics is a (copied) addition.

**Negative / costs**

- Patch programming is more opaque (routing hidden in an overlay); no at-a-glance
  panel reading.
- A second product to build, bundle, release, and maintain (own CLAP id, own
  factory presets, own xtask target).
- The matrix evaluator is new code in `vxn1b-engine` (the routing loop is *not*
  shared with VXN1, which keeps its fixed resolution) — it must reproduce VXN1's
  smoothing behaviour exactly for the default patch to sound like VXN1.
- **MPE aftertouch** requires threading MIDI channel through note allocation and
  per-voice pressure state — the largest new engine change beyond the matrix
  itself. VXN1's allocator is channel-agnostic today.

**Deferred / candidate (intentional)**

- Osc-level / FX-wet / LFO-rate destinations (added on demand).
- Slot-level `condition` gate (VXN2 has it speced but unshipped) — out of v1.
- Macro convenience knobs that write into slots — post-v1.
- Per-effect FX-wet as matrix *destinations* (the FX blocks themselves ship in
  v1 per §8; routing modulation *into* them is a candidate destination).
- Factory preset bank tuned to the matrix idiom (v1 ships a small init set).
- **Browser/web port deferred** — VXN1b ships desktop CLAP first; a wasm/web
  controller port is a low-risk follow-up on the VXN1/VXN2 precedent, not part of
  the initial build.

## Alternatives considered

- **Feature flag inside VXN1** (`--features matrix` swaps routing + faceplate).
  Rejected: two param tables, two faceplates, and two distinct CLAP plugin ids
  do not coexist cleanly in one crate; the products ship separately and diverge
  in preset format. A sibling product is cleaner and matches the monorepo's
  one-dir-per-synth pattern (vxn-1 / vxn-2 / vxn-3).
- **Copy-adapt `vxn-dsp` into `vxn1b-dsp`** (VXN1's DSP reuse policy). Rejected
  *for v1*: the sound is meant to be identical, so a fork only invites drift.
  Revisit if VXN1b ever wants DSP that VXN1 does not.
- **Re-add the matrix to VXN1 itself** (undo ADR 0004 in place). Rejected: VXN1
  ships and its fixed-panel identity is intentional; VXN1b is an *additional*
  instrument, not a redesign of the shipped one.

## References

- VXN1 ADR 0001 — overall design (engine reused verbatim).
- VXN1 ADR 0004 — fixed-panel modulation (the decision VXN1b reverses).
- VXN1 ADR 0005 — presets (name-keyed TOML, reused).
- VXN2 ADR 0001 §6 — mod matrix as central source/dest engine (the model copied).
- VXN2 ADR 0009 — mod-matrix secondary scale source (the per-route VCA copied).
- `vxn-2/crates/vxn2-dsp/src/dynamics.rs` — dynamics block copied into `vxn-dsp`
  (§8), minus its dedicated oversampling.
</content>
</invoke>
