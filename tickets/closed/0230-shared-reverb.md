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
two FDN-8 reverbs ([fdn_reverb.rs](../../vxn-1b/crates/vxn-dsp/src/fdn_reverb.rs)
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

## Close-out (2026-08-30)

Three commits: the LFO sine down a layer (`ca15b6c`), the kernel move
(`b9999a1`), then `WetFade::reset` and vxn-1b's adoption (`2d10df8`).

### The ticket's premise was false, and had been for six weeks

This ticket is billed as **the largest perceptual change in the extraction
plan**: vxn-1b mixes equal-power, vxn-2 linear, adopting vxn-2 dips mid-mix
reverb ~3 dB, with a wet-gain compensation curve held in reserve if patches
suffer. None of that applies. vxn-2's code is:

```rust
let mix = self.mix.tick();
let dry = (1.0 - mix).sqrt();
let wet = mix.sqrt();
```

— the same equal-power law vxn-1b uses, since
`5460922 feat(vxn-2): equal-power FX wet/dry crossfade (delay/reverb)` on
**2026-06-22**, six weeks before this ticket was written on 2026-08-02. The
ticket was drafted from vxn-2's doc comments, which still claimed
`(1-mix)·dry + mix·wet` in two places. Both are corrected in `b9999a1`, each
carrying the commit and date that made it stale.

**Consequences.** There is no mix-law change, no mid-mix dip, no compensation
decision, and vxn-1b's reverb is unchanged at any fixed mix. E041's planned-
ticket line calling 0230 "linear mix law canonical (largest perceptual change)"
should be struck. The only divergence that actually existed was where bypass
lives — the same change 0228 made to the phaser.

### The bug the adoption exposed

`FxChain::reset` promises "silence all tails and snap every slot to fully
bypassed". `WetFade::reset` snapped to whatever the *current* enable flag said
and stayed **primed**, so the next parameter fan-in glided down from the old mix
instead of snapping — audible on the first block after a transport reset.
`fx::tests::reset_snaps_to_bypass` caught it the moment the reverb went
internal. **The phaser has had this since 0228 and the chorus since 0229**; the
reverb was simply the slot that test drives.

`WetFade::reset` now drops to silence, disables and un-primes, so the next
`set` snaps the way a patch load does. That is what a re-idle means — nothing is
playing, the next fan-in is a fresh load — and it is what vxn-2's engine already
does for master gain one line below its own `reverb.reset()` call
([engine.rs:641](../../vxn-2/crates/vxn2-engine/src/engine.rs#L641)). The FX
fades were the inconsistent ones. Pinned by
`declick::tests::reset_unprimes_so_the_next_load_snaps` and
`reset_then_bypass_is_immediately_settled`; the old
`reset_idles_and_rearms_the_edge` encoded the superseded contract and was
rewritten to cover the complement.

**This changes vxn-2's reset path too.** The render hash is unchanged, but the
baseline patch never resets, so the hash is not evidence — the justification is
the alignment above, not a measurement.

### Left alone, now visible

vxn-2's engine calls `phaser.clear()` where it calls `reverb.reset()`, so those
two fades behave differently across an engine reset. A call-site decision, not a
kernel one, so it stays out of this ticket.

### Verification

- **Move is pure for vxn-2, measured back to back**: fingerprint over
  on-from-load, knob move, switch-off + settle, re-enable and off-from-load
  unchanged (`0x52dce89bc65d428a` / `0xb0a3f3488d5ef9e1`); engine render hash
  `0x95ac9a59d27aaddd` before and after.
- **Baselines had to be re-taken.** Another session is committing to this tree;
  its E048 work moved the vxn-2 hash off `0x533a37a7def1921a` and shifted
  `RenderBank::render` 9636→9869, `cook_stacks_block` 245→288,
  `Stack::note_on` 142→128 between captures. Confirmed not mine by re-measuring
  at HEAD with the work stashed — identical both ways. Every claim here is from
  a back-to-back pair, not an older capture.
- Workspace **1414 passed / 0 failed**, 87 suites.
- asm-check green; `FxChain::process_block` 123 → 132 as the reverb's blend
  moved inside.
- Grep: `REVERB` → 0 hits in `fx.rs`; fade arrays 5 slots → 2 across 0228-0230;
  `WetFade` held only by the three migrated kernels and the unused
  `Bypassable<K>`, none of them also in `fades`/`on`.
- Coverage strictly exceeds either copy: every test both had, plus
  `block_matches_per_sample` re-expressed against the shared harness (vxn-1b's
  version tested an out-of-place `process_block` that no longer exists) and a
  `reverb_slot_bypass_fades_and_settles` chain test.

### Closed without a Reaper listen

The ticket asks for sign-off before the re-baseline lands. There is no
re-baseline — no mix-law change, no golden encoding the old voicing — and the
one behavioural difference is the bypass glide already accepted for the phaser
in 0228. Closed on user instruction on that basis.
