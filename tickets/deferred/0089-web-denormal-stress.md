---
id: "0089"
product: vxn-2
title: "Denormal stress (held-quiet-sustain → reverb feedback)"
priority: medium
created: 2026-06-22
epic: E020
depends: ["0087"]
---

## Summary

Third ticket of [E020](../../epics/open/E020-web-perf-crossbrowser-ship.md). WASM
has no FTZ/DAZ — denormals are not flushed for free as they are with native
NEON. The 0034 spike's clean result was the *release* path (notes fully decayed
to exact silence, which the engine's silent-skip handles
— `vxn1-silent-skip-filter-state`). The untested case is a **held quiet
sustain** feeding **reverb feedback**: a low-amplitude tail that never reaches
exact zero can drive filter/reverb state into the denormal range and cause a CPU
cliff. This ticket stresses that case and adds a **targeted manual flush ONLY if
a measurable cliff appears**.

## Design

- **Stress patch.** Extend the 0087 bench
  ([bench.rs](../../vxn-1/crates/vxn-wasm/src/bench.rs)) into a denormal variant:
  hold a quiet sustain (low velocity / low sustain level so the amp envelope sits
  at a small non-zero value) into a high-decay reverb so the FDN feedback path
  recirculates a tiny signal indefinitely. Crucially this must NOT hit the
  exact-silence fast path
  ([lib.rs:514 `both_silent`](../../vxn-1/crates/vxn-engine/src/lib.rs#L514)) —
  the whole point is a perpetually-tiny non-zero signal.
- **Detection.** Reuse the 0087 per-quantum timing harness: render thousands of
  quanta of the held-quiet tail and watch for a sustained jump in
  render-ms-per-quantum vs the loud worst case. A denormal cliff shows as the
  *quiet* tail rendering *slower* than the loud chord — the diagnostic signature.
- **Flush, only if needed.** If a cliff is measured, add a targeted flush at the
  recirculating boundaries (reverb/delay feedback taps, filter state) — e.g. add
  a tiny DC offset / flush-to-zero when |x| < ~1e-30, in `vxn-dsp`/`vxn-engine`
  FX state. This is the *only* sanctioned engine change in E020 (epic Scope
  "Out"), and only if measurement demands it. If no cliff appears, the close-out
  records "verified safe, no flush needed".

## Acceptance criteria

- [ ] (headless) `cargo test -p vxn-wasm` includes a bench test that builds the
      held-quiet denormal patch and asserts the post-note-off tail is non-silent
      (stays on the hot path, does NOT collapse to exact zero).
- [ ] (MANUAL, M1 Chrome) Run the 0087 timing harness on the held-quiet tail for
      ≥10 000 quanta; compare render-ms-per-quantum against the loud 16-voice
      worst case. Record whether the quiet tail is slower (denormal cliff) or
      comparable (safe).
- [ ] (conditional) If a cliff is measured: land a targeted flush and re-measure;
      the quiet-tail cost must drop to within the loud-case envelope. If no
      cliff: document "no flush needed" and add no engine change.

## Notes

- Depends on 0087 for the timing harness and the worst-case patch scaffold.
- Memory: `vxn1-silent-skip-filter-state` (the release/exact-silence case is
  already covered; this is the *non-silent* held-quiet case it does not cover).
- Out of scope: any denormal work beyond the one feedback-path flush; new DSP
  features.
