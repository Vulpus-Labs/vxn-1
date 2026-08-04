# ADR 0002 — VXN1b dual-layer (two independent synths + per-layer matrix)

- **Status:** Proposed
- **Date:** 2026-07-31
- **Scope:** Reintroduce VXN1's keyboard **split / dual-layer** capability into
  VXN1b, but as **two fully independent synth instances** rather than VXN1's
  shared-pool + `param_source` routing. Each layer carries its own patch, voice
  pool, allocator, and **its own private mod matrix**. Partially revises the
  single-patch decision in [ADR 0001](0001-vxn1b-overall-design.md) §7; keeps
  ADR 0001's matrix-modulation thesis intact (now ×2).

## Context

[ADR 0001](0001-vxn1b-overall-design.md) made VXN1b **single-patch** — patch and
global params collapsed into one flat table — as a deliberate simplification
against VXN1's Upper/Lower two-layer model. In use, the keyboard split /
dual-layer feature turns out to be one of VXN1's stronger features (splits,
layered timbres, stereo doubling). Reintroducing it is worth the cost.

VXN1's model shared **one 16-voice pool** across both layers and routed each note
to a layer via `param_source` / `note_on`
([vxn-engine/src/lib.rs:713-762](../../vxn-1/crates/vxn-engine/src/lib.rs#L713-L762)),
with `KeyMode` ∈ {Whole, Dual, Split} + a split point held as non-automatable
blob state. Per-layer modulation there was a handful of **fixed panels** (Pitch
Mod, PWM Mod, Filter Mod) — cheap to duplicate.

VXN1b replaced those fixed panels with a **generic 16-slot matrix** (ADR 0001
§4). Duplicating *that* per layer is far heavier than duplicating fixed panels —
which is the whole reason this needs its own ADR.

## Decision

### 1. Two independent synth instances, not a shared pool

The core synth — voices + patch + **matrix** + drift consumers — becomes an
instantiable `Synth` unit. The plugin holds **2 × `Synth` + a global block**
(FX, mixer/balance, master level/pitch/drift/limiting, MIDI demux). This is a
departure from VXN1's shared pool: allocation, voice stealing, and twin/unison
are **private to each synth**. No shared allocator, no `param_source`
indirection.

Consequence — **voice budget**: **16 voices per synth (32 max)**. Single mode
leaves synth 2 fully bypassed (1× cost); the ~2× voice CPU is paid only when the
user opts into dual/split. The render loop has the headroom
([[vxn1-render-loop-optimized]]: dry_4x 51× RT).

### 2. MIDI demux: route-on / broadcast-off

A thin demux sits in front of the two synths:

- **Single**: synth 2 off; all events → synth 1.
- **Dual**: every event fanned to **both** synths.
- **Split**: note-ons routed by pitch vs split point (below → Lower, at/above →
  Upper). Other events (CC, wheels, pressure) fanned to both.

**Note-offs are always broadcast to both synths**, in every mode. The owning
synth releases the note; the other has no matching held note → no-op. This fixes
the split-move stuck-note bug (note-on routed at press time, split point moves,
note-off would otherwise route to the wrong synth) **without** per-note owner
tracking and **without** cutting held notes — they ring out on their origin
synth (standard hardware behavior). Residual: same pitch held on *both* synths
resolves to one note-off releasing both — inherent MIDI same-pitch ambiguity, no
worse than a single mono-per-pitch synth.

### 3. KeyMode from a Layer-2 on/off toggle + split enable

VXN1's 3-way `KeyMode` is derived from two UI controls:

- Layer 2 **off** → `Single` (today's behavior).
- Layer 2 **on**, split disabled → `Dual` (both layers, full range).
- Layer 2 **on**, split enabled → `Split` at the mixer-tab split point.

`KeyMode` + split point are **non-automatable blob state** (as in VXN1).

### 4. Per-layer private mod matrix

Each `Synth` owns a complete 16-slot matrix (topology + depths). No pooled slots,
no cross-layer "Both" tag, no cross-layer routing — a slot's source and dest are
always within its own layer. All modulation sources are **per-layer**: Env1,
Env2, LFO1, **LFO2** (VXN1b has *no* global LFO), plus the per-voice/controller
sources.

Consequence — **param count**: matrix depths double to **32 automatable** CLAP
params; patch block doubles (69 → 138); matrix topology blob is stored ×2. Total
lands ≈ 180 CLAP params (VXN1's 165 territory). This is the deliberate cost of
full independence over the (rejected) pooled-slot-with-layer-tag model (~154).

### 5. Cross-layer coupling: LFO2→LFO2 sync only

The single cross-layer link is an **LFO2 sync** flag on Layer 2: when set,
Layer 2's LFO2 slaves to Layer 1's LFO2 as master — **rate + phase lock** (L2
mirrors L1's phase accumulator), giving true locked stereo movement. "Synced
modulation, different timbre" is achieved by wiring `LFO2 → <dest>` in *each*
layer's own matrix and enabling sync — same shape, independent timbres.

Surfaced as **"Link"**, not "Sync" (0217): `lfo2_sync` is already the per-layer
*tempo*-sync param, so the cross-layer flag is `KeyState::lfo2_link` / the
`set_lfo2_link` UI op. The slave keeps its own LFO 2 **shape** (only phase, and
therefore rate, is taken from the master), so a linked pair can run different
shapes off one phase.

### 6. Global drift, ported unchanged from VXN1

`MasterDrift` [0,1], default 0, is a **single global control** (Tab 3), applied
to all voices in both synths — not per-layer. Ports VXN1's mechanism verbatim:
per-voice bounded random walk on osc pitch (±0.125 st @ 1.0, sub-Hz) + static
per-voice trim draws (env times ±12%, sustain ±3%, reso ±7%, cutoff ±3¢); no
driftable-param list, targets baked in DSP
([vxn-engine/src/voice.rs:65-140](../../vxn-1/crates/vxn-engine/src/voice.rs#L65-L140)).

### 7. Single global FX / mixer

FX is **one global instance**; both synths mix into it. FX is *not* duplicated
per layer. Global level / pitch / drift / limiting also live here.

**Amended (0220).** This section originally specified a single "layer balance"
control. Superseded by **two independent per-layer level faders, each with a
mute and a live stereo meter**. A balance control cannot set *absolute* layer
levels — which is what preset design needs — and it forces a taper choice
(unity-at-centre vs. equal-power) that is wrong for one of the two use cases
either way. `layer_level` / `layer_mute` are **patch** params, so the two-layer
expansion yields one instance per synth and a preset carries its own mix. Mute
folds into the same smoothed gain as level rather than gating the render, so a
muted layer keeps running and unmuting resumes a held note mid-flight. Metering
rides the shared spine (ticket 0240).

### 8. UI: three top-level tabs

- **Tab 1 — Layer 1**: full synth patch (Osc1/2, mixer, filter, Env1/2, LFO1,
  LFO2) + Layer 1's matrix overlay. Always on.
- **Tab 2 — Layer 2**: same surface + an on/off toggle. Off → single-patch.
- **Tab 3 — FX / Mixer / Global**: per-layer level + mute + meter → FX (see §7's
  amendment), split enable + point, all FX params, master level / pitch / drift /
  limiting. The **LFO 2 Link** toggle stays on the LFO 2 panel (0217/0220) rather
  than here — it reads better beside the LFO it governs.

The matrix overlay is **per-layer** (lives on each layer tab) — clean because
there are no cross-layer "Both" rows to render twice.

## Consequences

- **Supersedes** ADR 0001 §7's single-patch decision; ADR 0001's matrix thesis
  (§4) and DSP reuse (§2) stand, now instantiated twice.
- **Reshapes [[E038]]** (unbuilt): its single-patch 3-row faceplate (0209),
  single matrix overlay (0210), and FX-tab section (0211) are replaced by the
  3-tab structure here. E039 (this ADR's epic) sequences before / absorbs those.
- CPU up to ~2× in dual/split (opt-in); param count ≈ 180; preset + host-state
  format must carry two layers + KeyMode/split.
- Persisted state format changes — needs a version bump / migration for any
  single-patch VXN1b presets already saved.

## Alternatives rejected

- **Pooled 16-slot matrix with per-slot Upper/Lower/Both tag** (~154 params,
  half the matrix automation surface). Elegant and cheaper, but the user wants
  fully independent per-layer matrices; the pooled model can't give a layer more
  than a shared 16-slot budget.
- **VXN1's shared 16-voice pool + `param_source`.** Rejected for the
  two-instance model: cleaner (no routing indirection), and makes voice
  stealing / unison a per-synth concern instead of a cross-layer one.
- **Range-kill on split move** (emit note-offs for held notes in the moved
  range). Works but cuts held notes and needs a move-triggered sweep;
  broadcast-off (§2) is strictly simpler and lets notes ring out.
