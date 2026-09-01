---
id: "0344"
product: monorepo
title: "`Smoothed` glides never land on targets near 1.0 — stall a ULP short, forever"
priority: low
created: 2026-09-01
---

## Summary

`Smoothed::tick` advances `current += coeff * (target - current)` and snaps only
when the remaining distance is under an absolute `SNAP_EPS = 1e-6`
([smoothing.rs:46](../../crates/vxn-core-utils/src/smoothing.rs#L46)). Near 1.0 a
float step is 1.19e-7, so gliding **up** to a target of 1.0 the increment falls
below half a ULP while the distance is still ~1.4e-5 — `current` stops changing
and the snap test never fires. The smoother sits ~1.4e-5 short of its target for
the rest of the session.

Fading *down* to 0 is unaffected (ULPs get finer approaching zero), which is why
`WetFade::settled_off` has always worked and nothing noticed the other end.

Consequences today, all inaudible (−97 dBFS) but all real:

- An FX toggled on mid-render never reaches the wet mix its patch asks for.
- `WetFade::settled_full` — the licence for `Bypassable::process_block` to hand
  a whole block to its kernel — is only reachable via a snap (patch load, first
  `set`, reset), never by riding a fade up. Pinned as a documented fact in
  `declick::tests::the_bare_smoother_stalls_short_of_a_full_wet_target`.
- Every other `Smoothed` with a target at or near 1.0 — layer fades, bypass
  fades, master gain — is a hair short of it.

## Acceptance criteria

- [ ] `Smoothed::tick` lands on its target from either direction. Preferred
      detection is "the step made no progress" (`next == current`) rather than a
      coarser epsilon — no fudge factor, and it fires exactly when the
      arithmetic has given up.
- [ ] `REBASELINE:` both synths. Measured on 2026-09-01 with the fix applied to
      `WetFade` alone: vxn-2's null test moves to **−79.55 dBFS** (over its −100
      limit) and its hash to `0x3666245aa155a378`; vxn-1b's goldens did not
      move. Fixing `Smoothed` itself will reach more sites than that, so
      re-measure rather than reusing these numbers.
- [ ] A/B notes + Reaper sign-off, per the repo's re-baseline discipline.
- [ ] `WetFade::settled_full`'s doc comment loses its "reached by a snap, not by
      a glide" caveat, and `Bypassable`'s block fast path starts applying after a
      toggle-on as well as after a patch load.

## Notes

Found while building `Bypassable<StereoLimiter>` for
[0232](0232-limiter-chain-rewrites.md), which wanted an exact full-wet weight so
a fully-engaged limiter is bitwise the bare limiter. The fix was written and
reverted there: E041 requires vxn-2's render to be unchanged by its migrations,
and this changes it. It wants to be its own ticket with its own re-baseline,
which is why it is one.

The magnitude is worth keeping in view — −80 dBFS on a full render, from a
1.4e-5 error on a *gain* — so this is a correctness/tidiness fix, not a fix for
anything anyone can hear.
