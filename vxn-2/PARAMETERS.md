# VXN2 — Parameter reference

Generated alongside ADR 0001. The faceplate UI at `ui-mockup/index.html` is
the canonical layout reference; this document enumerates every parameter
surfaced by that UI and explains its role in the engine.

Notation:

- **Type** column: `i` = integer, `f` = float, `e` = enum (variants in
  brackets), `b` = bool.
- **Range** is the *plain* (user-facing) range. Normalisation to [0, 1] for
  CLAP and automation is parameter-specific (see `vxn2-engine` param
  descriptors when built).
- **Per-op** parameters repeat 6× with prefix `op1_`…`op6_`. Listed once.

## Scope: per-layer vs patch-level

Per ADR §8 / ticket 0009, a patch is one of three voicing modes
(Whole / Layer / Split). Layer + Split require two parameter sets (Upper,
Lower); Whole uses one. Each section below is tagged:

- **(per-layer)**: exists once per layer. Exposed via CLAP as both
  `upper_*` and `lower_*` regardless of `voicing_mode` (CLAP params can't
  appear/disappear at runtime). In Whole mode the engine drives all voices
  from the Upper set; Lower params are inert but still automatable.
- **(patch-level)**: single instance shared across layers. Exposed once.

This means every per-layer param costs two CLAP slots. The total CLAP-
exposed count at the bottom reflects this doubling.

---

## Per-operator (6×) *(per-layer)*

For each operator `op{1..6}`:

| Param           | Type | Range          | Default | Purpose                                                                                                                         |
|-----------------|------|----------------|---------|---------------------------------------------------------------------------------------------------------------------------------|
| `ratio_mode`    | e    | {Ratio, Fixed} | Ratio   | If `Ratio`, op frequency = note × `ratio`. If `Fixed`, op frequency = `fixed_hz` (Hz), independent of played note. Inharmonic + percussive sounds typically use Fixed for one op. |
| `ratio`         | f    | 0.50 .. 31.00  | 1.00    | Harmonic multiplier of the note frequency. Integer values give classic harmonic series; fractional ratios give inharmonic / metallic timbres. |
| `fixed_hz`      | f    | 1.0 .. 9772.0  | 440.0   | Absolute frequency when `ratio_mode = Fixed`. Range matches DX7 fixed-mode span (~5 decades). |
| `fine`          | f    | 0.00 .. 0.99   | 0.00    | Fine-tune fraction added to ratio (so effective ratio = `ratio + fine`). DX7-style fine. |
| `detune`        | i    | −7 .. +7       | 0       | Per-op detune in fixed steps (matches DX7's 15-value detune). Small enough not to track musical intervals; just thickens. |
| `level`         | i    | 0 .. 99        | 99 (carriers) / 0 (mods) | Operator output level. For carriers this is amp; for modulators this drives FM index. DX7's primary timbre control. |
| `vel_sens`      | i    | 0 .. 7         | 3       | Velocity sensitivity for `level`. Higher = louder/brighter on hard hits. DX7-style 0-7 scale. |
| `amp_sens`      | i    | 0 .. 3         | 0       | LFO amplitude-modulation sensitivity for this op (lets LFO tremolo apply per-op). |
| `eg_r1..r4`     | i    | 0 .. 99        | 99,50,35,60 | EG rates. R1 = attack speed, R2 = decay-to-sustain, R3 = sustain decay (slow drift), R4 = release. Higher = faster. |
| `eg_l1..l4`     | i    | 0 .. 99        | 99,70,50,0  | EG levels. L1 = peak after attack, L2 = decay target, L3 = sustain level, L4 = release floor (and pre-attack start). Carriers usually L4=0. |
| `ks_break_pt`   | i    | 0 .. 127       | 60 (C4) | Keyboard break point (MIDI note). At this note, key-scaling applies zero offset to `level`. |
| `ks_l_depth`    | i    | 0 .. 99        | 0       | Level scaling depth for notes *below* the break point. |
| `ks_r_depth`    | i    | 0 .. 99        | 30      | Level scaling depth for notes *above* the break point. |
| `ks_l_curve`    | e    | {+lin, −lin, +exp, −exp} | −lin | Shape of level scaling left of break point. `+` boosts, `−` cuts. Lin = linear, exp = exponential. |
| `ks_r_curve`    | e    | {+lin, −lin, +exp, −exp} | −exp | Shape of level scaling right of break point. |
| `ks_rate`       | i    | 0 .. 7         | 2       | Keyboard rate scaling. Speeds up EG rates as note pitch rises (mimics decay of plucked strings, etc.). Single value applies to all 4 EG rates. |
| `pan`           | f    | −1.0 .. +1.0   | 0.0     | Stereo pan for the op's output contribution (carriers contribute to the stereo bus; modulators' pan affects how their FM scattering manifests when carriers have differing pan). |
| `feedback`      | i    | 0 .. 7         | 0       | Per-op self-feedback amount. DX7 had one FB op per algo; VXN2 allows any op a feedback amount (VXN2 extension). Adds saw/noise character at high values. |

