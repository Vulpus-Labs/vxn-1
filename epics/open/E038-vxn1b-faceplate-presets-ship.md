---
id: E038
product: vxn-1b
title: "VXN1b faceplate + matrix overlay + FX tabs, factory presets, release"
status: open
created: 2026-07-25
---

## Goal

Give VXN1b its **compact faceplate**, the **mod-matrix overlay**, and the
**tab-switched FX section**; ship a factory bank; and release. This is the
UI/ship half of [ADR 0001](../../vxn-1b/adrs/0001-vxn1b-overall-design.md)
§7–§8 — the payoff of the whole variant (simpler, more compact front panel).

When this epic closes:

- The faceplate is a **3-row** compact layout (Osc1/Osc2/Mixer/Filter · LFO1/
  LFO2/Env1/Env2 · Voice/FX/Master); VXN1's five fixed mod panels + Filter Mod
  panel are **gone**.
- A **Mod Matrix overlay** (preset-bar trigger `Mod Matrix · N`) edits all 16
  slots — source, dest, bipolar depth, curve, scale-source per row — under MVC
  discipline (view never reads the model).
- The **FX section** is one panel with a tab strip selecting
  Chorus/Phaser/Delay/Reverb/Dynamics controls; per-effect header on/off.
- A small factory bank ships (embedded via `include_dir`), including a
  wheel-gated vibrato demo (scale-source) and an MPE-pressure demo.
- The plugin bundles/deploys via xtask; docs updated; ADR → Accepted.

Depends on [[E036]] (param table + matrix + sources) and [[E037]] (FX chain) —
the UI binds to their params/state.

**Web/browser port is deferred out of this epic** (and the product for now). The
VXN1/VXN2 web ports set a simple, well-trodden precedent; a VXN1b port is a
later, low-risk follow-up once the desktop plugin ships.

## Why now

The engine ([[E036]]/[[E037]]) is DAW-playable with host knobs but the *product*
thesis — a simpler, more compact faceplate with flexible routing — only lands
with the real UI. This epic delivers that surface and ships.

## Design (locked by ADR 0001)

- **Faceplate.** Reuse VXN1b's ported HTML widgets (fader, wave-rotary,
  button-group, segmented switch, preset bar). 3 rows; delete the ADR 0004
  row-3 mod panels + Filter Mod. Source-shaping panels (LFO/Env) stay.
- **Matrix overlay.** Scrollable 16-slot list; per row: source selector, dest
  selector, bipolar depth fader, curve selector, optional scale-source selector
  (`—`/None default reads as off). MVC: view emits change events, never reads
  the model (VXN2 dirty-bitset pump idiom). Macro convenience knobs are post-v1.
- **FX tabs.** Single panel; tab strip selects which effect's params show; tabs
  are pure UI (not signal routing). Header on/off per effect (orange title-bar
  toggle + body dim, VXN1 idiom).
- **Presets.** Name-keyed sparse TOML, embedded factory bank via `include_dir`
  (touch `factory.rs` before install — `vxn2-include-dir-no-rerun`).

## Planned tickets

Chain: **0209 → 0210 → 0211** (faceplate then overlays), **0212 → 0213**
(presets then ship). (0207/0208 were taken by E037/E036 work; renumbered.)

- [ ] **0209** — Compact faceplate HTML/CSS. Fork VXN1's `faceplate.html`; 3-row
      layout; port osc/mixer/filter/LFO/env/voice/master panels; **remove** the
      Pitch Mod / PWM Mod / Cross Mod / Mod Wheel / Pitch Wheel / Filter Mod
      panels. Wire to `vxn1b-ui-web` param bindings. Contract/token tests.
- [ ] **0210** — Mod-matrix overlay. 16-slot scrollable editor (source/dest/
      depth/curve/scale-src per row) reusing shared selectors; preset-bar
      `Mod Matrix · N` trigger; MVC parity (view never reads model). Emits slot
      topology + depth edits as events; reflects state on idle poll. Tests.
- [ ] **0211** — FX tabbed section UI. One panel, tab strip for Chorus/Phaser/
      Delay/Reverb/Dynamics; per-tab controls bound to the E037 FX params;
      per-effect header on/off toggle. Token/contract tests.
- [ ] **0212** — Factory preset bank. Small init set tuned to the matrix idiom;
      include a **wheel-gated vibrato** demo (scale-source) and an **MPE-pressure**
      demo (aftertouch → cutoff/amp); embed via `include_dir`. Round-trip through
      save/reload verified.
- [ ] **0213** — Release. `vxn-1b/xtask bundle`/deploy path; README +
      PARAMETERS.md for VXN1b; flip ADR 0001 status → Accepted; `clap-validator`
      clean; DAW smoke (`verify-audio-in-reaper` — user verifies manually). Epic
      close-out.

## Risks

- **Overlay density.** 16 slots × 5 selectors is a lot of UI; the `—`/None
  scale-source default and empty rows must read as "off" at a glance so a sparse
  patch looks calm, not cluttered.
- **MVC discipline.** The overlay is the most stateful new view; a view that
  reads the model mid-drag reintroduces the input-stomp class of bug
  (`vxn1-vizia-automation-relayout-input-stomp` lineage). Enforce with a parity
  test.
- **Preset legality.** Init presets are original (subtractive, not DX7 rips) — no
  legal posture concern (contrast `vxn2-factory-preset-legal-posture`).

## Acceptance

- The faceplate is the 3-row compact layout with no fixed mod panels; it reads
  simpler than VXN1's.
- The matrix overlay edits all 16 slots (source/dest/depth/curve/scale-src) under
  MVC discipline; changes round-trip to state/preset.
- The FX section is one tabbed panel with per-effect on/off; all five effects
  reachable.
- VXN1b runs in a DAW; a factory bank ships with the wheel-vibrato and
  MPE-pressure demos.
- Bundle/deploy works; `clap-validator` clean; ADR 0001 status → Accepted.
</content>
