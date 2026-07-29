---
id: "0207"
product: vxn-1b
title: "Wire the serial FX chain (chorus → phaser → delay → reverb → dynamics) into vxn1b-engine"
priority: high
created: 2026-07-29
epic: E037
depends: ["0206"]
---

## Summary

Wire all five effects into a serial post-voice FX chain in `vxn1b-engine`, at the
global oversample rate, with per-effect on/off + wet params. This is the engine
half of E037; the UI tab strip is [[E038]]. All five kernels now live in
`vxn-dsp` — chorus/phaser/delay/reverb already, dynamics from
[0206](0206-vxn1b-dynamics-kernel-vxn-dsp.md).

Currently `vxn1b-engine` instantiates **no** FX kernels; the post-voice sum goes
straight to master volume.

## Design

**Chain order (ADR 0001 §8):** synth → chorus → phaser → delay → reverb →
dynamics → master. Order is lockable during the ticket if a reason surfaces, but
this is the ADR default.

- **Insertion point.** In `Engine::render_control_block()`
  ([engine.rs:165](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L165)), the two
  `RenderBank`s accumulate into stereo `l`/`r`
  ([engine.rs:186-207](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L186-L207))
  before master volume ([engine.rs:214](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L214)).
  Run the FX chain on `l`/`r` **after** the bank sum and **before** master
  volume.
- **Rate.** The buffers are at `os_sample_rate` (length `base_frames · os`).
  Instantiate every kernel at `os_sample_rate` and process per sample across the
  block. OS is currently hard-wired to 1× in `build_ctx()`
  ([engine.rs:263-264](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L263-L264)),
  so in practice the chain runs at base rate today; when real OS lands (E036/E037
  OS work) the chain follows for free because it keys off `os_sample_rate`.
  Re-instantiate/retune kernels when the sample rate or OS factor changes (same
  place the banks pick up `os_sample_rate`).
- **Params.** Add a per-effect param group to
  [params.rs](../../vxn-1b/crates/vxn1b-engine/src/params.rs), slotted after
  `Oversample` ([params.rs:185](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L185))
  and before `MatrixSlot0Depth` ([params.rs:187](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L187)),
  following the `b()` (bool) / `f()` (float) / `e()` (enum) descriptor helpers.
  Minimum per effect: an **on/off** bool + a **wet/mix** float. Expose the
  handful of per-effect character params the existing kernels' `set_params` /
  `set_from` need (rate/depth for chorus & phaser, time/feedback for delay, size/
  decay for reverb, the dynamics 8). Keep the surface tight — mirror what VXN2 /
  VXN1 already expose for these kernels rather than inventing knobs.
- **Off-path is a true skip.** When an effect is off, skip its DSP entirely — do
  **not** run a wet=0 multiply through the kernel (epic risk: five serial
  effects at OS rate is real CPU). Each kernel already glides on the on/off edge
  (dynamics/phaser/delay/reverb fade wet to 0 before reverting to bit-exact
  passthrough), so the skip is safe once the fade has settled — gate on the
  kernel's `is_active()`-style predicate where it exists, else on the on flag
  plus a settle guard, matching each kernel's discipline.
- **Default patch: FX off / neutral.** Every effect defaults off; the default
  patch is unchanged audio vs. today (bank sum → master). Assert this.

## Acceptance criteria

- [ ] Serial chorus → phaser → delay → reverb → dynamics runs between the bank
      sum and master volume in `render_control_block()`, at `os_sample_rate`.
- [ ] Per-effect on/off + wet params exist in the table (added after
      `Oversample`, before the matrix slot block); `ParamId::COUNT` and
      value-text round-trip stay correct.
- [ ] Each effect bypasses to identity when off: a test drives audio through the
      chain with all effects off and asserts bit-exact (or within settle
      tolerance) equality to the no-FX path.
- [ ] The chain is allocation-free on the render path (no per-block allocation;
      kernels constructed at prepare / rate-change only).
- [ ] Default patch is audibly unchanged: existing render-parity tests stay green
      with the FX section present-but-off.
