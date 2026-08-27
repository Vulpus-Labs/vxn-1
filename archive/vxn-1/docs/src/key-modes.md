# Key modes

The **Key Mode** decides what MIDI notes do at the *instrument* level — before they reach any individual voice. VXN1 always carries two layers (Upper and Lower); the key mode is the routing rule that maps incoming notes to those layers.

There are three modes. Key Mode is **not automatable** — it's stored as plugin state but cannot be moved with DAW automation lanes (ADR 0003).

## Whole

**16-voice mono-timbral.** Both layers play the same patch.

- Every note round-robins through both layers' 8 channels — effective polyphony is 16.
- Layer A holds the patch values. Layer B silently follows Layer A.
- The Layer switcher is fixed to Upper; Lower is not editable.

Use Whole for normal full-polyphony performance with a single patch.

## Dual

**8 + 8 layered stereo.** Both layers play the *same* notes simultaneously but with **different patches**.

- Every note triggers a voice on each layer.
- Layer A reads Upper params; Layer B reads Lower params.
- Both layers are editable via the Layer switcher.
- Use **Layer Level** on each layer's [Voice & assign panel](panels/voice.md) to balance.

Classic uses: detuned doubles (slight tuning offset between layers), pad + bell layering, dark filter sound under a bright transient.

## Split

**8 + 8 split at a MIDI note.** Each layer plays only certain notes.

- Notes **below** the split point trigger Lower; notes **at-or-above** trigger Upper.
- Both layers are independently editable.
- The split point is a separate plugin-state field (not a parameter), set via the **Split Point** control in the Key Mode panel. Default: MIDI note 60 (C4).
- Split point is **non-automatable** (ADR 0003).

Use Split for bass-and-lead splits, layered keyboard performances, or any time the left hand needs a different patch from the right.

## Seed-on-entry

When you switch *from* Whole *to* Dual or Split, Layer B (Lower) is empty — it's been silently following Upper. The engine copies Upper → Lower on the transition so both layers start with the same patch, then they diverge as you edit Lower.

Switching from Dual or Split *to* Whole drops Lower silently — no merge, no destructive prompt. Switching back to Dual/Split restores the seed-on-entry copy from Upper, not the previous Lower state. **If you have a Lower patch you want to keep, save it as a Patch preset before switching to Whole.**

## What's per-layer vs. global

The complete list of what each layer holds independently:

- Both oscillators (waves, tuning, levels, PW).
- Sub level, noise level + colour.
- Cross-Mod Type and Amount.
- Filter (HPF, VCF, mode, slope, key track).
- Both envelopes.
- LFO 1 (per-voice, layer-scoped).
- All modulation routes (pitch, PWM, filter mod, cross-mod sweep, mod-wheel routes).
- Amp Gate, Amp LFO route.
- Voice & assign panel (Assign Mode, Legato, Unison Detune, Glide Time, Layer Level, Spread).

What's **global** (shared by both layers, single value):

- Master Tune, Master Volume, Master Drift, Limiter, Oversample.
- LFO 2 (Shape, Rate, Sync).
- All effects: Phaser, Chorus, Delay, Reverb.
- Key Mode itself and the Split Point.
- Performance control *values* (mod wheel position, pitch wheel position) — the *routings* of those controllers are per-layer.

## Performance controls and key modes

MIDI controllers (mod wheel, pitch wheel, sustain, velocity) reach **both** layers in all modes. Each layer interprets them according to its own routing — so in Dual mode, Upper might use the mod wheel for filter cutoff while Lower uses it for PWM, both at the same time.
