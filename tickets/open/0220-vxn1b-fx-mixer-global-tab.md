---
id: "0220"
product: vxn-1b
title: "FX / Mixer / Global tab — layer balance, split, FX, master (supersedes 0211)"
priority: high
created: 2026-07-31
epic: E039
depends: ["0215", "0217", "0218", "0219", "0240"]
---

## Summary

Build **Tab 3 — FX / Mixer / Global**. Per
[ADR 0002](../../vxn-1b/adrs/0002-vxn1b-dual-layer.md) §7–§8. **Supersedes
[[0211]]** (single-patch FX-tab panel) — FX is now one global chain shared by
both layers.

## Design

- **Mixer**: two **level faders side by side**, one per layer, each with a
  **mute** button and a **live stereo meter**, summing into the **single global
  FX** chain. Both `Synth`s mix here, not doubled FX.

  This **replaces ADR 0002 §7's single "layer balance" control** — two
  independent faders are what preset design actually needs (a balance knob can't
  set absolute layer levels), and the meters make the mix readable rather than
  guessed. See the ADR amendment note below.

  `LayerLevel` + `LayerMute` are **patch** params, not globals: putting them in
  `PATCH_PARAMS` gets one instance per layer from the existing two-layer
  expansion, and means a preset carries its own layer levels. This is VXN1's
  idiom (its `layer_level`), and the UI's
  [`paramIdByNameAtLayer('layer_level', …)`](../../vxn-1b/crates/vxn1b-ui-web/assets/panels/keys.js#L169)
  already assumes that shape. `PATCH_COUNT` 64→66, `TOTAL_PARAMS` 160→164;
  `layer_level` comes off the `fixed_mod_panel_params_are_gone` removal list.

  Metering rides the [[0240]] spine — two taps (L1/L2 post-fader stereo), no
  bespoke transport.
- **Split**: split **enable** toggle + **split point** (MIDI note). Only
  meaningful when Layer 2 is on; drives KeyMode ([[0215]]).
- **FX params**: all FX controls (Chorus/Phaser/Delay/Reverb/Dynamics), reusing
  the E037 FX chain params, with per-effect header on/off. May reuse a tab strip
  *within* this tab for the effects, or lay them out flat — whichever reads
  cleaner in the compact faceplate.
- **Global**: master **level / pitch / drift ([[0218]]) / limiting**.
- **LFO2 link** ([[0217]]): **stays in the LFO 2 panel strip** where 0217 put it,
  *not* on this tab. It reads better next to the LFO it affects; this supersedes
  the original "lives here" note above. It remains one `KeyState` flag with one
  cell — not mirrored in two places.
- **MVC**: view never reads model; same dirty-bitset pump. Meters are the one
  view-bound audio→UI push, and they carry no model state — a meter frame is
  never read back by the view as a value.

## Acceptance

- Tab 3 exposes: two per-layer level faders with mutes and live stereo meters
  feeding the global FX, split enable + point, all FX params (per-effect
  on/off), master level/pitch/drift/limiting.
- FX is a single global instance; both layers audibly mix through it at their
  fader levels; a muted layer contributes silence but keeps rendering state
  (no click on unmute, no stuck notes).
- Layer meters track their layer's post-fader output and read zero when muted.
- Split enable + point drive KeyMode; inert when Layer 2 off.
- `LayerLevel`/`LayerMute` round-trip through host state (the blob grows —
  coordinate with [[0221]]).
- Contract/token tests pass; loads without JS errors; opens in a DAW.

## Notes

**ADR 0002 amendment.** §7 specifies "both synths mix into it via a layer-balance
control". Superseded here by two independent level faders + mutes + meters.
Rationale: a single balance control cannot set absolute layer levels, which is
what preset design needs, and it forces a taper choice (unity-at-centre vs.
equal-power) that is wrong for one of the two use cases either way. Amend §7 when
this lands.
