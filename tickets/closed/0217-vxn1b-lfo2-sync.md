---
id: "0217"
product: vxn-1b
title: "LFO2→LFO2 cross-layer sync — L2 slaves to L1, rate + phase lock"
priority: medium
created: 2026-07-31
epic: E039
depends: ["0214", "0216"]
---

## Summary

Add the one cross-layer coupling in the two-synth design: **Layer 2's LFO2 can
sync to Layer 1's LFO2** (L1 = master). Per
[ADR 0002](../../vxn-1b/adrs/0002-vxn1b-dual-layer.md) §5.

## Design

- **Sync flag** on Layer 2 (blob state, or a param if automation is wanted —
  blob preferred, matches KeyMode).
- **Semantics: rate + phase lock.** When synced, L2's LFO2 does not run its own
  phase accumulator — it **mirrors L1's LFO2 phase** (and therefore rate). This
  gives true locked stereo movement (both layers' LFO2-driven routes move
  together), not merely matched rate with drifting phase.
- **Master = Layer 1.** L2 reads L1's LFO2 phase each control block. If Layer 1
  is off (can't happen — L1 always on) or LFO2 sync is off, L2's LFO2 free-runs
  from its own patch settings.
- **"Synced, different timbre" idiom**: wire `LFO2 → <dest>` in *each* layer's
  matrix ([[0216]]) and enable sync — same modulation shape, independent
  per-layer timbres.

## Acceptance

- Sync flag persists; when set, L2 LFO2 phase tracks L1 LFO2 phase (rate + phase).
- Test: with sync on and matching depths, L1/L2 LFO2-driven dests move in phase;
  with sync off, they free-run independently.
- Allocation-free; no cost when sync off.

## Close-out (2026-08-11)

- Link is a `KeyState` blob flag, not a param, as the ticket preferred:
  [`lfo2_link`](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L143), driven by
  `KeyOp::SetLfo2Link` ([engine.rs:174](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L174))
  and `Engine::set_lfo2_link` ([engine.rs:444](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L444)).
- Rate + phase lock with layer 1 as master: the engine passes
  `self.synths[0].lfo2_phase()` down when the flag is set, `None` when it isn't
  ([engine.rs:625](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L625)).
  `Synth::render_control_block` takes it as `lfo2_link: Option<f32>` and mirrors
  the master phase rather than advancing its own accumulator
  ([synth.rs:192-207](../../vxn-1b/crates/vxn1b-engine/src/synth.rs#L192-L207)).
  Off ⇒ `None` ⇒ free-run from the layer's own patch, no added cost.
- Persists: packed into the key blob
  ([engine.rs:195](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L195), decoded
  at [engine.rs:207](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L207)) and
  round-tripped in `state::tests` ([state.rs:347](../../vxn-1b/crates/vxn1b-engine/src/state.rs#L347)).
- Named *link*, not *sync*, to keep it distinct from the per-layer automatable
  `lfo2_sync` tempo-sync param
  ([engine.rs:96](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L96)).
- UI lives in the LFO 2 panel strip, not Tab 3 — per 0220's amendment of the
  original "lives on the global tab" note.
- Shipped in a5293ed. Manual DAW verification waived by the user (2026-08-11).
