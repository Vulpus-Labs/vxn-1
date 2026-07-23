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
| `phase`         | f    | 0.0 .. 1.0     | 0.0     | Per-op note-on phase offset as a fraction of one cycle (1.0 = 2π; cyclic, so 1.0 ≡ 0.0). Composes additively with the per-lane stack-phase decorrelation. A lone steady carrier is phase-deaf (magnitude spectrum unchanged), so this is inaudible in isolation — it matters for (a) the time-domain shape of additive sums on algo 32 (a saw flips even harmonics by 0.5 = π), (b) an op used as an FM modulator (the carrier sees the shifted phase), and (c) the attack transient. **Stack-path only** — the scalar reference/bench path does not reset phase at note-on, so the offset would wash out there. |

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
| `scale_src`| e    | Optional **secondary scale source** (E033), same roster as `source`; `none` (default) = depth unscaled. |

**Secondary scale source** (E033 / ADR 0009). Beyond the additive
`source → dest` routing, each slot has an optional `scale_src` that
*multiplicatively gates* the slot's depth — a VCA on the route. The route's
per-lane contribution becomes `source · curve(depth) · scale_norm(scale_src)`,
so e.g. `lfo1 → global_pitch` with `scale_src = mod_wheel` is a vibrato whose
depth follows the mod wheel (0 at wheel down, full at wheel up — the classic
DX7 mod-wheel-vibrato; see the *EP Wheel Vibrato* factory preset). The scale
source is normalised to `[0, 1]`:

| Scale source polarity | Sources | `scale_norm(x)` |
| --- | --- | --- |
| unipolar (already `[0, 1]`) | `mod_wheel`, `aftertouch`, `velocity`, `key`, `mod_env`, `voice_idx`, `voice_rand` | `x` (passthrough) |
| bipolar (`[-1, 1]`) | `lfo1`, `lfo2`, `pitch_eg`, `voice_spread` | `(x + 1) · 0.5` |

Both clamped to `[0, 1]`. `scale_src = none` is exact identity (multiply by
`1.0`), so an unscaled patch renders bit-identically to a pre-E033 engine.
`scale_src` is patch topology (like `source`/`dest`/`curve`) — **not** a new
CLAP-automatable param.

**Destinations** (29 total):

- Per-op: `op{N}_pitch`, `op{N}_level`, `op{N}_pan` (6 ops × 3 dests = 18; v3
  collapsed the old Ratio + Detune dests into one Pitch dest)
- Global: `global_pitch`, `lfo1_rate`, `lfo2_rate`, `lfo2_phase`
- Stacking macros (matrix can override): `stack_detune`, `stack_spread`
- FX: `delay_mix`, `reverb_mix`
- Feedback: `feedback`
- Filter (E007 / ADR 0004): `cutoff`, `resonance` — per-voice, collapse to a
  per-stack scalar (lane-0). `cutoff` modulates in the log/octave domain
  (gain 8 octaves at full depth); `resonance` is an additive `[0, 1]` offset.
  Inert until `filter-enable` is on.

**Granularity tiers & coherence** (E008): every source and dest has a
granularity tier — how many independent values it carries:

| Tier | Sources | Destinations |
| --- | --- | --- |
| **patch-global** (1/patch) | `lfo1`, `mod_wheel`, `aftertouch` | `lfo1_rate`, `delay_mix`, `reverb_mix` |
| **per-stack** (1/voice) | `pitch_eg`, `mod_env`, `velocity`, `key` | `lfo2_rate`, `stack_detune`, `stack_spread`, `cutoff`, `resonance` |
| **per-lane** (1/unison lane) | `lfo2`, `voice_idx`, `voice_spread`, `voice_rand` | `op{N}_{pitch,level,pan}`, `global_pitch`, `feedback`, `lfo2_phase` |

A routing is **coherent** iff the source tier is coarser-or-equal to the dest
tier: a coarser source broadcasts unambiguously to a finer dest; a finer
source into a coarser dest is a lossy collapse to lane 0 (which lane wins?).
The matrix UI renders incoherent routings in red with a tooltip but still
lets them be set (old patches load). Two special cases on top of the tier
rule: an LFO into **its own** rate (`lfo1→lfo1_rate`, `lfo2→lfo2_rate`) is
self-referential; `voice_idx` into a lane-0-collapsed dest (`cutoff`,
`resonance`, `delay_mix`, `reverb_mix`) is degenerate (`voice_idx[0]` is
always 0 → constant, no effect). The predicate (`matrix::coherence`) is the
single source of truth, exported in the matrix descriptor the UI reads.

