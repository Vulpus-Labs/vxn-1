---
id: "0229"
product: monorepo
title: "Shared StereoChorus, true-stereo only — mono-sum process deleted"
priority: medium
created: 2026-08-02
epic: E041
depends: ["0227"]
---

## Summary

Second ticket of [E041](../../epics/open/E041-shared-fx-unification.md).
vxn-1's `StereoChorus` has two non-equivalent entry points:
`process_block_stereo`
([chorus.rs:150](../../vxn-1b/crates/vxn-dsp/src/chorus.rs#L150), true stereo —
what vxn-1's engine uses) and per-sample `process`
([chorus.rs:201](../../vxn-1b/crates/vxn-dsp/src/chorus.rs#L201), **mono-sums
the input** — what vxn-1b's FxChain uses). A naive per-sample trait with a
blanket block impl would silently change vxn-1's sound. Resolution: the shared
kernel keeps only the true-stereo path; `FxKernel::process` becomes a
length-1 call into the stereo body; the mono-sum variant is deleted.

## Acceptance criteria

- [ ] `StereoChorus` in `vxn-core-dsp::chorus` implementing `FxKernel` with
      WetFade; block override tested sample-identical to per-sample form.
- [ ] vxn-1 adoption: block-stereo body moved verbatim — target **no**
      re-baseline (verify baseline + declick byte-identical; if the WetFade
      swap from `chorus_fade` shifts the toggle envelope, that lands as the
      flagged part).
- [ ] vxn-1b adoption (same commit as vxn-1): mono-sum → true-stereo is
      audible → `REBASELINE:` vxn-1b goldens with A/B notes.
- [ ] Outer fades deleted both sides; grep check.

## Notes

Chorus is vxn-1-lineage (vxn-2 has none) — the shared kernel makes it
importable to vxn-2/vxn-3 later by constructing its Params.

## Close-out (2026-08-29)

Four commits: two pure moves clearing the way (`48b82d6`, `69decf2`), then the
kernel and vxn-1b's adoption together (`f3d2e46`).

### The dependency chain the ticket does not mention

The chorus is built on `ModDelayLine`, which is built on `BoundedRandomWalk`,
neither of which was shared. A component crate cannot reach up into `vxn-dsp`,
so both moved first, each as its own verifiable commit:

- **`BoundedRandomWalk` → `vxn-core-utils`** (a leaf by ADR 0002's test). The
  risk here was not the chorus but `poly::oscillator`, which uses the same walk
  from inside a lane loop that must stay vectorised and which stays per-synth.
  `advance` keeps `#[inline(always)]` so it still crosses the boundary;
  asm-check confirms `PolyOscillator::process` held at 292 and
  `RenderBank::render` at 9636.
- **`ModDelayLine` → `vxn-core-dsp::delay_line`**, following its only consumer.
  Fingerprints over the line with jitter engaged and over both chorus entry
  points across a param move were unchanged (`0x4b456985f07c1f39` /
  `0x1c6d7e165d39ac28` / `0x56facd66acae73bc`).

Plain `xorshift64` also moved to `vxn-core-utils::math`, beside the star sibling
0228 put there. The alternative was letting the chorus adopt the star stream,
which would have changed the BBD hiss as well — and this is a ticket judged by
ear, so it should carry exactly one audible change.

### The mono sum, measured

The old body kept the **dry** leg in true stereo and derived only the **wet**
from the sum, so the pre-0229 output is recoverable exactly from the new kernel
as `out_mono + dry_g*(in - mono)` — no need to keep the deleted code around to
compare against. On a detuned stereo pair (220/223 Hz), rate 1.0, depth 0.7:

| mix | side | mid |
|---|---|---|
| 0.25 | +0.9 dB | +0.6 dB |
| 0.5 | +1.6 dB | +1.3 dB |
| 1.0 | +2.5 dB | +3.3 dB |

Mostly a **level rise at high mix**, not the widening the ticket's framing
implies: summing detuned material to mono partially cancelled it before it
reached the delay lines. A first attempt at this measurement fed the kernel
`(mono, mono)` and reported +8.3 dB of side — wrong, because that mono-ed the
dry leg too, which the old code never did.

### Deviation: `process` is a real per-sample body

The ticket specifies `FxKernel::process` become "a length-1 call into the stereo
body". Not done, deliberately: that body allocates `CONTROL_BLOCK` scratch and
runs three passes, and vxn-1b's serial chain calls per sample, so a length-1
call would be pure overhead on the audio thread. `process` is a true-stereo
per-sample body — same behaviour, none of the cost — and the block path keeps
its three-pass shape for cache locality.

The two are pinned together by `chorus::tests::block_override_matches_the_sample_path`
(via `assert_block_matches_sample`) and by
`chorus::tests::block_and_sample_agree_across_a_switch_off`, which covers what
that split makes easy to get wrong: a fade landing partway through a block,
where the per-sample path stops ticking and the block path has to stop at the
same sample or the two silently diverge.

### Two latent problems fixed in passing

- The block body indexed fixed `[f32; CONTROL_BLOCK]` scratch with the caller's
  length and would have panicked on a longer block. Nothing called it that way,
  but `FxKernel::process_block` takes arbitrary slices, so it chunks now.
- `set_hiss` / `set_jitter` were setters no shipping synth ever called. They are
  `ChorusParams` fields, which is what makes the kernel constructible from
  another synth's mapping — the epic's actual acceptance test.

### Grep check

`grep CHORUS fx.rs` → 0 hits; fade arrays 5 slots → 3 across 0228+0229. The
mono sum greps to nothing repo-wide. `WetFade` is held only by the two migrated
kernels and the unused `Bypassable<K>`; neither migrated slot appears in
`fades`/`on`.

### No REBASELINE commit, and no Reaper listen recorded

Same as 0228: vxn-1b has no render golden, and no committed expectation encodes
the chorus voicing, so there was nothing to re-baseline. Coverage replaced it —
the phaser and chorus slot tests collapse into one
`assert_internal_fade_slot` helper, which 0230–0232 inherit.

The ticket asks for a user listen before this lands. **It closed on user
instruction without one**; the A/B figures above are the record instead. If the
chorus reads too hot at high mix in the DAW, the fix is a compensating trim on
the wet leg, which is a voicing decision rather than a defect in this move.

### Verification

- Workspace **1402 passed / 0 failed**, 86 suites.
- asm-check green on all nine paths. `FxChain::process_block` 127 → 123 (floor
  80); the chorus still inlines into it — no new symbol — and
  `ModDelayLine::process` stays a separate symbol at 47, as it was before.
