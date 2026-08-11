---
id: "0244"
product: vxn-1b
title: "Assign modes: Unison/Solo/Twin allocation + per-voice unison detune"
priority: medium
created: 2026-08-05
epic: E036
depends: ["0198", "0202"]
---

## Summary

`AssignMode` and `UnisonDetune` exist in VXN1b's param table
([params.rs:184-186](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L184-L186)),
on the faceplate
([faceplate.html:220-221](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.html#L220-L221)),
and in presets — but **nothing in the engine read either**. The allocator built
in 0198 is flat poly, and [bank.rs](../../vxn-1b/crates/vxn1b-engine/src/bank.rs)
said so in its scope note: *"Poly only (matches the 0198 allocator); unison/mono
… deferred"*. So all four assign modes rendered bit-identically and the Detune
fader was inert in every one of them.

Measured before (held C4, default patch, `detune 0 vs 50 ct`, max|diff| of the
5 s tail) against VXN1 as the reference:

```text
                 vxn-1b (before)   vxn-1
Poly                 0.000         0.000
Unison               0.000         0.415
Solo                 0.000         0.000
Twin                 0.000         0.465
```

Poly and Solo are undetuned by design in VXN1 too — the gap is Unison and Twin.

Deferred work from the allocator ([0198](../closed/0198-vxn1b-mpe-voice-architecture.md)) and
the render bank ([0202](../closed/0202-vxn1b-matrix-evaluator.md)) — the piece of
[E036](../../epics/open/E036-vxn1b-matrix-engine.md)'s voice-allocation
architecture both of those left open.

## Design

**Not sample-parity with VXN1, deliberately.** VXN1 fans Unison across 8
channels in one bank; VXN1b allocates 16 voices per layer and stacks all of
them, so the same cents value spreads over twice as many copies. Parity was
already broken by the 16-voice allocation and is not a goal here — the
*behaviour* (what the Detune fader does in each mode) is.

Allocation policy lives in [`voice.rs`](../../vxn-1b/crates/vxn1b-engine/src/voice.rs)
as `note_on_mode` / `note_off_mode`, returning a fixed-capacity `Triggers` list
so note handling stays allocation-free:

- **Poly** — one voice, undetuned (the existing allocator).
- **Twin** — two distinct voices at ±`TWIN_SPREAD` × detune. The first is
  stamped before the second is drawn, so `allocate` sees it taken.
- **Solo** — monophonic on lane 0, undetuned; the other lanes are gated off so a
  switch out of Unison releases the stack rather than stranding it.
- **Unison** — every lane retriggers to the new note, fanned across
  `unison_spread(v) × detune`, each with a fresh random start phase.

Mono modes (Solo/Unison) get a 32-deep held-note stack with last-note priority:
releasing the sounding note reveals the one beneath, re-articulated unless
`legato`. A legato slide re-points pitch and re-stamps the fan without
retriggering, keeping the voice's latched note-random and allocation age.

Stack-width compensation is `1/√len` (not `1/len` — detuned, independently
phased copies sum as a random walk), carried to the bank on `BlockCtx` and
folded into the per-lane pan gains: one multiply per lane per block, nothing in
the frame loop.

## Acceptance criteria

- [x] `Voices::note_on_mode` / `note_off_mode` resolve all four assign modes and
      return the lanes to trigger; `Trigger::start_phase` overrides the bank's
      deterministic `lane_phase` for Unison
- [x] Per-lane `detune_cents` reaches both oscillators' pitch base in
      [bank.rs](../../vxn-1b/crates/vxn1b-engine/src/bank.rs) `render`
- [x] Mono held-note stack: last-note priority, buried-note release leaves the
      sounding note alone, legato slides without retriggering
- [x] `level_comp` = 1/√len applied, so switching assign modes doesn't jump level
- [x] Stacked modes take `UNISON_GLIDE_SCALE` on portamento
- [x] Mode-switch cleanup both directions; a note-off for a note not on the mono
      stack falls back to the poly path (no stranded gated-on voices)
- [x] Detune measurably changes Unison and Twin output, and provably does *not*
      change Poly or Solo (matching VXN1's table)
- [x] `default_patch_render_matches_vxn1` parity gate still passes (detune is 0
      at the factory default) and `hot_path_is_allocation_free` still passes
- [ ] Manual DAW check: Unison/Twin detune, Solo/Unison legato and last-note
      priority all behave in Reaper

## Notes

- UI needed no work — the faceplate already carried the `assign_mode`
  buttongroup and the detune-legato composite from the VXN1 fork.
- The Detune fader does not dim in Poly/Solo where it is inert. VXN1 has the
  same gap (it dims only the Legato toggle) and that behaviour is accepted;
  worth its own ticket if it ever bites.
- Sustain-pedal interaction in the mono modes is out of scope — VXN1b has no
  sustain-defer state in the allocator at all yet (see `steal_tier`'s note).
- Global drift was investigated alongside this and needs **no** work: it was
  already fully wired in VXN1b (osc pitch walk, cutoff key-track, per-lane
  component trims, distinct seeds per bank and per layer) and measures live at
  0.426 max|diff| vs VXN1's 0.311 on the same test.
- Landed against concurrent [0242](0242-vxn1b-cross-mod-panel-and-dest.md) work
  in `bank.rs`; `cargo fmt` was deliberately **not** run (the whole crate carries
  rustfmt diffs, so formatting would stomp unrelated in-flight work). Stage
  explicit paths — see [[vxn-concurrent-vxn2-work-no-git-add-all]].

## Close-out (2026-08-11) — superseded

Closed as **superseded by [0266](../open/0266-vxn1b-stack-width-and-voice-mode.md)**
and [ADR 0003](../../vxn-1b/adrs/0003-vxn1b-stack-width-and-voice-mode.md), not
as delivered-as-written.

The four-way `AssignMode` this ticket specced conflates two independent
decisions — lanes per note, and keyboard behaviour — and can only express four
of their combinations. It was implemented far enough to prove the mechanism
(mono held-note stack, detune fan, level compensation, legato slide, per-lane
start phases), and that mechanism **shipped**; what did not ship is the enum.
The surface landed as `stack_width` × `voice_mode` instead, which reaches every
old mode as a (width, mode) pair:

| Old mode | Width | Mode |
|---|---|---|
| Poly   | 1  | Poly |
| Twin   | 2  | Poly |
| Solo   | 1  | Solo |
| Unison | 16 | Solo |

So this ticket's *substance* is delivered — the Detune fader reaches the engine,
Unison/Twin-equivalent patches sound detuned, and the measured 0.000-vs-VXN1 gap
in the Summary is closed — while its *param surface* is gone. The remaining work
(stack-granular allocation, width 32, the 6-way UI) is tracked on 0266.
