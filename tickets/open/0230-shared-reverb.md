---
id: "0230"
product: monorepo
title: "Shared FdnReverb — vxn-2 canonical (linear mix + internal fade); vxn-1's equal-power law retired"
priority: medium
created: 2026-08-02
epic: E041
depends: ["0227"]
---

## Summary

Third ticket of [E041](../../epics/open/E041-shared-fx-unification.md). The
two FDN-8 reverbs ([fdn_reverb.rs](../../vxn-1/crates/vxn-dsp/src/fdn_reverb.rs)
vs [reverb.rs](../../vxn-2/crates/vxn2-dsp/src/reverb.rs)) share topology,
`BASE_MS` tables, and LFO scheme; they diverge in exactly two mechanical ways:
vxn-1 mixes equal-power (`√(1-m)·dry + √m·wet`) with bypass delegated to the
outer crossfade; vxn-2 mixes linear with internal `Smoothed` mix +
`mix_primed` + bit-exact passthrough. vxn-2's form is canonical (locked
decision).

**This is the largest perceptual change in the extraction plan**: vxn-1/1b
mid-mix reverb level shifts (equal-power → linear dips ~3 dB at mix=0.5 for
uncorrelated wet).

## Acceptance criteria

- [ ] Move commit: `FdnReverb` (vxn-2 body) → `vxn-core-dsp::reverb`,
      `FxKernel` impl; vxn-2 hash unchanged.
- [ ] Adoption commit (vxn-1 + vxn-1b together): both construct shared
      `FdnReverbParams`; `reverb_fade` + REVERB slot fade deleted. Reverb tail
      rings through a fade-out (kernel held on; WetFade owns bypass — same
      split both engines already use).
- [ ] `REBASELINE:` commit: vxn-1 baseline + reverb-toggle declick, vxn-1b
      goldens; rendered A/B captures noted; user signs off in Reaper before it
      lands.

## Notes

If the mid-mix level drop is musically unacceptable on existing vxn-1 patches
(factory + [[vxn1-jovian-presets]]), fallback is a one-line wet-gain
compensation curve in vxn-1's param mapping — decide at listen time, keep the
kernel canonical either way.
