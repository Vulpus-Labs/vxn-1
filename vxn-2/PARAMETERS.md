# VXN2 — Parameter reference

Generated alongside ADR 0001 and revised by [ADR 0002 — Drop dual-layer
voicing](adrs/0002-drop-dual-layer.md). The faceplate UI at
`ui-mockup/index.html` is the canonical layout reference; this document
enumerates every parameter surfaced by that UI and explains its role in the
engine.

Notation:

- **Type** column: `i` = integer, `f` = float, `e` = enum (variants in
  brackets), `b` = bool.
- **Range** is the *plain* (user-facing) range. Normalisation to [0, 1] for
  CLAP and automation is parameter-specific (see `vxn2-engine` param
  descriptors when built).
- **Per-op** parameters repeat 6× with prefix `op1_`…`op6_`. Listed once.

Per [ADR 0002] a patch is a single parameter set — no Whole / Layer / Split
voicing, no Upper / Lower split. Every parameter below is per-patch.

---

## Per-operator (6×)

For each operator `op{1..6}`:

| Param           | Type | Range          | Default | Purpose                                                                                                                         |
|-----------------|------|----------------|---------|---------------------------------------------------------------------------------------------------------------------------------|
| `ratio_mode`    | e    | {Ratio, Fixed} | Ratio   | If `Ratio`, op frequency = note × `(num + fine/100) / denom × 2^(detune/1200)`. If `Fixed`, op frequency = `fixed_hz` (Hz), independent of played note. Inharmonic + percussive sounds typically use Fixed for one op. |
| `num`           | i    | 1 .. 32        | 1       | Numerator of the rational ratio. Whole-number harmonic multiplier; matches DX7 coarse ratio domain. |
| `denom`         | i    | 1 .. 8         | 1       | Denominator of the rational ratio. `denom > 1` yields sub-octave (1/2 = octave down) and just intervals (3/2 = perfect fifth, 5/4 = major third, 7/4 = harmonic seventh). |
| `fixed_hz`      | f    | 1.0 .. 9772.0  | 440.0   | Absolute frequency when `ratio_mode = Fixed`. Range matches DX7 fixed-mode span (~5 decades). |
| `fine`          | i    | −100 .. +100   | 0       | Fine-tune offset on the numerator in hundredths (effective `num = num + fine/100`). Provides continuous sweep between rational detents and reaches irrational/transcendental ratios (e.g. 7/5 + fine = √2). Sweep width scales inversely with `denom`. |
| `detune`        | i    | −100 .. +100   | 0       | Per-op detune in cents (±1 semitone). Log-domain offset on top of the rational ratio — use for thickening, beating, microtuning. Independent of `num`/`denom`. |
| `level`         | i    | 0 .. 99        | 99 (carriers) / 0 (mods) | Operator output level. For carriers this is amp; for modulators this drives FM index. DX7's primary timbre control. |
| `vel_sens`      | i    | 0 .. 7         | 3       | Velocity sensitivity for `level`. Higher = louder/brighter on hard hits. DX7-style 0-7 scale. |
| `eg_r1..r4`     | i    | 0 .. 99        | 99,50,35,60 | EG rates. R1 = attack speed, R2 = decay-to-sustain, R3 = sustain decay (slow drift), R4 = release. Higher = faster. |
| `eg_l1..l4`     | i    | 0 .. 99        | 99,70,50,0  | EG levels. L1 = peak after attack, L2 = decay target, L3 = sustain level, L4 = release floor (and pre-attack start). Carriers usually L4=0. |
| `ks_break_pt`   | i    | 0 .. 127       | 60 (C4) | Keyboard break point (MIDI note). At this note, key-scaling applies zero offset to `level`. |
| `ks_l_depth`    | i    | 0 .. 99        | 0       | Level scaling depth for notes *below* the break point. |
| `ks_r_depth`    | i    | 0 .. 99        | 30      | Level scaling depth for notes *above* the break point. |
| `ks_l_curve`    | e    | {+lin, −lin, +exp, −exp} | −lin | Shape of level scaling left of break point. `+` boosts, `−` cuts. Lin = linear, exp = exponential. |
| `ks_r_curve`    | e    | {+lin, −lin, +exp, −exp} | −exp | Shape of level scaling right of break point. |
| `ks_rate`       | i    | 0 .. 7         | 2       | Keyboard rate scaling. Speeds up EG rates as note pitch rises (mimics decay of plucked strings, etc.). Single value applies to all 4 EG rates. |
| `pan`           | f    | −1.0 .. +1.0   | 0.0     | Stereo pan for the op's output contribution to the stereo bus. **Carrier-only** — FM is mono in the engine, so modulator pan has no audible effect. UI disables this control when the selected op is a modulator under the current algorithm. |

