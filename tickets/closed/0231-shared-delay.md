---
id: "0231"
product: monorepo
title: "Delay unification — shared vxn-2-superset StereoDelay with optional feedback damping; vxn-1 + vxn-1b adopt"
priority: medium
created: 2026-08-02
epic: E041
depends: ["0227"]
---

## Summary

Fourth ticket of [E041](../../epics/open/E041-shared-fx-unification.md). The
vxn-1/vxn-2 delays differ at feature-set level, not sonic identity
(established during planning): interp (linear vs Catmull-Rom), time smoothing
(40 ms one-pole slew vs 100 ms `Smoothed` glide — same audible continuous
glide), feedback path (one-pole damping param vs fixed 10 Hz DC blocker), mix
law, optional-vs-hardwired ping-pong, sync. Unify on a vxn-2-superset kernel
in `vxn-core-dsp::delay`:

- Base: [vxn2-dsp/src/delay.rs](../../vxn-2/crates/vxn2-dsp/src/delay.rs)
  (Ring + cubic read, DC blocker, sync, pingpong flag, ~100 ms time glide,
  `on`/`mix_primed`).
- Added: optional feedback damping (vxn-1's param), **gated so
  `damping == 0.0` skips the filter entirely** — one-pole with a=0 is not
  float-identity (`lp + (wet-lp)` ≠ `wet`), so the gate is what keeps vxn-2's
  render hash bit-exact.

## Acceptance criteria

- [ ] Move commit: shared kernel, vxn-2 hash unchanged (damping-gate verified
      by the hash itself + a unit test that damping==0 is bit-exact vs the
      pre-move kernel).
- [ ] Adoption commit (vxn-1 + vxn-1b together): pingpong=true, damping param
      mapped, equal-power → linear mix, 40 ms slew → 100 ms glide; outer
      `delay_fade` + DELAY slot fade deleted;
      [vxn-dsp/src/delay.rs](../../vxn-1b/crates/vxn-dsp/src/delay.rs) retired
      (or shimmed until nothing imports it).
- [ ] `REBASELINE:` commit: vxn-1 delay_toggle declick + the delay-time-sweep
      test (its slew-vs-snap comparison re-anchored to the 100 ms glide) +
      baselines; A/B notes; Reaper sign-off.
- [ ] vxn-3's send-bus delay untouched (out of scope; may adopt later with a
      saturating-feedback option).

## Notes

vxn-1's engine currently snaps `DelayTime` because the ramp lives in-kernel
([delay.rs:6-12](../../vxn-1b/crates/vxn-dsp/src/delay.rs#L6-L12)) — that
contract carries over unchanged (glide still lives in-kernel), only the
constant/curve differs. Sync plumbing on vxn-1 is optional scope: wire
`sync=off` initially, sync exposure is a separate feature decision.

## Close-out (2026-09-01)

Four commits: the move (`da61ed0`), vxn-1b's adoption (`a4fefbc`), a perf trim
to `WetFade` (`2005327`), then the goldens (`0f41bca`).

### The move is bit-exact for vxn-2, and the gate is why

[delay.rs](../../crates/vxn-core-dsp/src/delay.rs) is vxn-2's kernel plus
vxn-1b's feedback damping, sited after the DC blocker and **skipped entirely**
at `damping == 0.0`. Baseline hash rendered at the commit before the move and at
the tip: both `0x95ac9a59d27aaddd`. That comparison — not a unit test — is the
proof against the pre-move kernel; the two tests here cover the premise
(`delay::tests::a_flat_one_pole_is_not_float_identity`) and the gate's own
invariant, that a damping-0 run leaves the poles untouched
(`damping_zero_is_bit_exact_against_an_undamped_run`).

The checked-in vxn-2 `EXPECTED` does not match on this machine, before or after.
Pre-existing: that constant is a CI artefact by its own header, which is why the
dev-side bar is the null test.

### `RisingClear` had never been consumed, and was broken

This is the first kernel to honour the edge, which exposed a `WetFade` bug: the
active/inactive latch was written from the **pre**-tick state, so the last tick
before a fade landed recorded "active". Every owner gates on `is_active` and
then stops ticking, so the latch stayed stale for the whole idle stretch and the
next re-enable reported nothing. Fixed in `da61ed0`, then trimmed to derive the
latch from the ticked weight in `2005327`. Covered by
`declick::tests::an_owner_that_stops_ticking_when_the_fade_lands_still_gets_the_edge`
and, at the kernel, `delay::tests::re_enabling_does_not_dump_the_stale_tail`.

Phaser, chorus and reverb ignore the edge, so nothing shipped was affected.

### Two of the ticket's premises did not survive

- **"equal-power → linear mix" is wrong**, the same way 0230's mix-law framing
  was: both kernels already crossfaded `√(1-mix)·dry + √mix·wet`. No mix-law
  change happened, and none was needed. The claim came from vxn-2's
  `StereoDelayParams` doc comment, which said "Linear" while the code did not;
  corrected in the move.
- **The feedback cap change is unreachable.** 0.99 → 0.95 sounds like a real
  reduction, but `delay_feedback`'s param range already tops out at 0.95
  ([params.rs:759](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L759)) and no
  matrix destination writes it. Nothing in the plugin can ask for the range that
  moved.

### vxn-1b's adoption

[vxn-dsp/src/delay.rs](../../vxn-1b/crates/vxn-dsp/src/delay.rs) is a 36-line
re-export; the kernel and `DelayLine` (which nothing outside it used) are gone.
`FxChain` maps the params — including `sync = false`, because 0267 already
resolves the synced time upstream — and the DELAY slot leaves `fades`/`on`,
which are down to `N_SLOTS = 1`. `DELAY_MAX_SECONDS` is now the shared kernel's
own `MAX_DELAY_S`, so 0267's 4 s line is one constant instead of two.
`toggling_off_settles_back_to_bit_exact_skip` moved to dynamics (the last outer
fade); the delay joins `assert_internal_fade_slot`, which allows for the longer
in-kernel settle. The click-free `DelayTime` sweep test came across to
`delay::tests::delay_time_sweep_is_click_free`, re-anchored from the 40 ms slew
to the 100 ms glide.

`grep BypassXfade` across all four products now returns exactly one hit: a
sentence in `declick.rs` explaining what the idiom replaced.

### Goldens and A/B

Both vxn-1b baselines re-captured in `0f41bca` (`reference_render.f32`, and
`EXPECTED` `0xef1c866fd4a38540` → `0x5d7f71bfc17fb2f2` — the old value still
verified on this machine one commit earlier, so the migration is the whole
difference). Null peak against the old reference: **−2.19 dBFS**. A dedicated
before/after delay clip put numbers on what moved: PingPong **off** is identical
to 0.1 dB in tail RMS (the audible difference is the cubic tap and the DC
blocker), PingPong **on** swaps the sides, which is the input crossfeed vxn-1b
did not have. Full notes in the commit message. Listened in Reaper before the
goldens landed.

### Cost

vxn-2's delay bench: steady `2.366 → 2.458 µs` per 256 stereo samples
(**+3.9%**), bypassed unchanged at ~984 ns. Stubbing the damping branch out
attributes 0.7 % to the gate; the rest is the `WetFade` shape — pre-tick gate,
edge, latch — which is the standing price of one declick idiom over five
hand-rolled ones. Not chased further: making it free needs post-tick gating,
which would stop the delay writing its line on the sample a fade lands and cost
the bit-exactness this ticket rests on.

### Untouched

vxn-3's send-bus delay, per the ticket: `git diff` over `vxn-3/` across all four
commits is empty.
