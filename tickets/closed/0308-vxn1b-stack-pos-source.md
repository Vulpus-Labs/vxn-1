---
id: "0308"
product: vxn-1b
title: "Mod source: Stack Pos — lane position independent of the Spread knob"
priority: medium
created: 2026-08-26
epic: E039
---

## Summary

`SourceId::Spread` carries the lane's place in its stack **already multiplied by
the front-panel `Spread` param** (0260) — that scaling lives inside the source so
`Spread → Pan @ 1.0` reproduces VXN1's hard-wired unison spread with the knob
still a knob. The cost: every *other* use of lane position is hostage to a pan
control. Routing `Spread → Env1Scale` to fan envelope lengths across a unison
stack reads as dead at the default `spread = 0.0`, and the moment you raise the
knob to wake it up you also fan the stereo image.

Add a second source, **`StackPos`**, that emits the raw allocator position
(`stack_spread(i, width)`, `[-1, 1]`, `0.0` at width 1) with no param scaling.
`Spread` stays exactly as it is — the default patch, VXN1 parity and every saved
preset are untouched.

## Acceptance criteria

- [ ] `SourceId::StackPos = 12`, `N_SOURCES = 12`; `from_u8`, `is_bipolar`
      (bipolar — it is a position), `SOURCE_NAMES` (`"stack-pos"`) and
      `SOURCE_LABELS` (`"Stack Pos"`) all extended.
- [ ] `SourceInputs::stack_pos` filled from the allocator's unscaled position in
      `RenderBank::lane_sources`; `eval_sources` stores it.
- [ ] Appended after `Spread`, so existing state blobs and preset TOMLs decode
      unchanged; the UI's source dropdown picks it up from the engine vocab with
      no JS edit.
- [ ] Test: with `StackPos → Env1Scale @ 1.0`, a 4-wide stack cooks env scales
      `[0.5, .., 2.0]` at `spread = 0.0` — i.e. the knob is out of the loop.
- [ ] Test: `Spread`'s own behaviour is unchanged (still zero at `spread = 0`).
- [ ] `PARAMETERS.md` regenerated.

## Notes

Found via a user report that `Spread → Env 1 Scale` "doesn't vary envelope phase
lengths across the stack". It does — but only with the Spread knob up *and* a
stack wider than one. Verified in the engine: at `spread = 1.0`, a 4-wide stack
cooks `env_scale = [0.5, 0.794, 1.26, 2.0]`; at `spread = 0.0`, all `1.0`.

Envelope scales latch at note-on (`cook_env_mods`, 0268), so this source — like
`Spread` — is sampled at the trigger for the time/sustain dests and per-block
for everything else. That is by design.

## Close-out (2026-08-26)

- `SourceId::StackPos = 12` with `N_SOURCES = 12`
  ([matrix.rs:66](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs#L66)); `from_u8`
  decodes `12`, `is_bipolar` returns true (it is a position, like `Spread`), and
  both vocab tables gained `"stack-pos"` / `"Stack Pos"` in last place. The
  existing `SOURCE_NAMES.len() == N_SOURCES + 1` assertions and the
  `idx() == N_SOURCES - 1` last-source check were retargeted to `StackPos`.
- `SourceInputs::stack_pos` added and stored by `eval_sources`
  ([eval.rs:96](../../vxn-1b/crates/vxn1b-engine/src/eval.rs#L96)), filled from the
  allocator's unscaled lane position in `RenderBank::lane_sources`
  ([bank.rs:719](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L719)) — one line
  beside `spread_pos: stack_pos[v] * ctx.spread`, which is untouched.
- Appended after `Spread`, so no existing wire value moved: state blobs and
  preset TOMLs decode as before (the `preset` round-trip tests pass unchanged),
  and the faceplate's source dropdown picks the new entry up from the engine
  vocab (`window.vxn.matrix.sources`, built in
  [ui-web/lib.rs:322](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L322) and consumed
  by [matrix.js:192](../../vxn-1b/crates/vxn1b-ui-web/assets/panels/matrix.js#L192)) —
  no JS edit in the diff.
- Tests, all in `bank::tests`:
  `stack_pos_fans_env_times_with_the_spread_knob_at_zero` (4-wide stack,
  `StackPos → Env1Scale @ 1.0`, `spread = 0.0` → `[0.5, 2^-⅓, 2^⅓, 2.0]`),
  `stack_pos_is_inert_for_a_width_one_stack`, and
  `spread_source_still_needs_the_knob` (pins `Spread`'s knob coupling — flat at
  `spread = 0`, fanned at `1.0` — so the two sources stay honestly different).
  New helpers `stack_env_scales` / `one_route` alongside the 0268 env-scale tests.
- `vxn-1b/PARAMETERS.md` regenerated: one added row in the Sources table.
- Suite green across `vxn1b-engine`, `vxn1b-clap`, `vxn1b-ui-web`,
  `vxn1b-web-controller` — 298 engine lib tests + integration, 0 failures. No new
  clippy warnings.
- Origin: a report that `Spread → Env 1 Scale` "doesn't vary envelope phase
  lengths across the stack". It does, but only with the Spread knob up *and* a
  stack wider than one — measured `env_scale = [0.5, 0.794, 1.26, 2.0]` at
  `spread = 1.0`, all `1.0` at `0.0`. `StackPos` removes the knob from that
  dependency; `Spread` keeps its 0260 semantics.
