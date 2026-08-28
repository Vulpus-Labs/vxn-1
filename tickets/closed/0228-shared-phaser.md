---
id: "0228"
product: monorepo
title: "Shared StereoPhaser (vxn-2 superset) — vxn-1 + vxn-1b adopt, outer fades deleted"
priority: medium
created: 2026-08-02
epic: E041
depends: ["0227"]
---

## Summary

First ticket of [E041](../../epics/open/E041-shared-fx-unification.md). vxn-2's
[phaser.rs](../../vxn-2/crates/vxn2-dsp/src/phaser.rs) is a strict superset of
vxn-1's ([phaser.rs](../../vxn-1b/crates/vxn-dsp/src/phaser.rs)): same allpass
core, plus `PhaserParams` snapshot, `set_enabled` + `mix: Smoothed` +
`mix_primed`, `wet_makeup()`. Move the vxn-2 kernel to
`vxn-core-dsp::phaser` implementing `FxKernel`; vxn-1 and vxn-1b adopt it.

## Acceptance criteria

- [ ] Move commit: vxn-2 render hash unchanged (pure move for vxn-2).
- [ ] Adoption commit (vxn-1 AND vxn-1b together — parity oracle must not
      break in a window): vxn-1 maps its positional `set_params(rate, depth,
      fb, mix)` onto `PhaserParams` and deletes `phaser_fade`
      ([lib.rs:180-184](../../archive/vxn-1/crates/vxn-engine/src/lib.rs#L180-L184));
      vxn-1b drops the PHASER slot fade in
      [fx.rs](../../vxn-1b/crates/vxn1b-engine/src/fx.rs). No kernel wrapped
      by both an internal WetFade and an outer fade (grep check).
- [ ] `REBASELINE:` commit only: vxn-1 phaser-toggle declick expectations +
      baseline where the patch engages phaser; vxn-1b zipper/d4. A/B rendered
      notes attached; user listens in Reaper first.

## Notes

vxn-2 pins 4 stages / CENTER_HZ 600 / spread/width/jitter; check vxn-1's
audible surface maps cleanly — any param vxn-1 exposes that the superset
lacks blocks this ticket (none expected per survey). [[verify-audio-in-reaper]]

## Close-out (2026-08-28)

Three commits, not the ticket's three: the `REBASELINE:` one had nothing to
carry (below).

### Move — `226ae64`, pure for vxn-2

vxn-2's kernel is now
[`vxn-core-dsp::phaser`](../../crates/vxn-core-dsp/src/phaser.rs) implementing
`FxKernel`, with its hand-rolled `enabled` + `mix: Smoothed` + `mix_primed` trio
replaced by `WetFade`. [`vxn2-dsp::phaser`](../../vxn-2/crates/vxn2-dsp/src/phaser.rs)
is a 17-line shim.

It needed a leaf the ticket does not mention: the per-stage break-frequency
scatter is PRNG-seeded, and vxn-2's `rng::xorshift_step` was `pub(crate)`. It
moves to `vxn_core_utils::math::xorshift64_star`, with `vxn2-dsp::rng` a
re-export. **This is the whole of the audible delta for vxn-1b** — the two
crates used genuinely different generators (plain xorshift scaled by `i64::MAX`
vs the star multiplier), so the same seed draws a different ±3 % scatter and the
four notches shift. `vxn-dsp/src/math.rs:9` had documented that divergence; it
was not a fork to be reconciled, so vxn-2's stream had to win for the move to
stay pure.

**Purity measured, not argued.** A fingerprint hashed every output bit across
five transitions — on-from-load, knob move, switch-off + settle, re-enable, and
off-from-load — before and after: `0x11f1075ad7495f74` / `0x46dd845171cc5a51`,
identical. The engine baseline was no use here (its reference patch leaves the
phaser off, so it cannot see a phaser change either way); it prints
`0x533a37a7def1921a`, the value 0226 and 0227 recorded. asm-check: all nine
watched paths unchanged.

Two decisions worth recording, both documented on the types:

- **`WetFade::set(on, mix)` is new**, and the move needed it. The separate
  setters prime on whichever call lands first, so `set_enabled(true)` then
  `set_mix(m)` snaps to the *default* target and glides down to `m` — an
  audible ride-in on any patch loading with the effect engaged. 0229–0232 will
  hit this too.
- **The kernel ignores `EdgeAction::RisingClear`.** vxn-2 did not clear there,
  so honouring it would have broken purity; and the phaser's entire state is
  four one-pole allpass sections plus a feedback sample, flushed within a few
  samples under a wet mix still ramping from zero. `RisingClear` earns its keep
  on tails and detectors, not here.

### Adoption — `0eebb8d`, vxn-1b only

The ticket asks for vxn-1 and vxn-1b in one commit so the parity oracle never
breaks in a window. vxn-1 was archived 2026-08-27, so there is no oracle and no
`phaser_fade` to delete — the criterion is met vacuously.

[`fx.rs`](../../vxn-1b/crates/vxn1b-engine/src/fx.rs): the fade/on-state arrays
go 5 slots → 4 and the phaser has **no entry at all**, so `run_phaser` gates on
`is_active()` and true-skips ([fx.rs:347](../../vxn-1b/crates/vxn1b-engine/src/fx.rs#L347)).
`reset` calls `phaser.reset()`, not `clear()` — re-idling now means settling the
internal fade too.

**Grep check passes, and stronger than asked.** `WetFade` appears in exactly two
places repo-wide: the shared phaser, and `Bypassable<K>` (0232's tool, unused).
`grep PHASER fx.rs` → 0 hits. The other four slots are held internally on
(`on: true`) with the outer fade owning bypass, unchanged.

### No `REBASELINE:` commit

vxn-1b has no render golden, and neither `zipper_regression` nor the d4 suites
touch the phaser — so no committed expectation encodes the delta. Manufacturing
a `REBASELINE:` commit would have recorded a change no test was making. User
confirmed the new sound in Reaper (2026-08-28) before this closed.

What was genuinely missing was coverage: the slot's bypass had only ever been
tested through the outer fade that just went away. Two tests replace it —
`fx::tests::phaser_bypass_settles_to_a_bit_exact_skip` and
`fx::tests::phaser_switch_off_glides_rather_than_cutting`.

### Fallout — `6016ac2`

asm-check had been failing `engine::Engine::process_block` (111 vs floor 180)
since **before this ticket**, and nothing had de-vectorised: ticket 0318
outlined half that function into `render_layer`, a symbol the watch list had
never heard of. Confirmed by building the dylib at `707d296` in a throwaway
worktree (282, no `render_layer`) against HEAD (111 + 120), with `busy_profile`
at 16.1 / 16.1 / 15.8x vs 16.2 / 16.1 / 16.1x. A `Watch` now spans a set of
needles, because the inverse case is silent — an outlined loop that *also* went
scalar would have looked identical, and lowering the floor would have blessed
it. Fixed a second gap while there: the watch's `why` had always cited the
OutputStage decimator, which was a separate symbol at the 0223 capture and so
was never counted.

### Verification

- Workspace **1399 passed / 0 failed**, 86 suites (1394 after adoption, plus
  the two FX-slot tests and three asm-check tests below).
- vxn-2 dev render hash `0x533a37a7def1921a` — unchanged.
- asm-check green on all nine paths.
