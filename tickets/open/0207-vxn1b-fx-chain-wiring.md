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
</content>
