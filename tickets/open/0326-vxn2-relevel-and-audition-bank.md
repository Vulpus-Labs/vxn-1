---
id: "0326"
product: vxn-2
title: "vxn-2: re-level the factory bank and audition key scaling + feedback + velocity together"
priority: high
created: 2026-08-29
epic: E048
depends: ["0325"]
---

## Summary

Closing ticket of [E048](../../epics/open/E048-log-domain-level-pipeline.md).
Three changes alter how every preset sounds — the key-scaling port, the feedback
ladder, and the velocity offset. Audition them **once**, together, rather than
three times.

## Design

Re-sweep `master-volume` across the 45 factory presets with
`cargo run --release -p vxn2-engine --example level_presets -- --apply`, then
listen in a DAW.

Known-affected, worth listening to specifically:

- **Velocity** — every patch with `vel-sens > 0`, worst at high sensitivity.
  Hard-voiced patches get brighter and louder; soft-voiced ones barely move.
  There is no mechanical migration: a curve change is not a gain change.
- **Key scaling** — 33 presets shift ≈ −3.2 dB at C7 (the default
  `ks-r-depth 30`); `Ivory Dust` −15.7 dB and `Electric Boogaloo` −13.4 dB on
  modulators, both from high `ks-r-depth`. Nothing at C4 and below.
- **Feedback** — 42 presets migrated exactly. Three cannot be: `Tin Roof`
  (−4.1 dB), `Sympathetic` (−2.9 dB), `Ash Cloud` (−5.1 dB) sat above the
  hardware maximum, in the region past the sawtooth edge. They will be tamer;
  if the old sound was the point they need re-voicing by other means.

`Electric Boogaloo`'s tine is the acceptance case for 0324 — it is the report
that started this, and its `op2` is the bank's only `kvs 7` operator.

## Acceptance criteria

- [x] `level_presets` re-swept and applied; no preset clamps at the volume
      floor or ceiling. All 45 converge on -6.0 dBFS; a second pass is a no-op.
- [x] The sweep itself fixed first: it rendered at a 512-sample block, 16x
      coarser than the `CONTROL_BLOCK` cadence both host shells actually use, so
      it was setting levels from attack transients no build produces. That alone
      moved measured peaks by up to 6.7 dB (Snapback -6.7, Tin Roof -6.3,
      Rubber Band -5.6), dominating the engine changes it was meant to measure.
- [ ] DAW audition of all 45, with attention to the lists above. **Outstanding —
      this is the ticket's remaining half.**
- [ ] `Electric Boogaloo`'s tine reads right at playing velocity **without** a
      hand boost on `op2`'s output level. Measured +13.6 dB at vel 110 and
      +17.8 dB at vel 127 on the 15th-harmonic sideband, its velocity span
      widening from 24 dB to 43 dB — confirm by ear.
- [ ] Any preset needing re-voicing is either fixed or has a ticket.
- [ ] `ks-r-depth` values reviewed on the two outliers: those were dialled while
      the control was inert, so they may carry no design intent and be worth
      re-choosing now that they bite.

## Notes

Audition is a listening task, not a harness — see the standing practice of
verifying audio by ear in a DAW rather than building headless checks for it.