**Why so many per-op params**: FM sound design lives in modulator EG + KS
configuration. Hidden / merged controls make patches sound flat. Per the ADR,
the EG and KS graphs in the op-detail panel collapse R1..R4 + L1..L4 into one
draggable widget and the 5 KS params into another, so editing density on the
faceplate is manageable despite the parameter count.

---

## Algorithm *(per-layer)*

| Param  | Type | Range  | Default | Purpose                                                                       |
|--------|------|--------|---------|-------------------------------------------------------------------------------|
| `algo` | i    | 1 .. 32 | 5      | Selects one of the 32 DX7-canonical algorithm graphs. Determines which ops are carriers, which modulate which others, and which op has the algorithm's feedback path. |

The algorithm is a *topology* param: changing it doesn't change op parameters,
only the wiring. Per-op level meanings (carrier amp vs modulator FM index)
change as a *consequence* of the new wiring, hence the UI re-colours op tabs
on algo change.

---

## LFO 1 (global) *(patch-level)*

| Param          | Type | Range                          | Default | Purpose                                                                  |
|----------------|------|--------------------------------|---------|--------------------------------------------------------------------------|
| `lfo1_shape`   | e    | {Sine, Tri, Saw+, Saw−, Pulse, S&H} | Sine | Output waveform. Sine is the default for vibrato/tremolo; S&H for stepped textures. |
| `lfo1_rate`    | f    | 0.01 .. 50.0 Hz (or BPM-sync table) | 2.4 Hz | LFO frequency. When sync is on, snaps to host-tempo subdivisions (1/1, 1/2, ..., 1/64, dotted, triplet). |
| `lfo1_depth`   | f    | 0.0 .. 1.0                     | 0.30    | Overall depth scaler. Matrix slots routing FROM LFO1 multiply against this. Lets a single fader gate the entire LFO1 contribution. |
| `lfo1_sync`    | b    | off / on                       | off     | When on, `lfo1_rate` snaps to BPM subdivisions and resets phase on transport restart. |

LFO 1 is shared across all voices — single phase accumulator, evaluated once
per control block. Per ADR §4: use for patch-wide effects (locked chorus,
song-synced sweeps).

---

## LFO 2 (per-voice) *(per-layer)*

| Param           | Type | Range                          | Default | Purpose                                                                  |
|-----------------|------|--------------------------------|---------|--------------------------------------------------------------------------|
| `lfo2_shape`    | e    | {Sine, Tri, Saw+, Saw−, Pulse, S&H} | Saw+ | Per-voice waveform. |
| `lfo2_rate`     | f    | 0.01 .. 50.0 Hz                | 5.1 Hz  | Per-voice frequency. (No host sync — per-voice phases would diverge from grid anyway.) |
| `lfo2_delay`    | f    | 0 .. 4000 ms                   | 180 ms  | Delay before the LFO begins after note-on (matches DX7 LFO delay). |
| `lfo2_fade`     | f    | 0 .. 4000 ms                   | 320 ms  | Fade-in time from end-of-delay to full depth. |
| `lfo2_trig`     | e    | {Free, KeySync}                | Free    | Free: LFO2 keeps phase across notes (still per-voice instance). KeySync: phase resets to 0 at each note-on. |

LFO 2 is the breathy / humanising LFO. Per ADR §4: each voice (and each
stacked instance, per ADR §3) has its own phase accumulator. Decorrelating
phase across a stack via `voice_rand → lfo2 phase` matrix routing is the
"shimmer" trick.

---

## Pitch EG *(per-layer)*

| Param          | Type | Range          | Default | Purpose                                                                  |
|----------------|------|----------------|---------|--------------------------------------------------------------------------|
| `peg_r1..r4`   | i    | 0 .. 99        | varies  | Rates (matches per-op EG semantics). |
| `peg_l1..l4`   | i    | −99 .. +99     | 0,0,0,0 | Levels are *signed* — pitch can swing up or down. |
| `peg_depth`    | f    | 0.0 .. 1.0     | 1.0     | Overall depth multiplier into the pitch sum. |

Default routes to global pitch (additive). Mod matrix can route to any
pitch-shaped destination.

---

## Mod Env *(per-layer)*

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

## Assignment *(per-layer)*

| Param            | Type | Range                       | Default | Purpose                                                                  |
|------------------|------|-----------------------------|---------|--------------------------------------------------------------------------|
| `assign_mode`    | e    | {Poly, Solo}                | Poly    | Poly = up to 16 voices. Solo = monophonic. Per-layer so a Split can have mono bass + poly lead. |
| `legato`         | b    | off / on                    | off     | Solo only: legato (no retrigger on overlapped notes). |
| `glide_time`     | f    | 0 .. 2000 ms                | 12 ms   | Portamento time between consecutive notes (always in Solo, optional in Poly per legato). |

Voice cap (16) is a patch-level constraint enforced by the allocator across
both layers' assignments — see ticket 0009 AC.

---

## Voicing *(patch-level)*