**Why so many per-op params**: FM sound design lives in modulator EG + KS
configuration. Hidden / merged controls make patches sound flat. Per the ADR,
the EG and KS graphs in the op-detail panel collapse R1..R4 + L1..L4 into one
draggable widget and the 5 KS params into another, so editing density on the
faceplate is manageable despite the parameter count.

---

## Algorithm

| Param  | Type | Range  | Default | Purpose                                                                       |
|--------|------|--------|---------|-------------------------------------------------------------------------------|
| `algo` | i    | 1 .. 32 | 5      | Selects one of the 32 DX7-canonical algorithm graphs. Determines which ops are carriers, which modulate which others, and which op has the algorithm's feedback path. |

The algorithm is a *topology* param: changing it doesn't change op parameters,
only the wiring. Per-op level meanings (carrier amp vs modulator FM index)
change as a *consequence* of the new wiring, hence the UI re-colours op tabs
on algo change.

`feedback` (int, 0..7) sits next to the algo and drives the structural
feedback loop of whichever op the algorithm designates as its FB op.

---

## LFO 1 (global)

| Param          | Type | Range                          | Default | Purpose                                                                  |
|----------------|------|--------------------------------|---------|--------------------------------------------------------------------------|
| `lfo1_shape`   | e    | {Sine, Tri, Saw+, Saw−, Pulse, S&H} | Sine | Output waveform. Sine is the default for vibrato/tremolo; S&H for stepped textures. |
| `lfo1_rate`    | f    | 0.01 .. 50.0 Hz (or BPM-sync table) | 2.4 Hz | LFO frequency. When sync is on, snaps to host-tempo subdivisions (1/1, 1/2, ..., 1/64, dotted, triplet). |
| `lfo1_sync`    | b    | off / on                       | off     | When on, `lfo1_rate` snaps to BPM subdivisions and resets phase on transport restart. |

LFO 1 is shared across all voices — single phase accumulator, evaluated once
per control block. Per ADR §4: use for patch-wide effects (locked chorus,
song-synced sweeps).

LFO 1 has no global depth macro: per-route send level is the mod-matrix
slot depth column (a redundant `lfo1_depth` scaler was removed in E006 /
ticket 0061). LFO 1 enters the matrix at full bipolar scale.

---

## LFO 2 (per-voice)

| Param           | Type | Range                          | Default | Purpose                                                                  |
|-----------------|------|--------------------------------|---------|--------------------------------------------------------------------------|
| `lfo2_shape`    | e    | {Sine, Tri, Saw+, Saw−, Pulse, S&H} | Saw+ | Per-voice waveform. |
| `lfo2_rate`     | f    | 0.01 .. 50.0 Hz                | 5.1 Hz  | Per-voice frequency. Ignored when `lfo2_sync` is on. |
| `lfo2_sync`     | b    | { false, true }                | false   | Host-tempo sync. When on, the rate fader selects a musical subdivision instead of free-running Hz. |
| `lfo2_delay`    | f    | 0 .. 4000 ms                   | 180 ms  | Delay before the LFO begins after note-on (matches DX7 LFO delay). |
| `lfo2_fade`     | f    | 0 .. 4000 ms                   | 320 ms  | Fade-in time from end-of-delay to full depth. |

LFO 2 is the breathy / humanising LFO. Always key-triggered: every note-on
retriggers the lane phases to the shape's zero crossing and restarts
delay+fade from zero. Per ADR §4: each voice (and each stacked instance,
per ADR §3) has its own phase accumulator. Decorrelating phase across a
stack via `voice_rand → lfo2 phase` matrix routing is the "shimmer" trick.

---

## Pitch EG

| Param          | Type | Range          | Default | Purpose                                                                  |
|----------------|------|----------------|---------|--------------------------------------------------------------------------|
| `peg_r1..r4`   | i    | 0 .. 99        | varies  | Rates (matches per-op EG semantics). |
| `peg_l1..l4`   | i    | −99 .. +99     | 0,0,0,0 | Levels are *signed* — pitch can swing up or down. |
| `peg_depth`    | f    | 0.0 .. 1.0     | 1.0     | Overall depth multiplier into the pitch sum. |

