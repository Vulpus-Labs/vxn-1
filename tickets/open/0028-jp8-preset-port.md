---
id: "0028"
title: Jupiter-8 factory preset port
priority: low
created: 2026-05-28
epic: E007
---

## Summary

The fun payload of E007: author a curated set of **Jupiter-8-flavoured** factory
presets (~16–24 patches across categories, plus a few Dual/Split performances)
in the 0024 TOML format, and record the JP-8 → VXN1 mapping and its divergences.
This is **honest archetypes, not a ROM clone** — we have no original JP-8 patch
data, and the platforms differ (see the mapping table). Framed as "Jupiter
character", not "the factory bank". Builds on the format (0024) and bank infra
(0025); auditioned via the browser (0027). Decisions:
[ADR 0005](../../adrs/0005-vxn1-presets.md) §Consequences.

## Acceptance criteria

- [ ] `crates/vxn-engine/presets/factory/jp8/` populated with ~16–24 `.toml`
  presets, spanning the categories below; each with `tags = ["jp8", ...]` and a
  `meta.comment` noting its hardware inspiration **and** the main divergence.
- [ ] Coverage across archetypes (1–3 each): **Brass** (the signature JP-8 stab /
  swell), **Strings/Ensemble**, **Pad/Sweep**, **Sync Lead**, **Bass** (incl. a
  unison/Twin fat bass), **Bell/Pluck/FM**, **Poly Keys/Clav**, and at least one
  **Performance** Split (e.g. bass / lead) and one **Dual** layered stack.
- [ ] All pass 0025's CI round-trip (parse + zero warnings).
- [ ] The mapping/divergence table below is reproduced (or linked) wherever the
  JP-8 set is documented, so future authors and players understand the
  translation choices.
- [ ] Sanity-listened in a host: each preset plays and is recognisably its
  archetype across a couple of octaves; unison/Twin patches don't clip (level
  compensation is in the engine).

## JP-8 → VXN1 mapping

| JP-8 feature | VXN1 target | Notes / divergence |
|---|---|---|
| VCO-1 waves (saw / pulse / square) | `osc1_wave` Saw/Pulse | "square" = Pulse at `osc1_pw = 0.5` |
| VCO-2 waves (saw / pulse / tri / sine) | `osc2_wave` | VXN1 osc2 adds Sine/Tri natively — good fit |
| VCO-2 = **noise** | `noise_level` (+ `noise_color`) | noise is a **mixer source** in VXN1, not an osc-2 wave |
| VCO range / octave switches | `osc{1,2}_octave`, `osc{1,2}_coarse` (±7 st) | coarse can't span a full octave; use octave for ±12 |
| VCO-2 **Low-Freq** mode | (no direct map) | VXN1 has dedicated LFOs; emulate slow movement with LFO1/2 instead. Documented loss. |
| **XMOD** (VCO-1 freq ← VCO-2) | `cross_mod_type = "FM"` + `cross_mod_amount` | exp2/semitone FM; aliases for non-sine carriers (by design, [[vxn1-crossmod-pm-aliasing-by-design]]) — lean on `oversample` |
| **VCO SYNC** | `cross_mod_type = "Sync"` | band-limited (E006/0020). **Mutually exclusive** with FM in VXN1 (JP-8 had separate switches) — pick the dominant one per patch |
| Source mixer (VCO-1 / VCO-2) | `osc1_level` / `osc2_level` | ring (`ring_level`) is a VXN1 extra, use sparingly |
| **HPF** 4-step (0/1/2/3) | `hpf_cutoff` (Hz) | continuous in VXN1; map steps → ~`20 / 120 / 360 / 1000` Hz (20 ≈ off). Tune by ear. |
| VCF cutoff / reso | `cutoff` / `resonance` | OTA ladder is the same IR3109 family — strong fidelity |
| VCF -12 / -24 dB | `filter_slope` 12 dB / 24 dB | direct |
| VCF env amount + **ENV polarity** | `cutoff_env_depth` (bipolar) | negative depth = inverted env (replaces the polarity switch) |
| VCF key-follow | `filter_key_track` (on/off) | VXN1 is 1 oct/oct over C4, on/off only (JP-8 had a pot) — coarser |
| VCF ← LFO | `cutoff_lfo1_depth` / `cutoff_lfo2_depth` | choose LFO1 (per-voice) or LFO2 (global) per intent |
| VCA ← ENV-2 / gate | hardwired Env2; `amp_env_bypass` for gate | direct |
| ENV-1 / ENV-2 (ADSR) | `env1_*` / `env2_*` | Env1 = filter env, Env2 = amp env (VXN1 convention) |
| ENV shape | `env{1,2}_shape` Lin/Exp | JP-8 is roughly exponential — prefer Exp for amp |
| LFO (sine/saw/square/random) + rate | `lfo_shape` / `lfo_rate` (LFO1) and/or `lfo2_*` | VXN1 shapes are a superset (Tri, Saw±, S&H) |
| LFO **per-section** | LFO is **per-voice** (LFO1) or **global** (LFO2) | per-note vibrato → LFO1; patch-wide sweep → LFO2. Modern divergence ([[vxn1-feature-roadmap]]) |
| **Bender** lever (→ VCO / VCF depth) | `pitch_wheel_depth`; mod-wheel panel | JP-8 had no mod wheel; route expressive depth via `pitch_wheel_depth` and the `mod_wheel_*` panel |
| **Unison / Solo** | `assign_mode` Unison / Solo / Twin; `unison_detune` | VXN1 = 16 voices vs JP-8's 8; Twin is a VXN1 extra |
| Whole / Dual / Split | `key_mode` + `split_point` (Performance only) | direct (ADR 0003) |
| Aftertouch / **velocity** | leave `vel_cutoff_depth = 0` | JP-8 hardware had no velocity — keep authentic ([[vxn1-status]]) |
| (no onboard chorus) | `chorus_on = false` by default | chorus is Juno, not JP-8 — off for authenticity; a tasteful variant may enable it, noted in `comment` |
| Analog drift / VCO instability | (none) | no global drift model; `unison_detune` + phase decorrelation approximate thickness only. Documented loss. |

## Notes

- **Authoring stance:** build by ear from documented JP-8 synthesis recipes and
  the archetype, not from any claimed original patch data. Name presets
  descriptively (e.g. "Jupiter Brass", "Sync Lead", "Octave Bass") — avoid
  implying a specific ROM slot.
- **Sparse files:** only write params that deviate from default; lean on the
  format's default-fill so the JP-8 set auto-adopts engine default improvements.
- **Mutually-exclusive osc interaction** is the sharpest divergence: a JP-8 patch
  using sync *and* cross-mod must collapse to one `cross_mod_type` — choose the
  audibly dominant behaviour and note it in the patch comment.
- **Oversample:** sync/FM patches alias by design on non-sine carriers; set a
  higher `oversample` on those presets (it's a global/per-performance param) and
  note it.
- Could grow into its own content epic if it expands past ~24 patches or wants a
  proper documented "JP-8 sound design" write-up — keep it one ticket for now.
- This is the deliverable to **listen to**, not just test. Ask before any GUI /
  screen capture ([[ask-before-screen-capture]]).
