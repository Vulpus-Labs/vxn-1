---
id: "0004"
title: Hard sync (coupled oscillator path)
priority: high
created: 2026-05-24
epic: E002
---

## Summary

Add hard sync: osc2 (slave) phase resets each time osc1 (master) completes a
cycle, matching the JP-8 (Sync within VCO-2 locks it to VCO-1). This is the
defining "exciting" oscillator feature and establishes the coupled osc2→osc1
process path that cross-mod (0005) also needs.

## Acceptance criteria

- [x] `vxn-dsp/src/poly.rs`: a coupled process path that, per voice per sample,
      advances osc1 and — on osc1 phase wrap — resets osc2's phase. Computes and
      writes both `o1` and `o2`. Implemented as
      `PolyOscillator::process_pair(slave, sync, xmod, wave1, wave2, …)`
      (master = `self`; 0004 landed it as `process_pair_synced`, generalised to
      `process_pair` when 0005 added the `xmod` term), structured with osc2
      evaluated first so cross-mod can feed osc1's increment.
- [x] The existing independent `osc1.process(); osc2.process()` calls remain the
      **fast path**, used when sync is off and cross-mod depth is 0, so plain
      patches keep the vectorised loop with no perf regression. `voice.rs`
      branches on `ctx.sync` per sample (predictable, loop-invariant).
- [x] New bool param `OscSync`, **appended at the end of the `ParamId` table**;
      `sync: bool` on `BlockCtx`, set in `build_ctx`; `voice.rs` selects coupled
      vs fast path on it (and on cross-mod depth, once 0005 lands).
- [x] v1 sync reset is naïve (phase := 0 on master wrap, mask-selected so the
      lane loop still vectorises) and relies on the existing oversampling for
      alias control. Band-limited (minBLEP) sync correction is out of scope —
      `TODO(0004 follow-up)` doc-comment hook left on `process_pair_synced`.
- [x] Tests: `synced_slave_locks_to_master_period` (DSP) proves the slave output
      is exactly periodic at the master's period; `synced_pair_all_lanes_finite`
      covers mixed waveforms + frozen lanes; engine
      `sync_engages_and_sweeps_formant_finitely` proves the path is live, the
      slave tuning sweeps the formant, and output stays finite.

## Notes

- Direction: VCO-1 master, VCO-2 slave (manual p.16). osc1 owns the wrap test.
- Phase wrap is already detected inside `advance()` (`np >= 1.0`); the coupled
  path needs the wrap *signal*, so compute it explicitly rather than hiding it
  in `advance`.
- Coupling serializes osc1 after osc2 within the lane loop — keep it branchless
  (mask-select the reset) so it still vectorises across voices.
- Oversampling (`ctx.os`, up to 4×) is the v1 alias mitigation; note the
  residual aliasing in the doc-comment for the BLEP follow-up.
- Validation: `cargo test -p vxn-dsp -p vxn-engine`.
