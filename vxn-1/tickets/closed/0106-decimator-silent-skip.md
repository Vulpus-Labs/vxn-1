---
id: "0106"
title: Skip decimator on drained silent path
priority: low
created: 2026-06-07
epic: 
---

## Summary

The `Oversampler` decimator runs every block, even when the voice banks
take the silent fast path and write nothing to the oversampled bus.
Once the half-band FIR state has fully drained (after ~tap-length
samples of zero input), continuing to run it is pure compute on zeros
producing zeros.

Track a "decimator drained" flag per channel. Once consecutive
silent-fast-path blocks have ticked the decimator with zero input long
enough to flush its full tap delay, skip the `decimate()` call and
zero-fill the base-rate output buffer directly. Reset the flag the
moment a voice writes non-zero into the OS bus (e.g. on the next
trigger).

After E019 the engine carries two parallel decimators (L + R). Both
need their own drain counter; both fully drain in the same block in
the bit-identity case (`spread = 0` → identical inputs → identical
state evolution).

## Acceptance criteria

- [ ] `Synth` (or wherever the per-channel decimator state lives)
      gains a per-channel "samples since last non-zero input" counter
      and a derived `drained` flag.
- [ ] When all banks take the silent fast path AND the channel's
      counter has reached the half-band total tap length, the
      `decimate()` call is skipped for that channel and the base
      output buffer is zero-filled in its place.
- [ ] Counter resets to zero the first block a bank writes non-zero
      samples into the OS bus.
- [ ] `cargo test --workspace` green — in particular the baseline
      bit-identity test must still pass (the gated render + release
      path naturally drains the decimator across the 0.25 s tail).
- [ ] Bench delta on `render_16_voices/idle_no_voices`: ≥ 1.5× faster
      than the pre-skip number. The drained decimator skip plus the
      symmetric R-channel skip is the whole win.

## Notes

This is a follow-up to E019 — the second decimator added for stereo
routing made the idle-path waste more visible. Reading
`[[vxn1-render-loop-optimized]]` and the E019 bench notes for the
exact pre/post figures.

Drain threshold = sum of tap lengths across active half-band stages
for the current OS factor (`stage_a` at 1×, `+stage_b` at 4×, etc.).
Either read the lengths off `HalfbandFir::taps.len()` or hard-code
the worst case (the biggest stage). The unit is OS-rate samples, so
convert: `blocks_to_drain = ceil(total_taps / (base_block * os))`.

Spread=0 fast-path (scalar voice sum + skip R decimator) is the
sibling ticket 0107 — handles the loud-but-default case. They
compose: at idle + spread=0, both kick in and only the L decimator
runs, draining to zero, then skipping entirely.
