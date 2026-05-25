---
id: E006
title: Osc interaction polish + fixed-panel UI redesign
status: open
created: 2026-05-25
---

## Goal

Two linked strands:

1. **Finish oscillator interaction** — band-limit hard sync (sub-sample,
   BLEP-softened), add a **ring modulator** (Parker diode model), and recast
   sync/cross-mod as one **Off / Sync / FM** type selector with an amount.
2. **Replace the generic mod-matrix UI with fixed, labelled panels** in the
   JP-8/Juno idiom. The 6×4 source→dest matrix (24 depth params) is ripped out;
   modulation becomes a small set of hardwired routes with per-channel
   source selectors.

The two strands share the param-table rewrite, so they ride one epic. Decisions
recorded in [ADR 0004](../../adrs/0004-vxn1-osc-interaction-and-fixed-panels.md).

## Background

- Hard sync today ([`vxn-dsp::poly::process_pair`]) resets the slave on the
  sample where the master wraps — **sample-accurate, not sub-sample**. Up to ~1
  sample of reset jitter → aliasing sidebands + slightly detuned sync. The
  `TODO(E002 follow-up)` minBLEP note flags exactly this.
- The fix already exists in **`patches-dsp::oscillator`**: `advance_wrap_frac` /
  `advance_all_wrap_frac` (extract the sub-sample master-wrap fraction),
  `sync_reset(frac)` (place the slave phase at `(1-frac)·inc`), and
  `sync_blep_residual(post_phase, post_dt, delta)` (polyBLEP residual that
  band-limits the reset edge — this *is* the analog "softening"). VXN1's poly
  kernel was a stripped copy of those; this re-imports the sync path.
- Cross-mod **depth** already exists (`CrossMod` param, exp2/semitone FM). What's
  missing is the **type selector** and a ring modulator.

## Scope

**In:**

- **Sub-sample BLEP-softened hard sync (0020):** port the three patches-dsp
  primitives into `process_pair`, using the *cross-mod-modulated* `inc1` as the
  master `dt`. Slave reset becomes fractional + polyBLEP-corrected.
- **Poly ring modulator (0021):** SoA port of `patches-modules::RingMod`
  (Parker DAFx-11 diode-bridge: `diodeblock(sig+½c) − diodeblock(sig−½c)`),
  osc1×osc2, summed into the mix via a new **RingLevel**. Drop **brown** noise
  (`NoiseColor` → White/Pink only).
- **Param-model + routing rewrite (0022):**
  - `OscSync` (bool) + `CrossMod` (amount) → `CrossModType` enum {Off, Sync,
    FM} + `CrossModAmount`.
  - **Rip out the 24-cell matrix** (`Env1Pitch … KeyPwm`, `ModSource`/`ModDest`,
    `MATRIX_BASE`, `matrix_index`). Replace with fixed routes (below).
  - **Amp dest gone** — VCA hardwired to Env2.
  - **Key→cutoff** → dedicated filter **key-track on/off** (1 oct/oct over C0).
  - Add **RingLevel**; trim NoiseColor.
- **Fixed-panel UI (0023):** rebuild the editor as the panels below.

**Fixed routing model (replaces the matrix):**

| Channel (dest)        | LFO source            | Env source            | Extra             |
| --------------------- | --------------------- | --------------------- | ----------------- |
| Pitch (both osc, vib) | {Off/LFO1/LFO2}+depth | {Off/Env1/Env2}+depth | Pitch-wheel depth |
| PWM                   | {Off/LFO1/LFO2}+depth | {Off/Env1/Env2}+depth | —                 |
| Cutoff                | {Off/LFO1/LFO2}+depth | {Off/Env1/Env2}+depth | Velocity depth    |
| Osc 2 pitch (wide)    | —                     | {Off/Env1/Env2}+depth | (mod-wheel)       |

The **common Pitch** channel is **vibrato-scaled** (narrow range, both
oscillators). A **separate wide Osc 2 pitch** destination (octave range, osc2
only) drives sync/cross-mod sweeps — fed by an env selector + the mod-wheel.

Mod-wheel is its **own panel** (independent of the above): mod→PWM, mod→cutoff,
mod→reso, **mod→Osc2 pitch** (octave range, sync sweeps). (Replaces today's
single `ModWheelDest` selector.) LFO2's routing is preserved purely through the
per-channel selectors — no dedicated LFO2 cells.

**Panel layout (0023):**

- **Osc 1:** shape, pitch (octave/coarse/fine), PW.
- **Osc 2:** shape, pitch, PW, **cross-mod type {Off/Sync/FM} + amount**.
- **Osc mod:** pitch (vibrato, both osc) ← LFO/env(+pitch-wheel); PWM ← LFO/env;
  Osc 2 pitch (wide, octave range) ← env.
- **Mixer:** osc1, osc2, **ring**, noise levels + noise-type (two buttons:
  white/pink).
- **Filter:** HP cutoff, LP cutoff, reso, **drive**, key-track on/off.
- **Filter mod:** cutoff ← velocity/LFO/env.
- **Mod wheel:** mod→PWM, mod→cutoff, mod→reso, mod→Osc2 pitch (octave range).

**Out (deferred):**

- minBLEP for the cross-mod/FM path itself (still leans on oversampling).
- Ring-mod source routing beyond osc1×osc2 (e.g. noise carrier).
- Preset management (future ADR 0004).
- Env time-scaling by key (ADR 0002 §5 — its own epic).

## Tickets

- [ ] [0020 — Sub-sample BLEP-softened hard sync](../../tickets/open/0020-subsample-blep-sync.md)
- [ ] [0021 — Poly ring modulator + drop brown noise](../../tickets/open/0021-ring-mod.md)
- [ ] [0022 — Param model + routing rewrite (matrix rip-out)](../../tickets/open/0022-fixed-routing-param-model.md)
- [ ] [0023 — Fixed-panel editor rebuild](../../tickets/open/0023-fixed-panel-ui.md)

## Dependency order

```text
0020 (BLEP sync) ── independent (DSP only) ──┐
0022 (param/routing rewrite) ──> 0021 (ring, uses RingLevel) ──> 0023 (UI)
```

0022 is foundational: it owns the table rewrite and the `build_ctx` routing
rewrite, so it lands before the UI and defines `RingLevel`/`CrossModType` that
0021 and 0023 consume. 0020 touches only `vxn-dsp` and can land any time.

## Acceptance

- Hard sync is band-limited: enabling sync no longer adds the broadband
  aliasing of the sample-accurate reset; the reset edge is sub-sample placed and
  polyBLEP-softened; output stays finite for all lanes incl. frozen voices.
- Cross-mod type selector: Off = bit-identical to the independent fast path;
  Sync = band-limited hard sync; FM = today's exp2 cross-mod at the set amount.
- Ring mod: osc1×osc2 through the diode model, mixed by RingLevel; zero on
  either input ⇒ silence; drive shapes harmonic colour. Brown noise removed.
- The 24-cell matrix is gone; modulation flows through the fixed routes with
  per-channel Off/LFO1/LFO2 and Off/Env1/Env2 selectors; mod-wheel routes are an
  independent panel; key-track is a filter on/off doing exactly 1 oct/oct over
  C0; the VCA is hardwired to Env2.
- Editor shows the fixed panels above; no RT allocation; no orphaned params.