**Units & depth full-scale** (E008 0094): every source emits a normalized
shape — bipolar `[-1, 1]` (`lfo1`, `lfo2`, `voice_spread`, **`pitch_eg`**) or
unipolar `[0, 1]` (`mod_wheel`, `aftertouch`, `velocity`, `key`, `mod_env`,
`voice_idx`, `voice_rand`). `pitch_eg` is the EG *shape* (`level_st /
peg_depth`), **not** raw semitones — so the pitch dest's ±24 st gain sets the
excursion and there is no hidden 24× re-scale (the old double-scale bug). Each
dest's gain converts `depth × shape` to its native unit at `depth = 1`:

| Dest | `depth = 1` full-scale |
| --- | --- |
| `op{N}_pitch`, `global_pitch` | ±24 st (±2 oct) |
| `op{N}_level` | full multiplicative tremolo on the EG |
| `op{N}_pan` | hard L↔R |
| `feedback` | the 0..7 feedback range |
| `cutoff`, `lfo1_rate`, `lfo2_rate` | ±4 octaves (log domain) |
| `resonance`, `delay_mix`, `reverb_mix` | additive `[0, 1]` offset |
| `stack_detune`, `stack_spread` | scales the macro by `(1 + v)` (0→2×) |
| `lfo2_phase` | ±1 full LFO2 cycle (per-lane offset) |

**Depth taper**: the 7 semitone pitch dests (`global_pitch`, `op{N}_pitch`)
apply a cubic taper (`d³`) to the stored depth before the ±24 st gain, so
vibrato-scale amounts (≤ 0.5 st) occupy usable widget travel (25% ≈ ±0.4 st,
50% ≈ ±3 st, 100% = ±24 st). The stored / CLAP value stays linear; the engine
cooks the taper at block rate. All other dests stay **linear** — the log-domain
rate/cutoff gains and the `(1 + v)`-scale stack macros are already shaped, so a
taper would double-bend them (0094 decision).

**Migration** (0094): patches routing `pitch_eg → *_pitch` change audibly —
the old path swung `peg_depth × 24 ×` the EG; the fix swings `±24 st ×` the EG
*shape* at depth 1. Re-scale such slots' depths to restore the authored
excursion (the factory re-audit is [ticket 0097]). No blob-version bump:
depths are stored normalized and unchanged — only the runtime source
interpretation shifted.

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

The FX bus order is **`cleanup → dynamics → phaser → delay → reverb → master gain → limiter`**.
Dynamics is first so the comp evens FM transients before delay regen and reverb
tail accumulate them; phaser sits between dynamics and the time FX; the master
brickwall limiter remains last as a safety stage.

Each block-level FX (Dynamics, Phaser, Delay, Reverb) bypasses to a bit-exact
passthrough when its `*_on` toggle is off — `set_enabled(false)` first fades a
wet-mix smoother to 0 (no click on switch-off), and only then reverts to the
zero-cost passthrough. None of the FX-block params is a mod-matrix destination
(host-automation only).

### Dynamics

Stereo feed-forward peak compressor → tanh saturator, channel-strip topology
(comp first so the saturator drives consistent harmonic content). Soft-knee
(6 dB internal width), one log2 + one exp2 per active sample. **First in the
FX bus.**

| Param           | Type | Range            | Default | Purpose                                              |
|-----------------|------|------------------|---------|------------------------------------------------------|
| `dyn-on`        | b    | off / on         | off     | Bypass toggle. Off ⇒ bit-exact passthrough.          |
| `dyn-threshold` | f    | −60 .. 0 dB      | −12 dB  | Compressor threshold.                                |
| `dyn-ratio`     | f    | 1 .. 20          | 4       | Compression ratio (1 = no compression).              |
| `dyn-attack`    | f    | 0.1 .. 200 ms    | 10 ms   | Peak detector attack time.                           |
| `dyn-release`   | f    | 5 .. 1000 ms     | 100 ms  | Peak detector release time.                          |
| `dyn-makeup`    | f    | 0 .. 24 dB       | 0 dB    | Post-comp, pre-sat linear makeup gain.               |
| `dyn-drive`     | f    | 0 .. 36 dB       | 0 dB    | Saturator input drive. 0 ⇒ identity (no harmonics).  |
| `dyn-mix`       | f    | 0.0 .. 1.0       | 1.0     | Dry/wet on the comp + sat chain.                     |