- [ ] Dynamics-at-1× aliasing checked: verify clean at the default OS rate; note
      in close-out if 1× needs a caveat (per epic risk).
- [ ] Both suites green (shared `vxn-dsp` — `vxn-no-parallel-cargo-test`).

## Notes

- Per-effect wet as a mod-matrix **destination** (ADR §2) stays a later
  candidate — this ticket is host-automation params only, matching how VXN2
  treats dynamics/phaser (`DynamicsParams` is host-automation, not a matrix dest).
- The `LimiterOn` param ([params.rs:184](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L184))
  and the master limiter are separate/unwired — out of scope here unless the
  chain ordering forces a decision on it.
- UI (tab strip, per-effect panels) is [[E038]] — this ticket ships the engine
  wiring only; params are automatable from the host without UI.

## Close-out (2026-07-29)

- **Chain wired.** New [fx.rs](../../vxn-1b/crates/vxn1b-engine/src/fx.rs)
  (`FxChain` + `FxParams`): serial chorus → phaser → delay → reverb → dynamics,
  inserted in [engine.rs](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L210)
  between the bank sum and master volume, run at `os_sample_rate` (1× today, so
  it follows the OS factor once that lands). `FxChain::new` in `Engine::new`,
  `fx.reset()` in `Engine::reset`.
- **True-skip bypass, click-free toggle.** Each slot carries a 10 ms `Smoothed`
  bypass fade; kernels are held internally on (their `mix` arg = the musical wet
  amount) and the fade owns on/off — the same split VXN1's `MasterFx` uses for
  its reverb. Steady-off (`!on && fade.current() == 0.0`) never calls the
  kernel's `process` — a real skip, not a wet=0 multiply (E037 CPU risk).
  `Smoothed` snaps within `SNAP_EPS`, so the fade reaches exactly 0 and the skip
  re-arms; off→on edge clears the slot's kernel so a re-enabled delay/reverb
  doesn't dump a stale tail.
- **26 params** in [params.rs](../../vxn-1b/crates/vxn1b-engine/src/params.rs),
  slotted between `Oversample` and the matrix depths: per-effect on + mix + a few
  character knobs (ranges mirror VXN1's FX section; the dynamics eight mirror
  VXN2's kernel clamps). All default **off/neutral** — the factory patch is
  FX-free. Matrix slot CLAP ids shifted +26; fine — persistence is name-keyed
  ([[vxn1-id-stability-dropped]]).
- **Bypasses to identity when off.** `fx::tests::all_off_is_bit_exact_passthrough`
  drives audio through the whole chain with every effect off and asserts bit-exact
  equality to the input; `toggling_off_settles_back_to_bit_exact_skip` proves a
  toggled-off slot returns to the exact-skip path once the fade settles;
  `enabling_an_effect_changes_the_output` and `reset_snaps_to_bypass` round it out.
- **Default patch audibly unchanged.** All `render::tests` parity checks stay
  green — FX-off is a transparent passthrough, so VXN1 render parity is intact.
- **Alloc-free.** Kernel ring buffers allocate once in `FxChain::new` (called
  from `Engine::new`); `process_block` performs no allocation.
- **Green** (`vxn-no-parallel-cargo-test`, run once, captured): `cargo test -p
  vxn1b-engine --lib` = 95/95 (4 new fx tests + parity). All vxn1b crates build
  (the CLAP shell derives its count from `ParamId::COUNT`, auto-adjusts). clippy
  clean (only the pre-existing vxn-dsp `tap`-index warning).
- **Deferred — dynamics-at-1× aliasing (epic risk).** The saturator can alias on
  fast transients at 1×; verifying "clean at the default OS rate" is an ear check
  in a DAW ([[verify-audio-in-reaper]] — audio is verified manually, not via a
  headless harness). Pending a Reaper pass; note a 1× caveat there if needed. OS
  plumbing itself is still 1×-hardwired (E036/E037 OS work), so this is the one
  acceptance item that can't close from code alone.
</content>