Default routes to global pitch (additive). Mod matrix can route to any
pitch-shaped destination.

---

## Mod Env

| Param           | Type | Range          | Default | Purpose                                                                  |
|-----------------|------|----------------|---------|--------------------------------------------------------------------------|
| `mod_env_a`     | f    | 0 .. 4000 ms   | 2 ms    | Attack. |
| `mod_env_d`     | f    | 0 .. 4000 ms   | 320 ms  | Decay. |
| `mod_env_s`     | f    | 0.0 .. 1.0     | 0.60    | Sustain. |
| `mod_env_r`     | f    | 0 .. 4000 ms   | 180 ms  | Release. |
| `mod_env_shape` | e    | {Lin, Exp}     | Lin     | Segment shape. Exp = analog-style curve. |

General-purpose envelope. No default routing; matrix-only. Per-voice
retrigger on note-on.

---

## Assignment

| Param            | Type | Range                       | Default | Purpose                                                                  |
|------------------|------|-----------------------------|---------|--------------------------------------------------------------------------|
| `assign_mode`    | e    | {Poly, Solo}                | Poly    | Poly = up to 16 voices. Solo = monophonic. |
| `legato`         | b    | off / on                    | off     | Solo only: legato (no retrigger on overlapped notes). |
| `glide_time`     | f    | 0 .. 2000 ms                | 12 ms   | Portamento time between consecutive notes (always in Solo, optional in Poly per legato). |

Voice cap (16) is enforced by the allocator.

---

## Voice stacking

| Param         | Type | Range                       | Default | Purpose                                                                  |
|---------------|------|-----------------------------|---------|--------------------------------------------------------------------------|
| `stack_density` | i  | 1 .. 8                      | 4       | Number of concurrent op-voices per played note. Density 1 = no stacking. |
| `stack_detune`  | f  | 0 .. 100 cents              | 8       | Maximum detune across the stack (in cents). Outer instances detune by ±this; centre instance not detuned. |
| `stack_spread`  | f  | 0.0 .. 1.0                  | 0.60    | Stereo pan spread across the stack. 0 = mono. 1 = outer instances fully L/R. |
| `stack_phase`   | f  | 0.0 .. 1.0                  | 0.50    | Phase spread across the stack at note-on. 0 = all instances aligned. 1 = maximally decorrelated. |
| `stack_distrib` | e  | {Linear, Geometric, Random} | Linear  | How detune/pan distribute across stack instances. Linear = even, Geometric = exponential clustering, Random = each note-on randomises. |

These are macro convenience knobs. Per ADR §3 they write into matrix-style
routings using `voice_idx`, `voice_spread`, `voice_rand` as sources. Power
users can additionally route those sources via the matrix.

---

## Mod matrix

The matrix is a 16-slot table per patch. Each slot has:

| Field      | Type | Notes                                                                                            |
|------------|------|--------------------------------------------------------------------------------------------------|
| `source`   | e    | One of: `lfo1`, `lfo2`, `pitch_eg`, `mod_env`, `mod_wheel`, `aftertouch`, `velocity`, `key`, `voice_idx`, `voice_spread`, `voice_rand`. |
| `dest`     | e    | One of the routable destinations (see below).                                                    |
| `depth`    | f    | −1.0 .. +1.0 (normalised; multiplied by dest-specific range to get plain offset).                |
| `curve`    | e    | {lin, exp, log, bipolar}.                                                                        |

**Destinations** (v1 set):

- Per-op: `op{N}_ratio`, `op{N}_level`, `op{N}_detune`, `op{N}_pan` (6 ops × 4 dests = 24)
- Global: `global_pitch`, `lfo1_rate`, `lfo2_rate`, `lfo2_phase`
- Stacking macros (matrix can override): `stack_detune`, `stack_spread`
- FX: `delay_mix`, `reverb_mix`

