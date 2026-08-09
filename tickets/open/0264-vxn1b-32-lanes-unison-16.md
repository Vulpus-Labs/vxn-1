---
id: "0264"
product: vxn-1b
title: "Widen Synth to 32 lanes (4 render banks), cap the Unison fan at 16"
priority: medium
created: 2026-08-08
epic: null
depends: []
---

## Summary

Each `Synth` owns 16 voices across two 8-lane [`RenderBank`](../../vxn-1b/crates/vxn1b-engine/src/synth.rs#L73)s. That
capacity is fine for Poly, but the multi-voice assign modes eat it: Twin takes
two lanes per note, so a Twin patch is 8-note polyphonic — thin for the exact
patches (fat detuned stacks) that want Twin in the first place.

Widen the synth to 32 lanes (4 banks) so Poly gets 32 notes and Twin gets 16.
Unison must **not** follow the widening: its stack width is a voicing decision,
not a capacity one, and a 32-wide fan over the same `UnisonDetune` cents would
change the character of every existing Unison patch. Cap it at the current 16.

The alternative designs were considered and rejected:

- *Let Layer 1 borrow Layer 2's pool when Layer 2 is off* — the overflow voices
  would need Layer 1's patch pushed into synth 2, plus a cross-synth allocation
  and stealing policy, which breaks the "allocation and stealing are **private
  to each synth** — no shared pool" invariant
  ([synth.rs:8](../../vxn-1b/crates/vxn1b-engine/src/synth.rs#L8)). It also
  leaves the spilled voices on synth 2's independently-phased LFO 2, and needs a
  policy for held spill voices when Layer 2 is switched on. More machinery than
  widening, and it only helps in Single mode.
- *Raise `vxn_dsp::MAX_VOICES`* — that const lives in vxn-1's shared crate
  ([lib.rs:47](../../vxn-1/crates/vxn-dsp/src/lib.rs#L47)) and would drag vxn-1
  along with it.

## Design

**Capacity.** Replace `const N: usize = MAX_VOICES`
([voice.rs:26](../../vxn-1b/crates/vxn1b-engine/src/voice.rs#L26)) with a
vxn-1b-local `const MAX_VOICES_1B: usize = 32`; leave `vxn_dsp::MAX_VOICES`
alone. `Synth::banks` becomes `[RenderBank; 4]` and `SynthSeeds::banks` becomes
`[u64; 4]` — **bank 0 and 1 keep their existing seed values** on both layers, so
lanes 0–15 render exactly as today. The hand-unrolled two-bank render fan at
[synth.rs:254-278](../../vxn-1b/crates/vxn1b-engine/src/synth.rs#L254-L278)
becomes a loop over `RenderBank::LANES`-sized chunks of the render view.
Everything else — allocator, stealing, matrix eval, `apply_envelopes` — is
already written against `N` / `&mut self.banks` and needs no change.

**Unison cap.** Add `const UNISON_LANES: usize = 16` and use it in the three
places the fan is sized:

- the stack width in `note_on_mode`
  ([voice.rs:357](../../vxn-1b/crates/vxn1b-engine/src/voice.rs#L357)) and its
  note-off twin
  ([voice.rs:412](../../vxn-1b/crates/vxn1b-engine/src/voice.rs#L412));
- the fan denominator in `unison_spread`
  ([voice.rs:54](../../vxn-1b/crates/vxn1b-engine/src/voice.rs#L54)).

The `for v in lanes..N { self.gate[v] = false }` cleanup keeps `N`, not
`UNISON_LANES` — it must still release lanes 16–31 when a Poly/Twin hold precedes
a switch into a mono mode. `level_comp(lanes)` already derives from `lanes`, so
it stays 1/√16 with no edit.

This makes Unison bit-identical to today: same lanes, same bank seeds, same fan
denominator, and `phase_rng` draws in the same order and count so the start-phase
sequence is unchanged.

**Triggers capacity.** `Triggers` is fixed-capacity `N`
([voice.rs:89](../../vxn-1b/crates/vxn1b-engine/src/voice.rs#L89)); size it to
`UNISON_LANES` instead, which is the real per-event maximum (Unison 16, Twin 2),
so note-on doesn't copy a 32-slot array.

## Acceptance criteria

- [ ] Poly sounds 32 simultaneous notes per layer; the 33rd steals. Test in
      `voice.rs` asserting 32 distinct active lanes before any steal.
- [ ] Twin sounds 16 notes (32 lanes); the 17th steals.
- [ ] Unison still stacks exactly 16 lanes, and lanes 16–31 stay inactive under
      Unison and Solo.
- [ ] Unison output is unchanged: a rendered Unison block is bit-identical to the
      same render before this ticket (capture a golden buffer on the current
      `main` and assert against it).
- [ ] A mode switch from a 32-note Poly hold into Solo/Unison releases every lane
      above the new stack width (no stranded voices in 16–31).
- [ ] Render-parity gate
      ([tests/parity.rs](../../vxn-1b/crates/vxn1b-engine/tests/parity.rs)) still
      passes — it holds one Poly note on lane 0, so widening must not move it.
- [ ] Idle cost unchanged: banks 2–3 take the `is_silent` early-out
      ([bank.rs:455](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L455)) when no
      lane in them is active. Confirm on the busy/idle profile harness that an
      idle-layer block does not regress.
- [ ] Nothing advertises 16-voice capacity to the host or in the faceplate — grep
      for a CLAP voice-info impl and any UI voice-count display, and update if
      present.

## Notes

- Per-voice DSP state doubles per synth (4 banks × 2 layers). Bytes, not cycles —
  the `is_silent` early-out means idle banks don't render.
- Real CPU doubles only when 32 notes actually sound, which is the point of the
  ticket.
- Deliberately **not** exposing stack width as a param. `UNISON_LANES` being a
  const makes a later "Stack Size" (8/16/32) control easy plumbing, but it would
  be a new patch param — CLAP id relayout plus a state `VERSION` bump — so it's
  out of scope here. Related: [0244](0244-vxn1b-assign-modes-unison-detune.md).
- The doc comment at
  [voice.rs:50-52](../../vxn-1b/crates/vxn1b-engine/src/voice.rs#L50-L52)
  ("VXN1 fans 8 channels; VXN1b allocates 16 per layer and stacks all of them")
  stops being literally true once the synth holds 32 — reword it to say the fan
  is capped at `UNISON_LANES` by choice.
- Orthogonal to the mirrored-layer idea (Layer 2 slaved to Layer 1's patch, own
  drift + a detune offset) discussed alongside this: that stays a separate
  ticket, and would compose with this one as 32 + 32.
