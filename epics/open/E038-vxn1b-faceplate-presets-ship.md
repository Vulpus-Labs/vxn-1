---
id: E038
product: vxn-1b
title: "VXN1b factory presets + release"
status: open
created: 2026-07-25
---

> **Scope reduced (2026-07-31):** the **UI half** of this epic — the compact
> faceplate (0209), matrix overlay (0210), and FX-tab section (0211) — was
> **folded into [[E039]]** when VXN1b gained dual-layer
> ([ADR 0002](../../vxn-1b/adrs/0002-vxn1b-dual-layer.md)). Those three tickets are
> **closed as superseded** (their forked HTML widgets are inherited by E039's
> 3-tab UI, 0219/0220). E038 is now just **factory presets + release**, rebased on
> the two-layer state/preset format delivered by E039 ([[0221]]). E038 ships
> **after** E039.

## Goal

Ship a **factory preset bank** and **release** VXN1b, once E039's dual-layer
engine and 3-tab UI land. This is the ship half of the original faceplate epic;
the UI half moved to E039.

When this epic closes:

- A factory bank ships (embedded via `include_dir`), tuned to the matrix idiom
  and the two-layer surface — including a wheel-gated vibrato demo (scale-source),
  an MPE-pressure demo, and at least one **split** and one **dual-layer** demo.
- The plugin bundles/deploys via xtask; README + PARAMETERS.md updated for VXN1b
  (two layers, KeyMode/split, per-layer matrix, global drift); ADR 0001 + ADR 0002
  → Accepted; `clap-validator` clean; DAW smoke passes.

## Depends on

[[E039]] (dual-layer engine + 3-tab UI + two-layer state/preset format). Presets
here use the [[0221]] format; the release covers the full two-layer instrument.

## Planned tickets

Chain: **0212 → 0213** (presets then ship), after E039.

- [ ] **0212** — Factory preset bank. Init set tuned to the matrix idiom + two
      layers; include a **wheel-gated vibrato** demo (scale-source), an
      **MPE-pressure** demo (aftertouch → cutoff/amp), and **split** + **dual**
      demos exercising both synths + LFO2 sync; embed via `include_dir` (touch
      `factory.rs` before install — [[vxn2-include-dir-no-rerun]]). Round-trip
      through save/reload verified. Uses the [[0221]] two-layer format.
- [ ] **0213** — Release. `vxn-1b/xtask bundle`/deploy path; README +
      PARAMETERS.md for VXN1b (two layers, KeyMode/split, per-layer matrix, drift);
      flip ADR 0001 + ADR 0002 status → Accepted; `clap-validator` clean; DAW
      smoke ([[verify-audio-in-reaper]] — user verifies manually). Epic close-out.

## Superseded tickets

- **0209** compact 3-row faceplate → [[E039]]/[[0219]] (3-tab; widgets inherited)
- **0210** single matrix overlay → [[E039]]/[[0219]] (per-layer overlay)
- **0211** FX tabbed section → [[E039]]/[[0220]] (FX now one global chain)

## Risks

- **Preset legality.** Init presets are original (subtractive, not DX7 rips) — no
  legal posture concern (contrast [[vxn2-factory-preset-legal-posture]]).
- **Format churn.** Presets depend on [[0221]]'s two-layer format landing first;
  don't author the bank against the old single-patch layout.

## Acceptance

- A factory bank ships with matrix, split, dual-layer, wheel-vibrato, and
  MPE-pressure demos; all round-trip through save/reload.
- Bundle/deploy works; `clap-validator` clean; DAW smoke passes.
- README + PARAMETERS.md cover the two-layer instrument; ADR 0001 + ADR 0002
  status → Accepted.