**CLAP exposure**: slots **1–8 `depth`** are CLAP params
(`mtx1_depth` … `mtx8_depth` = 8 CLAP params). Slots 9–16 `depth` and
*all* slot `source` / `dest` / `curve` fields are patch state only, not
CLAP-automatable. Rationale: 16 slots × 4 fields = 64 params is mostly
meaningless to automate (source / dest are topology selectors), but users
do want a few automatable depths for expressive macros. 8 slots is the
compromise — enough for a couple of performance macros without bloating
the param table. UI cue: slots 1–8 render with an "automatable" badge so
users park their DAW-driven routings there.

### Replacing keyboard splits

If you want note-range-dependent timbre within one patch, the matrix
exposes `key` as a source with a curve — route `key` (bipolar curve to
centre at C4) into the destination you want to vary by note range. Splits
across two contrasting patches are host territory (DAW MIDI ranges /
track stacks) and no longer live inside the synth.

---

## Effects

### Delay (clean)

| Param            | Type | Range                                | Default | Purpose                                              |
|------------------|------|--------------------------------------|---------|------------------------------------------------------|
| `delay_on`       | b    | off / on                             | on      | Bypass toggle (header switch).                       |
| `delay_time`     | f    | 1 ms .. 4000 ms (or BPM-sync table) | 3/8    | Delay time. Sync table reuses VXN1's subdivisions.   |
| `delay_sync`     | b    | off / on                             | on      | Snap `delay_time` to BPM subdivisions.               |
| `delay_feedback` | f    | 0.0 .. 0.95                          | 0.45    | Feedback amount. Capped under 1.0 to prevent runaway. |
| `delay_mix`      | f    | 0.0 .. 1.0                           | 0.25    | Wet/dry mix.                                         |
| `delay_pingpong` | b    | off / on                             | off     | Ping-pong (alternating L/R taps).                    |

### Reverb (FDN)

| Param          | Type | Range          | Default | Purpose                                                  |
|----------------|------|----------------|---------|----------------------------------------------------------|
| `reverb_on`    | b    | off / on       | on      | Bypass toggle.                                           |
| `reverb_size`  | f    | 0.0 .. 1.0     | 0.55    | Maps to delay-line lengths in the FDN.                   |
| `reverb_decay` | f    | 0.1 .. 20.0 s  | 2.4 s   | RT60 target. Drives feedback matrix gain.                |
| `reverb_damp`  | f    | 0.0 .. 1.0     | 0.50    | High-frequency damping inside the FDN.                   |
| `reverb_mix`   | f    | 0.0 .. 1.0     | 0.20    | Wet/dry mix.                                             |

---

## Master

| Param           | Type | Range          | Default | Purpose                                              |
|-----------------|------|----------------|---------|------------------------------------------------------|
| `master_tune`   | f    | −100 .. +100 ct | 0      | Global tuning offset in cents.                       |
| `master_volume` | f    | −60 .. +6 dB   | −6 dB   | Master output gain.                                  |

(No limiter — explicitly dropped from scope per ADR §10 review. DAW can
limit if needed.)

---

## Parameter count summary

### Per-patch

| Section                       | Count                |
|-------------------------------|----------------------|
| Per-op (×6)                   | 20 × 6 = 120         |
| Algorithm + Feedback          | 2                    |
| LFO 2                         | 5                    |
| Pitch EG                      | 9                    |
| Mod Env                       | 5                    |
| Assignment                    | 3                    |
| Stacking                      | 5                    |
| Mod matrix slots 1–8 depth    | 8                    |
| **Per-patch subtotal**        | **157**              |

### Patch-level

| Section            | Count |
|--------------------|-------|
| LFO 1              | 3     |
| Delay              | 6     |
| Reverb             | 5     |
| Master             | 2     |
| **Patch-level subtotal** | **16** |

### CLAP totals

| Quantity                | Value          |
|-------------------------|----------------|
| Per-patch + patch       | 157 + 16 = **173** |
| Mod matrix non-CLAP fields | source + dest + curve × 16 slots + depth × slots 9–16 = 56 fields (patch sub-table, not CLAP) |

Mod matrix slot `source`, `dest`, `curve` are excluded from CLAP because
they're topology selectors (changing them mid-automation rewires routing,
not a useful continuous control). Slot `depth` is the modulatable quantity:
slots **1–8** depth is CLAP-automatable; slots 9–16 depth is patch state
only. Users park automation targets in slots 1–8 (UI flags them). Revisit
if 8 proves too few in practice.

Compares to DX7's ~155 (no per-op feedback, no second envelope class, no
voice stacking, no FX). The increment is intentional and load-bearing.
