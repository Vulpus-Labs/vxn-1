---
id: "0273"
product: vxn-1b
title: "One statement per routing rule — retire render.rs's shadow implementation"
priority: medium
created: 2026-08-21
epic: null
depends: []
---

## Summary

[render.rs](../../vxn-1b/crates/vxn1b-engine/src/render.rs) presents itself as
*the* matrix→render apply layer: "this module maps those totals onto the same
DSP consumption points, so the forked render loop is otherwise VXN1's,
byte-for-byte". It is not. Smoothing (0208) sits between the dest total and the
consumption point, so `bank::render` re-implements the mapping inline — and six
of the module's nine public functions are now reachable only from their own
tests:

| Function | Reached from |
|---|---|
| `voice_cutoff_hz`, `voice_resonance`, `pwm_offset` | `bank::render` |
| `voice_pitches`, `voice_pw1`, `voice_pw2`, `voice_hpf_hz`, `voice_cross_mod_amount`, `voice_amp` | **tests only** |

The hazard is not the dead code, it is that the live code *cites* the dead code
as authority:

- [bank.rs:597](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L597) — "`g1`/`g2`
  gate XModSweep onto the mode-selected osc, exactly as `render::voice_pitches`
  does" — while [`sweep_gates`](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L1173)
  is a second, independent copy of that match.
- [bank.rs:727](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L727) —
  "`render::voice_cross_mod_amount` is the statement of that rule" — while the
  bank writes `.max(0.0)` inline, in two places.
- The `(0.05, 0.95)` pulse-width clamp exists four times: `voice_pw1`,
  `voice_pw2`, and twice inline in the bank (block-start peek and per-quantum
  tick).

Change the sweep gating or the PW clamp in `render.rs` and you ship nothing, and
every test still passes.

Two smaller duplications belong with this:

- [`bank::render_shape`](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L1141) is
  byte-identical to `eval::shape`, with a comment admitting it ("mirror of
  `crate::eval`'s `shape` (kept private there)").
- `eval::idx`, `eval::di` and `render::dest` are three copies of "unwrap the
  `Option<usize>` sentinel", behind 37 `.idx().unwrap()` call sites.

## Design

- Delete `voice_pitches`, `voice_pw1`, `voice_pw2`, `voice_cross_mod_amount`,
  `voice_amp` and their tests. Keep the *rules* they encoded by moving each
  assertion onto the live implementation: the sweep-gating test moves to
  `bank::sweep_gates`, the clamp/`max` tests to the small helpers below.
- `voice_hpf_hz` is exempt: [0272](0272-vxn1b-hpf-cutoff-dest-inert.md) makes it
  live. Land 0272 first or leave the function alone here.
- Retitle the `render.rs` module doc to what it actually is — three pure dest
  consumers (`voice_cutoff_hz`, `voice_resonance`, `pwm_offset`), plus
  `voice_hpf_hz` after 0272 — and delete the "byte-for-byte" claim.
- Extract the two clamps the bank repeats:
  `fn cooked_pw(base: f32, offset: f32) -> f32` and
  `fn cooked_pm_index(base: f32, offset: f32) -> f32`. Both are one line; the
  point is that the block-start peek and the per-quantum tick stop being
  copy-paste of each other.
- Make `eval::shape` `pub(crate)` and delete `bank::render_shape`.
- Add `DestId::i(self) -> usize` / `SourceId::i(self) -> usize` (sentinel → 0,
  documented as unreachable for real ids) and replace `eval::idx`, `eval::di`,
  `render::dest` and the `.idx().unwrap()` sites with it. `idx() -> Option` stays
  for the places that genuinely branch on the sentinel (`eval_dests`,
  `amp_coeffs`).

## Acceptance criteria

- [x] No function in `render.rs` is reachable only from `#[cfg(test)]` code.
- [x] `sweep_gates` is the only expression of the Off/Ring→both, Sync→osc1,
      PM→osc2 rule in the crate, and carries the test that moved off
      `voice_pitches`.
- [x] The `(0.05, 0.95)` PW clamp and the cross-mod `.max(0.0)` each appear once.
- [x] `eval::shape` has one definition.
- [x] `.idx().unwrap()` appears nowhere outside tests.
- [x] Full `cargo test -p vxn1b-engine` green, and a default-patch render is
      bit-identical before and after (no behaviour change in this ticket).

## Notes

Pure refactor — no DSP output change is intended or permitted. The bit-identity
check is the real acceptance gate; the rest is structure.

Do this before [0276](0276-vxn1b-bank-render-decomposition.md): decomposing
`bank::render` is much easier once the dead parallel implementation is gone and
the clamps are named.

## Close-out

Landed 2026-08-21. Files touched: `vxn1b-engine/src/{render.rs, bank.rs,
eval.rs, matrix.rs, params.rs}`.

`render.rs` went from 9 public functions (6 test-only) to 4, all live, and from
~350 to 229 lines. Deleted: `voice_pitches`, `voice_pw1`, `voice_pw2`,
`voice_cross_mod_amount`, `voice_amp`. `voice_hpf_hz` was kept and made live by
[0272](../closed/0272-vxn1b-hpf-cutoff-dest-inert.md). The module doc no longer
claims to be the apply layer; it states which dests it owns, which the bank owns,
and why (anything the bank must smooth is inseparable from the render loop).

Every deleted function's assertions moved onto the live code rather than being
dropped — new bank tests `sweep_is_mode_gated_and_pitch_is_not`,
`cooked_pw_clamps_each_oscillator_independently`,
`cooked_pm_index_is_additive_and_non_negative`, `vca_reproduces_vxn1_amp_base`,
`amp_folding_uses_the_evaluators_curve`.

New single statements:
- `bank::cooked_pw` / `bank::cooked_pm_index` replace four and two inline copies.
- `eval::shape` is `pub(crate)`; `bank::render_shape` is gone.
- `SourceId::index()` / `DestId::index()` replaced `eval::idx`, `eval::di`,
  `render::dest` and all 39 `.idx().unwrap()` sites. `idx() -> Option` stays for
  the places that genuinely branch on the sentinel.

Beyond the ticket: `PW_MIN`/`PW_MAX` moved to `params.rs` and now bound both the
two PW params *and* the modulated width, so a route cannot reach a duty cycle
the knob refuses.

All acceptance criteria met. Bit-identity confirmed on four patches (default,
all continuous dests live, the note-on-latched dests, cross-mod + HPF).
