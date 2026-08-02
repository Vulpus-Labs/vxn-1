---
id: "0219"
product: vxn-1b
title: "3-tab UI shell + Layer 1 / Layer 2 tabs (supersedes 0209/0210 single-patch)"
priority: high
created: 2026-07-31
epic: E039
depends: ["0215", "0216"]
---

## Summary

Build the three-tab faceplate shell and the two **Layer** tabs. Per
[ADR 0002](../../vxn-1b/adrs/0002-vxn1b-dual-layer.md) §8. **Supersedes [[0209]]**
(single-patch 3-row faceplate) **and [[0210]]** (single matrix overlay) — see
[[E039]] "Relationship to E038." Resolve that overlap before starting.

## Design

- **Tab strip**: Layer 1 · Layer 2 · FX/Mixer/Global ([[0220]]). Tabs are pure
  UI (not signal routing).
- **Tab 1 — Layer 1**: full synth patch — Osc1/2, Mixer, Filter, Env1/2, LFO1,
  LFO2 — bound to **Layer 1** param names. Plus Layer 1's **matrix overlay**.
  Always on.
- **Tab 2 — Layer 2**: identical surface bound to **Layer 2** params + Layer 2's
  matrix overlay. Plus a **Layer 2 on/off toggle** that drives KeyMode ([[0215]])
  — off → Single (synth 2 bypassed), on → Dual/Split.
- **Per-layer matrix overlay**: the 16-slot editor (source/dest/depth/curve/
  scale-src) lives **on each layer tab**, bound to that layer's 32-of-total depth
  params + topology. Clean — no cross-layer "Both" rows to render twice.
- **MVC discipline**: view never reads the model; per-layer dirty-bitset pump
  ([[vxn2-mvc-discipline]]). Two overlays = keep the parity test **per layer**.
- Reuse VXN1b's ported HTML widgets (fader, wave-rotary, button-group, segmented
  switch). Bind `data-param` to the two-layer param names from [[0216]].

## Acceptance

- 3-tab shell; Layer 1 / Layer 2 tabs each carry the full patch surface + a
  private matrix overlay, bound to the correct layer's params.
- Layer 2 on/off toggle drives KeyMode; off leaves synth 2 silent.
- MVC parity test passes for **both** overlays (view never reads model mid-drag).
- Contract/token tests (control→param map per layer) pass.
- `vxn1b-clap` GUI extension opens the faceplate in a DAW; loads without JS
  errors.

## Close-out (2026-08-02)

Built in staged chunks (tabs → Layer-2 wire → matrix overlay → review polish).

- **3-tab shell + Layer tabs.** Tab strip (Layer 1 · Layer 2 · FX/Global) in
  [faceplate.html](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.html); the
  Layer tabs flip the edit layer via the existing 0045 rebind
  ([dispatch.js `wireTabs`/`rebindAllForLayer`](../../vxn-1b/crates/vxn1b-ui-web/assets/dispatch.js)),
  now live at `patchCount = 64` (0216). Per-layer panels (incl. LFO 2) grouped in
  the layer pane; globals in the FX/Global pane. Verified
  `__tests__/tab-shell.test.js` (per-layer id resolution, pane switching).
- **Layer 2 enable → KeyMode.** A square header-switch on the Layer 2 tab; its
  click toggles enable + selects the tab. The non-param KeyState crosses to the
  engine: `set_key_mode` → [ui-web `parse_custom_ui`](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs)
  → [clap `on_custom_ui`](../../vxn-1b/crates/vxn1b-clap/src/lib.rs) →
  `SharedParams` KeyState channel → `Engine::set_key_state`. Off → Single (synth 2
  silent). Verified `engine::tests::key_op_maps_mode_to_toggles`,
  `shared::tests::key_op_channel_is_dirty_once_and_carries_state`.
- **Per-layer matrix overlay** (absorbs 0210). A modal opened from the preset bar;
  16-slot editor with vxn-2-style custom combos (source/dest/curve/scale) + a
  bipolar center-origin depth fader (`makeBipolar`, 400 px travel), per layer via
  `refreshForLayer`. Topology edits post `set_matrix` →
  `SharedParams::edit_matrix_slot` → reload. Selectors emit-only + reflect the
  local snapshot (MVC: never read the model); depth rides the gesture-bracketed
  dispatch path. Verified `__tests__/matrix-overlay.test.js`,
  `__tests__/bipolar-fader.test.js`, `shared::tests::matrix_edit_updates_the_right_layer_and_flags_reload`.
- **Contract/token per layer**: `__tests__/param-id-by-name.test.js`,
  `tab-shell.test.js` (lower = upper + patchCount; globals pass through).
- **DAW-open**: opens + plays in Reaper. The "Layer 1 silent" report was a stale
  pre-0216 project state remapped onto the new two-layer table — not a code bug;
  proven by `vxn1b-clap` `layer1_sounds_through_the_full_process_flow` +
  `layer1_sounds_after_a_state_save_load_roundtrip`. Factory reset resolves it;
  state `VERSION` 1→2 rejects genuine old-format blobs.
- **Green**: vitest 177, `vxn1b-engine` 120, `vxn1b-clap` 7, `vxn1b-ui-web` 5.
  **Supersedes 0209/0210/0211** (removed).
