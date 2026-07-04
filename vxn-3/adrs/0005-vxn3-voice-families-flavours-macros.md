# ADR 0005 — VXN3 voice families, flavours, and editable macro bindings

- **Status:** Proposed
- **Date:** 2026-07-04
- **Scope:** How VXN3's voice roster is organised so that (a) many drum sounds
  are reachable from a small, RT-safe engine set, and (b) the sounds are
  *playable-with* — editable and extensible by hand — rather than a fixed menu.
  Supersedes the informal "3 engines" roster of ADR 0001 §3a with a structured
  model; leaves the host-param split of ADR 0003 intact.

## Context

E021 shipped the groove proof with three thin engines (Kick/Tone, Metal, Noise)
and E032 gave them a host surface: **3 generic macro slots per track**, the
active engine reinterpreting each slot onto its patch (ADR 0003). The roadmap now
pivots deliberately: **make VXN3 an interesting toy before a complete
instrument**, so that playing it reveals what it should become. That means
expanding the voice roster *early*.

The reference material is the sibling `patches-drums` repo — 17 synthesis modules
across 7 drum categories (kick, snare, hat, tom, clap, claves, cymbal). Crucially
those 17 modules collapse to **a few synthesis families**, and within a family the
different drums are not different code — they are **different points in one
family's parameter space**. `Kick` and `Tom` are the same driven-oscillator
circuit at different pitch / sweep / decay settings; `ClosedHiHat` and `Cymbal`
are the same metallic bank at different tunings and decays.

`patches-drums` exposes those params **flat, with no macro layer**. VXN3's playable
surface is the opposite: **3 macros per track**, and a deep faceplate patch. The
design question is therefore not "how many engines" but: *how do a family's full
parameter space, a small set of playable macros, and the named drum sounds relate
— such that all of it is editable and new sounds can be defined?*

## Decision

Adopt a three-tier model: **Family → Flavour → Macro binding.**

### Family (= engine)

A family is one synthesis architecture with a **full parameter space** `P`. Each
param carries metadata: stable id, display name, unit, range, default, response
curve. A family is RT-safe and SoA-vectorised as today (ADR 0001). The roster is
**four families**, covering both analog schools of every `patches-drums` category:

| Family | Model | Covers (as flavours) |
|---|---|---|
| **Driven** | phase-accum sine + pitch sweep + amp env, drive/click | kick, tom, snare-body, claves |
| **Noise** | noise → band/high-pass + burst gate + snap | snare-noise, clap |
| **Metal** | inharmonic bank (metallic/XOR/modal) + HP noise + shimmer | closed/open hat, ride, cymbal |
| **Struck** | BridgedT struck resonator — pitch-droop, Q=decay, excitation shape | kick2, tom2, claves2, modal cymbal |

Three families are today's engines *enriched* to cover their category's full range;
**Struck** (the `patches-drums` "2" resonator school) is new. This is the whole
roster — closed, per ADR 0001. It replaces 17 modules with 4 engines + data.

### Flavour (= a named point in a family's space)

A **flavour** is a named configuration over one family:

1. a **base vector** — a value for every param in `P` (the "fixed" layer);
2. a **macro-binding table** — for each of the `K = 3` macro slots, the subset of
   `P` it drives, each with a **depth** and **curve** (the "modulated" layer);
3. **default macro values** the flavour ships with.

"Kick", "Tom", "Claves" are three flavours of **Driven**. A flavour is **data,
not code** — authored as a small TOML record, shipped via `include_dir!` like the
vxn-2 factory bank ([[vxn2-preset-system]], mind [[vxn2-include-dir-no-rerun]]).

### Macro binding (additive-from-base)

A binding is `(macro_slot → param, depth, curve)`. Evaluation, on each trig (not
per-sample):

```
final(p) = clamp( base[p] + Σ_{i : slot_i binds p} curve_i(macro_i) · depth_i , range(p) )
```

A param may be driven by more than one macro; an unbound param just uses `base[p]`.
This is a **deliberately constrained modulation matrix** — one source type (a macro
knob), destination = any family param, additive depth — not the general matrix of
vxn-2 ([[vxn2-architecture]]). Small on purpose; expands only if play demands it.

### Everything is editable — four layers

| Layer | What | Where it lives | Automatable? |
|---|---|---|---|
| Macro **values** | the 3 played knobs | host param table (ADR 0003, 0171) | yes — host |
| **Base** params | fixed layer of the flavour | faceplate deep patch | no (faceplate/state) |
| Macro **bindings** | which params a macro drives, depth, curve | faceplate deep patch | no |
| **New flavour** | snapshot base + bindings under a name | flavour store (factory + user) | — |

The host still sees only the fixed 3 macros/track (ADR 0003 unchanged); base +
bindings are the **faceplate-only layer**, serialised into `clap.state` as the
per-track deep patch (**ticket 0179** fills the reserved `patch_len` bytes — the
"patch" it serialises *is* the flavour: base vector + binding table + version).

A **kit** is then just `N_TRACKS × (family, flavour)` + mix/master — the unit a
future preset epic persists.

## Consequences

- **0179's patch blob = a flavour** (base + binding table + patch-version tag),
  not a flat value list. Get that layout right; the flavour store and kit epic read
  the same bytes.
- **Faceplate grows a flavour editor**: base sliders, a binding-assignment surface
  (pick param, set depth/curve per macro), and *save-as-flavour*. This is the
  "playable-with" payoff and the bulk of the UI work.
- **Roster is small, content is large.** New drums are usually new *flavours*
  (data), not new engines (code). Enriching a family's `P` is the only code cost.
- **`value_to_text` (0172) becomes flavour-aware**, not just engine-aware: a macro's
  text depends on what the current flavour bound it to ("Decay 0.42 s"). Extends the
  0172 pure-dispatch discipline; still no reach into the audio-thread engine.
- **RT cost is negligible**: binding eval is a per-trig sum over ≤ `K` macros ×
  their bound params, allocation-free; the per-sample kernels are unchanged SoA.
- **Open/closed hat** is expressible either as two flavours linked by a choke group
  (Phase 1) or one flavour with a choke-driven decay param — defer to the Metal
  family ticket.

## Alternatives considered

- **17 modules as 17 engines.** Rejected: fights the closed SoA roster (ADR 0001),
  massive code surface, and no shared editable space — Kick and Tom couldn't morph
  into each other, which is the whole point of "play to discover".
- **Fixed (non-editable) macro maps per engine** (status quo from 0170). Rejected by
  this ADR's driving insight: if flavours are points in a shared space, the *binding*
  is part of the sound and must be editable, else new flavours are impossible.
- **Full general mod-matrix** (vxn-2 style). Deferred: more than play currently
  needs. The additive-from-base macro binding is the minimal thing that makes
  flavours editable; widen later only if the toy phase asks for it.

## Open questions (resolve during E034 by playing)

- Curve set (linear / exp / S) and whether curve is per-binding or per-param.
- Do macro **values** belong to the flavour (shipped defaults) or purely to
  performance/automation state? Leaning: performance state; flavour ships defaults.
- Minimum viable `P` per family to cover its flavours without bloat — discovered by
  authoring the factory flavours, not specified up front.