### Phaser

Stereo allpass phaser, four cascaded all-pass sections per channel with an
anti-phase L/R triangle LFO sweep around 600 Hz. Macro surface only; stages,
centre frequency, and stereo spread are pinned internally.

| Param             | Type | Range            | Default | Purpose                                              |
|-------------------|------|------------------|---------|------------------------------------------------------|
| `phaser-on`       | b    | off / on         | off     | Bypass toggle. Off ⇒ bit-exact passthrough.          |
| `phaser-rate`     | f    | 0.05 .. 8 Hz     | 0.4 Hz  | LFO rate.                                            |
| `phaser-depth`    | f    | 0.0 .. 1.0       | 0.6     | Sweep depth (±2 oct around 600 Hz at depth = 1).     |
| `phaser-feedback` | f    | −0.9 .. 0.9      | 0.3     | Feedback through a soft-clipped path.                |
| `phaser-mix`      | f    | 0.0 .. 1.0       | 0.5     | Dry/wet (wet gets a mild mid-mix makeup curve).      |

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

## Filter (optional, per-voice — E007 / ADR 0004)

An optional per-voice oversampled OTA-C ladder, post-stack-sum / pre-voice-sum.
**Off by default**: with `filter_enable` off the render path is the unchanged
sample-major loop and output is bit-identical to a filterless patch. `cutoff`
and `resonance` are mod-matrix destinations (`Cutoff` / `Resonance`).

| Param               | Type | Range            | Default | Purpose                                                        |
|---------------------|------|------------------|---------|----------------------------------------------------------------|
| `filter_enable`     | b    | off / on         | off     | Master toggle. Off ⇒ zero added cost, bit-identical output.    |
| `filter_cutoff`     | f    | 20 .. 20000 Hz   | 12000   | Corner frequency, exp taper. Matrix dest `Cutoff` (log domain).|
| `filter_resonance`  | f    | 0.0 .. 1.0       | 0.0     | Feedback amount; self-oscillates at 1.0. Matrix dest `Resonance`.|
| `filter_mode`       | e    | LP / HP / BP / Notch | LP  | Response tap-mix.                                              |
| `filter_slope`      | e    | 2-Pole / 4-Pole  | 4-Pole  | 12 vs 24 dB/oct.                                               |
| `filter_drive`      | f    | 0.1 .. 16.0      | 1.0     | Pre-`tanh` input drive into stage 0.                          |
| `filter_oversample` | e    | 1× / 2× / 4× / 8× | 4×     | Oversample factor localised to the filter.                    |

`filter_enable`, `filter_mode`, `filter_slope`, `filter_oversample` are
structural selectors — like `delay_on`/`algo`/`lfo2_shape` they are CLAP
params (the codebase has no non-automatable flag) but reconfigure topology
rather than sweeping. `cutoff`/`resonance`/`drive` are continuous.

---

## Parameter count summary

### Per-patch

| Section                       | Count                |
|-------------------------------|----------------------|
| Per-op (×6)                   | 21 × 6 = 126         |
| Algorithm + Feedback          | 2                    |
| LFO 2                         | 5                    |
| Pitch EG                      | 9                    |
| Mod Env                       | 5                    |
| Assignment                    | 3                    |
| Stacking                      | 5                    |
| Mod matrix slots 1–8 depth    | 8                    |
| **Per-patch subtotal**        | **163**              |

### Patch-level

| Section            | Count |
|--------------------|-------|
| LFO 1              | 3     |
| Delay              | 6     |
| Reverb             | 5     |
| Master             | 2     |
| Filter (E007)      | 7     |
| **Patch-level subtotal** | **23** |

### CLAP totals

| Quantity                | Value          |
|-------------------------|----------------|
| Per-patch + patch       | 163 + 23 = **186** |
| Mod matrix non-CLAP fields | source + dest + curve × 16 slots + depth × slots 9–16 = 56 fields (patch sub-table, not CLAP) |

Mod matrix slot `source`, `dest`, `curve` are excluded from CLAP because
they're topology selectors (changing them mid-automation rewires routing,
not a useful continuous control). Slot `depth` is the modulatable quantity:
slots **1–8** depth is CLAP-automatable; slots 9–16 depth is patch state
only. Users park automation targets in slots 1–8 (UI flags them). Revisit
if 8 proves too few in practice.

Compares to DX7's ~155 (no per-op feedback, no second envelope class, no
voice stacking, no FX). The increment is intentional and load-bearing.
