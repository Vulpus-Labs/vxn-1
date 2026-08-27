---
id: "0225"
product: monorepo
title: "BypassXfade + raised_cosine_rise → vxn-core-utils (absorbs ticket 0195)"
priority: medium
created: 2026-08-02
epic: E040
depends: ["0222"]
---

## Summary

Fourth ticket of [E040](../../epics/open/E040-vxn-core-dsp-foundations.md);
executes and absorbs open ticket
[0195](0195-shared-declick-core-utils.md) unchanged. Move
`raised_cosine_rise` + `BypassXfade` from
[vxn-engine/src/smoothing.rs:47-150](../../vxn-1/crates/vxn-engine/src/smoothing.rs#L47-L150)
to `vxn-core-utils::smoothing`; point vxn-2's two inline raised-cosine sites
in [engine.rs](../../vxn-2/crates/vxn2-engine/src/engine.rs) at the shared
helper; drop vxn-1's duplicate `ms_to_samples`.

After E041, `BypassXfade` is no longer used for per-FX enables (WetFade
replaces it) — it remains the primitive for **whole-span** switches: vxn-1's
oversample-change crossfade (`OutputStage`) and vxn-2's span fades.

## Acceptance criteria

- [ ] `BypassXfade` + `raised_cosine_rise` in `vxn-core-utils::smoothing`;
      vxn-engine imports them; single `ms_to_samples`.
- [ ] vxn-2's inline `0.5 - 0.5*cos(π·t)` sites call the shared fn — copy the
      expression verbatim; vxn-2 render hash must not drift (eval-order risk
      flagged in 0195).
- [ ] vxn-1 [tests/declick.rs](../../vxn-1/crates/vxn-engine/tests/declick.rs)
      byte-identical — verify, don't recapture.
- [ ] Ticket 0195 closed as absorbed (close-out points here).

## Notes

Pure move. E041 later repurposes this primitive; keep its API unchanged here.

## Close-out (2026-08-27)

- `BypassXfade` + `raised_cosine_rise` now live in
  [vxn-core-utils::smoothing](../../crates/vxn-core-utils/src/smoothing.rs),
  both `pub`. `vxn-dsp` re-exports them alongside `Smoothed` / `one_pole_coeff`
  ([vxn-dsp/src/lib.rs:74](../../vxn-1/crates/vxn-dsp/src/lib.rs#L74)), so
  vxn-engine's import style is unchanged. 101 lines deleted from
  [vxn-engine/src/smoothing.rs](../../vxn-1/crates/vxn-engine/src/smoothing.rs).
  API unchanged, as the ticket asked — E041 repurposes it later.
- `BypassXfade`'s doc now carries ADR 0002 §5's scoping: it is the **whole-span**
  primitive (vxn-1's oversample-change fade, vxn-2's span fades), not the per-FX
  enable mechanism that `WetFade` takes over in E041.

### The `ms_to_samples` criterion could not be met as written

"Drop vxn-1's duplicate `ms_to_samples`" assumed the two were duplicates. **They
are not.**

| | expression | floor | callers |
|---|---|---|---|
| `vxn_core_utils::ms_to_samples` | `(ms*0.001*sr).max(0.0) as usize` — truncates | 0 | limiter lookahead, vxn-1b OS fade |
| vxn-1 engine's private one | `(ms*0.001*sr).round().max(1.0) as usize` — rounds | 1 | vxn-1's 5 FX fades + OS fade |

They agree at 48/88.2/96/176.4/192 kHz but **differ at 44.1 kHz**: a 5 ms window
is 220 truncating, 221 rounding. Dropping vxn-1's would have shortened its
oversample-change fade by a sample at 44.1 kHz on a shipped product — silently,
since the goldens run at 48 kHz where the two agree.

The contracts are genuinely different, and neither caller can take the other's:
a limiter lookahead of 0 is meaningful, a fade window of 0 is degenerate. Merging
them is precisely the "signal-model compromise" [ADR 0002](../../adrs/0002-vxn-core-dsp.md)
§2's boundary test forbids. So: `ms_to_samples` is unchanged, and a second
`fade_len_samples` (round, floor 1) carries the fade contract. Both are
documented with a pointer to the other and the 44.1 kHz case. vxn-1's 7 call
sites moved to `fade_len_samples`, keeping its behaviour bit-identical.
`fade_len_and_ms_to_samples_differ_at_44k1` pins the distinction so a future
tidy-up cannot quietly re-merge them.

### There were four copies of `raised_cosine_rise`, not three

0195 counted vxn-1's and vxn-2's two inline sites. vxn-1b had a fourth, in
[output.rs](../../vxn-1b/crates/vxn1b-engine/src/output.rs) — filed before that
port existed. All four were the byte-identical expression
`0.5 - 0.5 * (core::f32::consts::PI * t).cos()`, so adopting the shared fn is
bit-exact by construction and 0195's eval-order worry does not arise. Grep for
that expression across the tree now returns only `vxn-core-utils`.

### Verification

- **vxn-2 render hash proven unchanged**, not assumed. The recorded constant
  does not match on this machine (it is pinned to CI's macos-15; this is
  macOS 14), so instead of skipping, the test was run with `VXN_RENDER_HASH=1`
  against a clean worktree at HEAD and against the change. Both produce
  **`0x533a37a7def1921a`** — identical actual hashes, identical mismatch against
  the CI constant. The pre-existing mismatch is environmental; the change moves
  nothing. CI will still enforce the absolute value.
- vxn-1 `tests/declick.rs`: all 7 pass unmodified, no re-capture
  (`all_fx_off_is_bit_exact_across_fx_params`, the five toggle tests,
  `oversampling_change_is_declicked`).
- vxn-1 `baseline_render_is_stable` passes.
- New tests on the shared surface: the 44.1 kHz divergence, `fade_len_samples`
  never degenerate, `raised_cosine_rise` endpoints/midpoint, and two `BypassXfade`
  behaviour tests (equal-gain across the window + lands exactly on target; no-edge
  no-fade + reset idles).
- [0195](../closed/0195-shared-declick-core-utils.md) closed as absorbed, with
  its three Notes carried forward as obligations on this ticket — all three
  discharged above.

### Flagged, not fixed

vxn-1's OS fade is 221 samples at 44.1 kHz and vxn-1b's is 220, from the same
`OS_FADE_MS = 5.0`, because vxn-1b's `output.rs` uses the truncating
`ms_to_samples`. That drift predates this ticket. Aligning it would change
vxn-1b's audio on a shipped product and belongs in a REBASELINE commit with its
own justification — out of scope for a pure move. Left as-is and noted here.
