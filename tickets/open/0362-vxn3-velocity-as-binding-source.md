---
id: "0362"
product: vxn-3
title: "vxn-3 velocity as a fourth flavour-binding source — patterns that breathe, not just get louder"
priority: high
created: 2026-09-04
epic: E034
---

## Summary

Velocity does exactly one thing in every family: scale output amplitude. `peak[k] = velocity`
in Driven and Noise; Metal and Struck fold it into the excitation level. Nothing else in a
voice responds to how hard a step was hit — not decay, not brightness, not sweep depth, not
band centre.

That is why patterns don't breathe. Accent on an 808 is level-only and that is fine for an
808, but a synthesis machine with a full parameter space per family is leaving its best
expressive lever unused. The p-lock system can automate the three macros per step, but that
is *authoring* a change, not *playing* one — and it costs a p-lock lane per parameter.

The flavour runtime already has the mechanism. `Binding { slot, param, curve, depth }`
([flavour.rs:86-91](../../vxn-3/crates/vxn3-engine/src/flavour.rs#L86-L91)) is additive-from-base
over a slot index, and `resolve` takes `macros: &[f32]` as a plain slice
([flavour.rs:224](../../vxn-3/crates/vxn3-engine/src/flavour.rs#L224)) — it does not care
that the slice happens to be `MACRO_SLOTS` long. Velocity becomes a source by passing a
4-element slice.

## Design

- **Reserve slot index `MACRO_SLOTS` (= 3) as Velocity.** Not a new struct field, not a new
  binding kind — a slot-index convention, documented in `flavour.rs` and ADR 0005.
- **Engines build a resolve slice of `MACRO_SLOTS + 1`** at trig: the three live macro
  values followed by the trig's velocity. Every engine already calls `resolve_patch()` from
  `on_trig`, but currently only when `dirty` — with a velocity binding present the resolve
  must run on **every** trig, since velocity changes per hit. Gate on
  `dirty || flavour.binds_velocity()` so a flavour with no velocity binding keeps today's
  "resolve only when stale" behaviour and today's cost.
- **Amplitude must not double-dip.** Voices currently multiply by velocity *and* would now
  optionally bind it. That is the author's problem, not the runtime's — but note it in the
  param docs, and consider whether a velocity-bound `Decay` wants `peak` left alone.
- **Serialisation is unchanged in shape.** `macro_defaults` stays `[f32; MACRO_SLOTS]`;
  slot 3 has no default because velocity comes from the trig. `Binding.slot` is already a
  `u8` with room. Confirm the deserialiser accepts `slot == 3` and rejects `slot > 3`.
- **`flavour_macro_display`** ([flavour.rs:239](../../vxn-3/crates/vxn3-engine/src/flavour.rs#L239))
  and the 0172 value-text path iterate slots `0..MACRO_SLOTS` — they must keep ignoring
  slot 3, which is not a host macro and has no knob.
- **Author at least one flavour using it** so the feature ships audible: a hat whose
  brightness tracks velocity, or a kick whose punch does.

## Acceptance criteria

- [ ] A binding on slot 3 makes a family param track trig velocity: two trigs at different
      velocities on the same flavour produce outputs differing in the bound dimension (not
      merely in level) — e.g. bound to `Decay`, the quiet hit is measurably *shorter*, not
      just quieter.
- [ ] A flavour with **no** slot-3 binding resolves at most as often as it does today
      (assert via a resolve counter or by the existing
      `change_takes_effect_on_next_trig_not_mid_voice` staying green).
- [ ] Per-trig resolve stays allocation-free — alloc-trap extended to a velocity-bound
      flavour under a dense pattern.
- [ ] `slot == 3` round-trips through serialize/deserialize; `slot > 3` is rejected.
- [ ] Host macro readout and value-text ignore slot 3 (no phantom fourth knob); existing
      `value_to_text` tests green.
- [ ] At least one authored factory flavour binds velocity.
- [ ] `cargo test -p vxn3-engine -p vxn3-clap` green; clippy clean; `clap-validator` 0
      failures.

## Notes

- ADR 0005 calls the binding table "one source type (a macro knob)" and explicitly defers a
  general matrix. This adds a **second source, still additive-from-base, still per-trig** —
  well short of the vxn-2 matrix. Update the ADR's Decision section rather than leaving the
  text contradicting the code.
- The faceplate flavour editor (**0185**, still open) will need a source column in its
  binding surface. Either land this first so 0185 designs for two sources, or accept a small
  follow-up. Prefer the former.
- Relatedly out of scope: velocity curves per binding beyond the existing `Curve::{Linear,
  Exp}`, and a global accent lane in the sequencer.
