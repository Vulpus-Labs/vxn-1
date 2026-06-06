---
id: "0010"
title: Audio baseline diff harness vs pre-extraction tag
priority: medium
created: 2026-06-06
epic: E001
---

## Summary

Build the golden-render harness 0006 asked for and run it against the
`pre-vxn-core-extraction` git tag to prove vxn-1 audio is bit- (or
near-bit-) identical to its pre-E001 state. The check is what closes
E001 with confidence.

## Acceptance criteria

- [ ] `vxn-1/xtask audio-baseline render --tag pre-vxn-core-extraction
      --out baseline.f32` — renders a fixed 60s MIDI sequence through
      the engine at the tagged commit (HEAD~N) and writes interleaved
      stereo `f32` to disk. The MIDI sequence exercises every voice /
      LFO / FX path; check it in under `vxn-1/tests/golden/`.
- [ ] `vxn-1/xtask audio-baseline diff baseline.f32` — re-renders
      against the current working tree and prints per-sample RMS +
      max-abs against the baseline. Exits non-zero if RMS > 1e-6.
- [ ] Initial run: baseline rendered + tested against current `main`.
      Expect a clean pass (the partial migration so far is pure-typer;
      no DSP changes).
- [ ] Run again after 0007 + 0008 + 0009 close. Expect RMS < 1e-6.
      If it fails, identify the divergent path: inlining boundary
      change, LFO seed perturbation, RNG drift. Document the path
      and either fix or raise the tolerance to 1e-4 RMS for that
      path.
- [ ] CI step (or local target) added so future refactors keep the
      diff inside tolerance.
- [ ] Golden MIDI + render command documented in
      `vxn-1/tests/golden/README.md`.

## Notes

Pre-tag exists: `pre-vxn-core-extraction`. The render path uses
`vxn-1` vxn-clap's audio process loop via a clack-host harness (or a
direct `Engine::process` call — simpler).

If RNG / free-running LFOs make bit-identity impossible, the per-path
tolerance fallback is documented in 0006 — 1e-4 RMS per offending
path. Don't over-engineer determinism here; the goal is to catch
unintended algorithmic perturbation, not to chase last-bit FP noise.
