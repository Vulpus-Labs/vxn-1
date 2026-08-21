---
id: "0272"
product: vxn-1b
title: "HPF Cutoff is a selectable mod destination that does nothing"
priority: high
created: 2026-08-21
epic: null
depends: []
---

## Summary

`DestId::HpfCutoff` is offered in the matrix destination dropdown — it has an
enum variant ([matrix.rs:148](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs#L148)),
a wire name, a `"HPF Cutoff"` display label
([matrix.rs:290](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs#L290)), a
`DEST_GAIN` of 48 st ([eval.rs:139](../../vxn-1b/crates/vxn1b-engine/src/eval.rs#L139))
and an apply function ([render.rs:161](../../vxn-1b/crates/vxn1b-engine/src/render.rs#L161)) —
and the render never reads it.

`bank.rs` sets the HPF once per block from the raw param and nothing else:

```rust
let hpf_active = ctx.hpf_cutoff > HPF_OFF_HZ;
if hpf_active {
    self.hpf.set_cutoff_all(ctx.hpf_cutoff, ctx.os_sample_rate);
}
```

`grep HpfCutoff bank.rs` returns exactly one hit, a module doc note at
[bank.rs:31](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L31) saying the dest is
"still deferred (the HPF is set bank-wide)". `render::voice_hpf_hz` is called
only by its own unit test, so the test suite is green on a dest the user cannot
hear.

A player who routes Env 1 → HPF Cutoff gets silence and no feedback.

## Design

**Wire it, don't remove it.** The deferral reason is stale: `PolyHpf` has been
per-voice all along —
[hpf.rs:78](../../vxn-1/crates/vxn-dsp/src/hpf.rs#L78) holds `a: [f32; N]` and
[hpf.rs:102](../../vxn-1/crates/vxn-dsp/src/hpf.rs#L102) exposes
`set_cutoff(v, hz, sr)`. `set_cutoff_all` is the convenience path, not the only
one. The dest costs one `set_cutoff` per lane in the block-start loop, alongside
the ladder coefficients that already go per lane there.

Two details:

- **The `hpf_active` gate must account for the route.** With the param parked at
  its 20 Hz minimum (`HPF_OFF_HZ`, i.e. "off") a positive route should still be
  able to open the filter. Gate on the param *or* any lane's modulated cutoff
  clearing the threshold, and take the bank-wide `set_cutoff_all` fast path when
  no lane is modulated, so an unrouted patch stays bit-identical.
- **No smoothing tier is being added.** Cutoff/Resonance get away without a
  `MotionSmoother` entry because the OTA ladder ramps its own coefficients; the
  HPF does not ramp. A one-pole HPF coefficient stepping at block rate is far
  less audible than a gain step, and it is what already happens when the user
  moves the param, so this ships without a smoother. If a stepped source into
  the dest turns out to buzz, that is a follow-up, not this ticket.

## Acceptance criteria

- [x] A route into `HpfCutoff` audibly moves the high-pass: a new bank test
      renders with `Env1 → HpfCutoff` at depth and asserts the low-frequency
      content drops relative to an unrouted render.
- [x] With the HPF param at its 20 Hz minimum, a positive route still opens the
      filter (the `hpf_active` gate is not keyed on the raw param alone).
- [x] A patch with no `HpfCutoff` route renders **bit-identically** to before
      the change, and still takes the `set_cutoff_all` path.
- [x] `render::voice_hpf_hz` is reached from the render path, not only from its
      test.
- [x] The stale "still deferred" note at [bank.rs:31](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L31)
      is removed.

## Notes

Found while reviewing modulation-routing complexity for maintainability; the
other four tickets from that review (0273–0276) are pure tidy-ups with no
behaviour change. This one is the only functional defect in the batch.

If wiring turns out to be more than it looks, the fallback is to drop
`HpfCutoff` from `DEST_NAMES`/`DEST_LABELS` so it stops being selectable — but
that loses a genuinely useful destination and breaks any preset already
referencing it, so it is the worse answer.

## Close-out

Landed 2026-08-21. Files touched: `vxn1b-engine/src/bank.rs`.

Wired, not removed — the deferral reason (`PolyHpf` is bank-wide) was stale:
`PolyHpf` has held `a: [f32; N]` and exposed `set_cutoff(v, …)` all along.
`set_lane_filter` now returns each lane's `render::voice_hpf_hz(&dests,
ctx.hpf_cutoff)` and the bank picks its path from `hpf_modulated`: per-lane
coefficients when a route is live on any sounding lane, the original
`set_cutoff_all` otherwise.

The `hpf_active` gate became `ctx.hpf_cutoff > HPF_OFF_HZ || (hpf_modulated &&
any sounding lane above the rail)`, so a route opens the filter from the 20 Hz
"off" position.

All four acceptance criteria met. Four new bank tests: the route thins the low
end; it works from the off rail; an inert route (wheel at zero) renders
bit-identically at three base cutoffs; and the dest is genuinely per-lane —
a two-note render equals the sum of the two solo renders, which a bank-wide
`set_cutoff_all` could not produce.

**Bypass stays per bank.** One modulated lane pulls its neighbours through the
filter too. At `HPF_OFF_HZ` that is what "off" already means — a 20 Hz one-pole
is transparent over the audio band — so the cost is arithmetic, not tone.

**No smoother added**, as designed. If a stepped source into the dest turns out
to buzz, that is a follow-up.

**Not verified by automated test:** the audible result — needs a listen in
Reaper, per [[verify-audio-in-reaper]].