| Param            | Type | Range                       | Default | Purpose                                                                  |
|------------------|------|-----------------------------|---------|--------------------------------------------------------------------------|
| `voicing_mode`   | e    | {Whole, Layer, Split}       | Layer   | Whole = single patch (Upper params drive engine). Layer = two parallel patches summed. Split = keyboard-split patches. |
| `split_point`    | i    | 0 .. 127 (MIDI)             | 60 (C4) | Note at/above which the Upper layer plays in Split mode. |
| `edit_layer`     | e    | {Upper, Lower}              | Upper   | Which layer the op-detail panel edits. Non-automatable view state — *not* a CLAP param. |

---

## Voice stacking *(per-layer)*

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

## Mod matrix *(per-layer)*

The matrix is a 16-slot table *per layer* (Upper + Lower each have their own
16 slots). Each slot has:

| Field      | Type | Notes                                                                                            |
|------------|------|--------------------------------------------------------------------------------------------------|
| `source`   | e    | One of: `lfo1`, `lfo2`, `pitch_eg`, `mod_env`, `mod_wheel`, `aftertouch`, `velocity`, `key`, `voice_idx`, `voice_spread`, `voice_rand`. |
| `dest`     | e    | One of the routable destinations (see below).                                                    |
| `depth`    | f    | −1.0 .. +1.0 (normalised; multiplied by dest-specific range to get plain offset).                |
| `curve`    | e    | {lin, exp, log, bipolar}.                                                                        |

**Destinations** (v1 set):

- Per-op: `op{N}_ratio`, `op{N}_level`, `op{N}_detune`, `op{N}_pan`, `op{N}_feedback` (6 ops × 5 dests = 30)
- Global: `global_pitch`, `lfo1_rate`, `lfo2_rate`, `lfo2_phase`
- Stacking macros (matrix can override): `stack_detune`, `stack_spread`
- FX: `delay_mix`, `reverb_mix`

**CLAP exposure**: slots **1–8 `depth`** are CLAP params per layer
(`upper_mtx1_depth` … `upper_mtx8_depth` + lower equivalents = 16 CLAP
params). Slots 9–16 `depth` and *all* slot `source` / `dest` / `curve`
fields are patch state only, not CLAP-automatable. Rationale: 16 slots × 4
fields × 2 layers = 128 params is mostly meaningless to automate (source /
dest are topology selectors), but users do want a few automatable depths
for expressive macros. 8 slots is the compromise — enough for a couple of
performance macros per layer without bloating the param table. UI cue:
slots 1–8 render with an "automatable" badge so users park their
DAW-driven routings there.

---

## Effects *(patch-level)*

Both layers feed the same FX chain — see ticket 0009 AC.

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

## Master *(patch-level)*

| Param           | Type | Range          | Default | Purpose                                              |
|-----------------|------|----------------|---------|------------------------------------------------------|
| `master_tune`   | f    | −100 .. +100 ct | 0      | Global tuning offset in cents.                       |
| `master_volume` | f    | −60 .. +6 dB   | −6 dB   | Master output gain.                                  |

(No limiter — explicitly dropped from scope per ADR §10 review. DAW can
limit if needed.)

---

## Parameter count summary

### Per-layer (×2 in CLAP — `upper_*` + `lower_*`)

| Section            | Count                |
|--------------------|----------------------|
| Per-op (×6)        | 21 × 6 = 126         |
| Algorithm          | 1                    |
| LFO 2              | 5                    |
| Pitch EG           | 9                    |
| Mod Env            | 5                    |
| Assignment         | 3                    |
| Stacking           | 5                    |
| Mod matrix slots 1–8 depth | 8            |
| **Per-layer subtotal** | **162**          |

### Patch-level (×1 in CLAP)

| Section            | Count |
|--------------------|-------|
| LFO 1              | 4     |
| Voicing            | 2 (`edit_layer` is view state, not CLAP) |
| Delay              | 6     |
| Reverb             | 5     |
| Master             | 2     |
| **Patch-level subtotal** | **19** |

### CLAP totals

| Quantity                | Value          |
|-------------------------|----------------|
| Per-layer × 2 + patch   | 2 × 162 + 19 = **343** |
| Mod matrix non-CLAP fields (per layer) | source + dest + curve × 16 slots + depth × slots 9–16 = 56 fields × 2 layers = 112 fields (patch sub-table, not CLAP) |

Every per-layer param is exposed under both `upper_` and `lower_` prefixes
regardless of `voicing_mode`. CLAP host param lists are static; the engine
gates which set drives voices according to `voicing_mode` (Whole = Upper
only; Layer = both summed; Split = Upper above `split_point`, Lower below).
Lower params remain automatable in Whole mode even though they're inert —
this keeps the param table stable across mode changes mid-session.

Mod matrix slot `source`, `dest`, `curve` are excluded from CLAP because
they're topology selectors (changing them mid-automation rewires routing,
not a useful continuous control). Slot `depth` is the modulatable quantity:
slots **1–8** depth is CLAP-automatable per layer; slots 9–16 depth is
patch state only. Users park automation targets in slots 1–8 (UI flags
them). Revisit if 8 proves too few in practice.

Compares to DX7's ~155 (no per-op feedback, no second envelope class, no
voice stacking, no FX, no layers). The increment is intentional and
load-bearing.
