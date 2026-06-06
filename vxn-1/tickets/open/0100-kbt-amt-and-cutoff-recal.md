---
id: "0100"
title: KBT as 0..1 amt over C0; cutoff recal to Jupiter-8 range
priority: medium
created: 2026-06-06
---

## Summary

Two linked filter-section changes:

1. **`filter_key_track` becomes an amount slider (0..1)**,
   replacing the boolean switch. The KBT curve also moves its
   reference note from C4 (MIDI 60) to C0 (MIDI 12), so the
   shift is `(note − 12) · amt` semitones added to cutoff
   instead of `(note − 60) · 1.0` for `true` only.
2. **Cutoff range recalibrates to Jupiter-8 convention** —
   roughly 20 Hz to 16 kHz, log/exp taper, *no* C4-pinned
   midpoint. The current "C0..20 kHz with C4 at fader 0.5"
   taper exists so that "keytrack on, play C4" lands exactly
   at the slider value — once KBT is referenced to C0 that
   coincidence no longer matters, and the C4 pin distorts the
   taper toward overly-bright defaults.

Factory presets that used `filter_key_track = true` migrate to
an explicit `filter_key_track = 1.0` (or a tasteful lower
value) and re-tune `cutoff` so the patch sits where it used to.

## Acceptance criteria

### Param table

- [ ] `crates/vxn-app/src/params.rs`: `filter_key_track` descriptor
      changes from `b(…)` (bool) to `f("filter_key_track", "Key
      Track", 0.0, 1.0, 0.0, "", Taper::Linear)`. Default is
      `0.0` (no track) to keep recall deterministic across the
      bank.
- [ ] `PatchParam::FilterKeyTrack` discriminant unchanged in
      name; per [[vxn1-id-stability-dropped]] no append-only
      discipline.
- [ ] `cutoff` descriptor retuned. Proposed:
      ```rust
      f("cutoff", "Cutoff", 20.0, 16000.0, 1000.0, "Hz",
        Taper::Exp { mid: 800.0 })
      ```
      …subject to taste in the live plugin — feel free to nudge
      to 18 kHz top or shift the mid pin within the 600–1200 Hz
      band. Don't keep the C4 pin (261.6256).
- [ ] The `exp_taper_pins_min_mid_max_when_min_positive` test
      updates to whatever pins the new descriptor uses.

### Engine / voice

- [ ] `crates/vxn-engine/src/voice.rs` `resolve_mod`: replace
      ```rust
      let key_track = if ctx.filter_key_track {
          s.note as f32 - 60.0
      } else { 0.0 };
      ```
      with
      ```rust
      let key_track = (s.note as f32 - 12.0) * ctx.filter_key_track;
      ```
      …and `BlockCtx::filter_key_track` changes from `bool` to
      `f32`.
- [ ] `set_block_ctx` (or wherever `filter_key_track` is read
      out of `ParamValues`) reads it as f32, not bool. Drop the
      `p.bool(PatchParam::FilterKeyTrack)` call site.
- [ ] Engine test `filter_key_track_opens_cutoff_with_pitch`
      adapts: drive with `filter_key_track = 1.0` for the
      "on" case, `0.0` for "off", confirm the same monotonic
      "higher note → brighter" relationship.

### Faceplate

- [ ] `crates/vxn-ui-web/assets/faceplate.html` line 184: replace
      ```html
      <div class="ctl-strip" data-control="switch" data-param="filter_key_track" data-label="KeyTrk"></div>
      ```
      with a fader (or compact slider — match the strip's
      visual budget):
      ```html
      <div class="ctl-strip" data-control="fader" data-param="filter_key_track" data-label="KeyTrk"></div>
      ```
      If the bottom strip can't host a full fader, lift KBT
      into the panel body alongside cutoff/res. Pick what
      reads cleanest in the live preview.
- [ ] `crates/vxn-ui-web/src/lib.rs` line 1425: update the
      control-kind tuple from `"switch"` to `"fader"` (or
      whichever primitive matches the placement choice above).

### Factory presets

- [ ] Audit every preset under
      `crates/vxn-engine/presets/factory/**/*.toml` that has
      `filter_key_track = true`:
      - Replace `filter_key_track = true` with
        `filter_key_track = 1.0` (or the tasteful value the
        patch wants — Mini Bass / Boofy Bass probably want
        full track; pads often sound better at 0.3–0.5).
      - Re-tune `cutoff` so playing in the patch's expected
        register sits at the same brightness it used to. The
        old C4-pinned cutoff value will now bias bright; halve
        or quarter the Hz value as a first pass, then check by
        ear.
- [ ] Presets without `filter_key_track` continue to default to
      `0.0` (off) — no migration needed.
- [ ] Baseline tripwire test
      (`crates/vxn-engine/tests/baseline.rs`, modified per
      git status) regenerates if it hashes against patch
      audio; note the rationale in the commit.

## Notes

The C4 reference was a happy accident from putting the cutoff
slider's midpoint at C4 — meant a double-clicked default with
keytrack-on resonated at C4. Useful for sound-design, but it
masks the *amount* dimension: anything other than fully tracked
required either fudging cutoff or accepting odd note-to-
brightness coupling. With amt as a knob and the reference at
C0, the relationship is the textbook one and the cutoff slider
is a flat sound-design control.

C0 (MIDI 12) sits at 16.35 Hz; using it as the zero-shift
reference means *all* playable notes shift cutoff upward, which
is what 100% keytrack on a real Jupiter-8 does — the VCF opens
as you go up the keyboard.

The taper change is the real risk. The C4 pin made the
brightest half of the slider very dense (most of the audible
sweep happened in the bottom half). Removing it should make the
slider feel more linear-in-brightness across its range; if it
feels uneven, try `Taper::Exp { mid: 600.0 }` instead of `800.0`.

If the bottom-strip fader feels cramped, consider promoting
KBT into the filter panel body proper — it's an amount now,
not a strip toggle, so the placement signal matters.

If keytrack values above ~0.85 sound too aggressive on
high-register playing (because we removed the soft C4 reference
that held cutoff bounded), consider clamping the contribution
or applying a gentle compression curve in `resolve_mod`. First
pass: trust the maths.

## Touches

- `crates/vxn-app/src/params.rs`
- `crates/vxn-engine/src/voice.rs`
- `crates/vxn-engine/src/lib.rs` (the `bool` → `f32` set_param
  site + the existing test)
- `crates/vxn-ui-web/assets/faceplate.html`
- `crates/vxn-ui-web/src/lib.rs`
- `crates/vxn-engine/presets/factory/**/*.toml` — Mini Bass,
  Boofy Bass, Clavics, Split Bass & Lead, Great Divide (per
  current grep)
