---
id: "0123"
title: Filter keytrack follows drifted pitch, not raw key value
priority: medium
created: 2026-06-13
epic: E022
---

## Summary

Filter key-track currently references the raw integer MIDI
note: `key_track = (s.note − 12) · filter_key_track`
(`voice.rs:1201`), computed once per block in the pure
`resolve_mod` and folded into the shared `cutoff_mod`
(`voice.rs:1232`). The per-voice pitch drift
(`osc1.drift_value[v]` / `osc2.drift_value[v]`,
`voice.rs:863,871`) never reaches the cutoff.

In real hardware the VCF tracks the *keyboard CV*, and when
the VCO is drifting that CV-derived pitch drifts with it, so a
keytracked filter wanders in lockstep with the note's pitch
drift (scaled by the track amount). Today our VCF sits dead
still while the oscillators wander — the keytrack contribution
should instead follow the drifted pitch.

Make the keytrack term per-voice and add the voice's pitch
drift (in semitones) before scaling by `filter_key_track`:

```text
cutoff_keytrack[v] = (note − 12 + pitch_drift[v]) · filter_key_track
```

At `filter_key_track = 1.0` this adds the full ±0.25-semitone
drift to the cutoff (same magnitude as the pitch wander); at
`0.0` it contributes nothing, exactly as now.

## Acceptance criteria

- [ ] The keytrack contribution to cutoff includes the voice's
      pitch drift. With `filter_key_track > 0` and
      `drift_amount > 0`, two voices on the same note show
      cutoff offsets that differ over time, tracking each
      voice's pitch drift.
- [ ] `filter_key_track = 0.0` leaves cutoff bit-identical to
      pre-change (the drift term is gated out by the
      multiply — no keytrack, no drift coupling).
- [ ] `drift_amount = 0.0` leaves cutoff bit-identical to
      pre-change (no drift to couple). The layer-sum
      equivalence tests, which already pin `drift_amount = 0`,
      still pass unchanged.
- [ ] The keytrack term is now per-voice: the restructure
      moves the `(note − 12) · amt` computation (or the added
      drift component) out of the per-block `resolve_mod` into
      the per-voice cutoff site at `voice.rs:877`, or threads
      a per-voice drift value into the cutoff path. Document
      which, and keep `resolve_mod` pure.
- [ ] Keytrack feeds on the **mean** of `osc1.drift_value[v]`
      and `osc2.drift_value[v]` (decided). osc1/osc2 drift
      independently (separate salts); the mean is the voice's
      effective pitch drift for the VCF. Comment the choice at
      the call site.
- [ ] `tests/baseline.rs` hash updated if any factory/baseline
      patch has both keytrack and drift engaged, with a commit
      note attributing the delta to this ticket.

## Notes

`set_coeffs` is called per-voice inside the `for v` loop
(`voice.rs:878`), so the per-voice hook already exists — the
cutoff Hz at `voice.rs:877` just needs the extra per-voice
semitone term before `fast_exp2`. The cheap shape is to keep
the static `(note − 12) · amt` in `cutoff_mod` and add only
`pitch_drift[v] · filter_key_track` at the per-voice site, so
the per-block path is untouched for the common case.

Magnitude sanity: drift is ±`DRIFT_MAX_SEMITONES`(0.25)·amount
(`poly.rs:196`). At full keytrack that is ±0.25 semitone of
cutoff wobble — audible shimmer, not detune. No new clamp
needed; the existing cutoff clamp downstream still applies.

This is the tracking-path coupling only. An *independent* VCF
walk (the filter's own expo drifting separately from pitch) is
deliberately out of scope here — it belongs with the fixed/own
VCF variance in 0124's family, and would be a separate
`BoundedRandomWalk` if ever wanted.

Interaction: `PolyOtaLadder` ramps its coefficients internally
(see `smoothing.rs:17`), so a per-voice cutoff that now moves
every drift step feeds the ramp slightly more often — verify
no idle-path / silent-skip regression (memory:
silent-skip-filter-state freezes coeff ramps on silent blocks;
drift only ticks on sounding voices, so this should be inert,
but confirm).
