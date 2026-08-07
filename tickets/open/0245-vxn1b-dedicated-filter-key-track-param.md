---
id: "0245"
product: vxn-1b
title: "Dedicated filter key-track param (0..1, C0 pivot); Key matrix source back to C4-centred"
priority: medium
created: 2026-08-06
epic: E036
depends: ["0200", "0202"]
---

## Summary

VXN1b dropped VXN1's `filter_key_track` param and expressed filter key-tracking
as a **Key → Cutoff** matrix route
([matrix.rs:349](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs#L349)). That
does not reproduce VXN1's control, for three reasons:

1. **The slider isn't the same control.** Slot depth is bipolar `[-1, 1]`, so
   unity tracking sits at
   [`KEY_CUTOFF_UNITY_DEPTH`](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs#L306)
   `= 0.25` — a quarter of travel, unmarked. VXN1's `filter_key_track` is
   `0.0..1.0` where `1.0` *is* 1 oct/oct.
2. **Pivot.** The Key source emitted octaves relative to **C4**, but VXN1's
   key-track pivots at **C0** (`(note − 12) · amt`,
   [voice.rs:1396](../../vxn-1/crates/vxn-engine/src/voice.rs#L1396) — ticket
   0100 moved it there deliberately). Same slope, 48 st flat: the same base
   cutoff sounded four octaves darker in VXN1b. The C0 pivot is what makes
   "cutoff min (`16.3516` Hz = C0) + track at max ⇒ cutoff *is* the played
   note" hold — a designed coincidence of two calibrations
   ([vxn-1 params.rs:556](../../vxn-1/crates/vxn-app/src/params.rs#L556),
   [vxn-1b params.rs:534](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L534)).
3. **The engine already needs the number the matrix doesn't have.**
   [`drift_key_track()`](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L144)
   reverse-engineers the tracking amount by scanning slots for Key→Cutoff and
   dividing out `DEST_GAIN[Cutoff]`, purely to feed the drift coupling (VXN1
   tracks the VCF to the VCO's *drifted* pitch).

Key-track is filter **calibration**, not modulation: static per note, wants to
be exact, and shouldn't be unwirable. Give it a param; leave the matrix route
available on top as the *free* amount (env-scaled, negative, curved tracking —
things VXN1 can't do).

## Design

- **New patch param** `filter_key_track`, `0.0..1.0` linear, default `0.0` —
  VXN1's descriptor verbatim. Placed with the filter block (after `HpfCutoff`).
  Cutoff mod becomes `matrix Cutoff dest + (note − 12) · filter_key_track`
  semitones, summed exactly as VXN1 sums key-track with its other cutoff
  sources ([voice.rs:1421-1428](../../vxn-1/crates/vxn-engine/src/voice.rs#L1421)).
- **`drift_key_track()` reads the param** instead of scraping the matrix; the
  matrix-topology scan goes.
- **Drop the pre-wired Key→Cutoff factory slot** ([matrix.rs:359](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs#L359)).
  Its only reason to exist was standing in for the missing param; removing it
  frees slot 1 and leaves the factory patch (Env2→Amp, LFO1→Pitch) intact.
  `KEY_CUTOFF_UNITY_DEPTH` stays as the documented matrix-route calibration.
- **Key source reverts to C4-centred** (`(note − 60)/12`). With parity carried
  by the param, the source is free to be the better *generic* modulator:
  signed around middle for Key→Pitch/Amp tilts. This reverts the C0 pivot
  applied while diagnosing the divergence.
- **State version bump** to `4` — the param block is positional
  ([state.rs:53](../../vxn-1b/crates/vxn1b-engine/src/state.rs#L53)). Presets
  are name-keyed TOML, so they need no migration.
- **Faceplate:** one declarative fader in the Filter panel
  ([faceplate.html:133](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.html#L133)),
  labelled `Key` to match VXN1
  ([vxn-1 lib.rs:1208](../../vxn-1/crates/vxn-ui-web/src/lib.rs#L1208)).

## Acceptance criteria

- [ ] `ParamId::FilterKeyTrack` exists, `0..1` linear, default `0.0`; removed
      from the "params that should be gone" assert in
      [params.rs:897](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L897).
- [ ] Test: at `filter_key_track = 1.0` the cutoff shift equals `note − 12`
      semitones across the MIDI range — VXN1's `resolve_mod` formula, pivot
      included; at `0.0` the contribution is exactly zero.
- [ ] Test: with cutoff at its minimum (`16.3516` Hz) and key-track at `1.0`,
      the per-voice cutoff Hz equals the played note's frequency (within
      `fast_exp2` tolerance).
- [ ] Test: `drift_key_track` follows the param, not the matrix — a patch with
      no Key→Cutoff slot and `filter_key_track = 1.0` still couples cutoff to
      osc drift; the matrix scan is gone.
- [ ] Key source emits octaves relative to C4 again (`eval` test updated), and
      a Key→Cutoff route at `KEY_CUTOFF_UNITY_DEPTH` still gives 1 oct/oct
      *on top of* the param.
- [ ] `default_patch()` no longer seeds a Key→Cutoff slot; slot 1 is inert.
      `tests/parity.rs` still passes.
- [ ] State `VERSION` = 4; round-trip test covers the new param.
- [ ] Faceplate shows a `Key` fader in the Filter panel; both layers address
      their own instance through the 0216 outer map.

## Notes

- Out of scope: the `scale_norm` clamp on Key as a *scale* source
  ([matrix.rs:92](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs#L92)) — a
  C4-centred Key clamps to `[0, 1]`, so only C4–C5 is a usable ramp. Wants a
  separate scale normalisation (`note/127`); file separately if it bites.
- The param is per-layer (patch block), so the two-layer expansion gives each
  synth its own key-track — see [[vxn1b-two-layer-param-map]].
- Adding a param shifts every later CLAP id; that's fine, id stability is not a
  constraint — see [[vxn1-id-stability-dropped]].
