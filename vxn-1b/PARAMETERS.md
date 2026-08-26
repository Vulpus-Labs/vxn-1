# VXN1b — Parameter reference

**Generated** from the `vxn1b-engine` param table — do not edit by hand. Regenerate with:

```sh
cargo run --release --example gen_parameters_doc -p vxn1b-engine \
  > vxn-1b/PARAMETERS.md
```

The host sees **185 parameters**: 75 patch params per layer × 2, plus 35 globals shared by both layers. A Layer 1 control and its Layer 2 twin are separate automation targets; Layer 2's CLAP id is its Layer 1 id + 75.

Columns:

- **CLAP id** — the automation id. For patch params this is the Layer 1 id.
- **Name** — the preset key, and the `data-param` the faceplate binds to.
- **Type** — `f` float, `i` integer, `b` bool, `e` enum (variants listed).
- **Range** — the plain, user-facing range. CLAP normalisation to [0, 1] is linear across it; a *taper* (noted where present) warps only where the fader sits, not the host range.

---

## Patch parameters (per layer)

Each of these exists twice — once for Layer 1, once for Layer 2 — with independent values and independent automation.

### Oscillators, sub and noise

| CLAP id | Name | Label | Type | Range | Default |
|--------:|------|-------|------|-------|---------|
| 0 | `osc1_wave` | Osc 1 Wave | e {Sine, Triangle, Saw, Pulse} | 4 variants | `Saw` |
| 1 | `osc1_coarse` | Osc 1 Coarse | i | -7 .. 7 st | 0 st |
| 2 | `osc1_fine` | Osc 1 Fine | f | -50 .. 50 ct | 0 ct |
| 3 | `osc1_octave` | Osc 1 Octave | i | -4 .. 4 oct | 0 oct |
| 4 | `osc1_level` | Osc 1 Level | f | 0 .. 1 | 0.8 |
| 5 | `osc1_pw` | Osc 1 PW | f | 0.05 .. 0.95 | 0.5 |
| 6 | `osc1_free_run` | Osc 1 Free | b | off / on | off |
| 7 | `osc2_wave` | Osc 2 Wave | e {Sine, Triangle, Saw, Pulse} | 4 variants | `Saw` |
| 8 | `osc2_coarse` | Osc 2 Coarse | i | -7 .. 7 st | 0 st |
| 9 | `osc2_fine` | Osc 2 Fine | f | -50 .. 50 ct | 0 ct |
| 10 | `osc2_octave` | Osc 2 Octave | i | -4 .. 4 oct | -1 oct |
| 11 | `osc2_level` | Osc 2 Level | f | 0 .. 1 | 0.6 |
| 12 | `osc2_pw` | Osc 2 PW | f | 0.05 .. 0.95 | 0.5 |
| 13 | `osc2_free_run` | Osc 2 Free | b | off / on | off |
| 14 | `sub_level` | Sub Level | f | 0 .. 1 | 0 |
| 15 | `cross_mod_type` | Cross Mod | e {Off, Sync, FM, Ring} | 4 variants | `Off` |
| 16 | `cross_mod_amount` | Cross Mod Amt | f | 0 .. 4 | 0 |
| 17 | `noise_level` | Noise Level | f | 0 .. 1 | 0 |
| 18 | `noise_color` | Noise Colour | e {White, Pink} | 2 variants | `White` |

### Filter

| CLAP id | Name | Label | Type | Range | Default |
|--------:|------|-------|------|-------|---------|
| 19 | `cutoff` | Cutoff | f | 16.3516 .. 16000 Hz *(exp, mid 800)* | 1000 Hz |
| 20 | `resonance` | Resonance | f | 0 .. 1 | 0.2 |
| 21 | `drive` | Drive | f | 0.1 .. 4 *(exp, mid 1)* | 1 |
| 22 | `filter_mode` | Filter Mode | e {LP, HP, BP, Notch} | 4 variants | `LP` |
| 23 | `filter_slope` | Filter Slope | e {12, 24} | 2 variants | `24` |
| 24 | `hpf_cutoff` | HPF Cutoff | f | 20 .. 18000 Hz *(exp, mid 1000)* | 20 Hz |
| 25 | `filter_key_track` | Key Track | f | 0 .. 1 | 0 |
| 26 | `cutoff_tuned` | Tuned | b | off / on | off |

### Envelopes

| CLAP id | Name | Label | Type | Range | Default |
|--------:|------|-------|------|-------|---------|
| 27 | `env1_attack` | Env 1 Attack | f | 0.001 .. 10 s *(exp, mid 1)* | 0.005 s |
| 28 | `env1_decay` | Env 1 Decay | f | 0.001 .. 10 s *(exp, mid 1)* | 0.3 s |
| 29 | `env1_sustain` | Env 1 Sustain | f | 0 .. 1 | 0 |
| 30 | `env1_release` | Env 1 Release | f | 0.001 .. 10 s *(exp, mid 1)* | 0.3 s |
| 31 | `env1_shape` | Env 1 Shape | e {Lin, Exp} | 2 variants | `Lin` |
| 32 | `env2_attack` | Env 2 Attack | f | 0.001 .. 10 s *(exp, mid 1)* | 0.005 s |
| 33 | `env2_decay` | Env 2 Decay | f | 0.001 .. 10 s *(exp, mid 1)* | 0.2 s |
| 34 | `env2_sustain` | Env 2 Sustain | f | 0 .. 1 | 0.8 |
| 35 | `env2_release` | Env 2 Release | f | 0.001 .. 10 s *(exp, mid 1)* | 0.3 s |
| 36 | `env2_shape` | Env 2 Shape | e {Lin, Exp} | 2 variants | `Exp` |

### Amp

| CLAP id | Name | Label | Type | Range | Default |
|--------:|------|-------|------|-------|---------|
| 37 | `amp_env_bypass` | Amp Gate | b | off / on | off |

### Layer mix

| CLAP id | Name | Label | Type | Range | Default |
|--------:|------|-------|------|-------|---------|
| 38 | `layer_level` | Layer Level | f | 0 .. 1 | 1 |
| 39 | `layer_mute` | Layer Mute | b | off / on | off |
| 40 | `layer_pan` | Layer Pan | f | -1 .. 1 | 0 |
| 41 | `layer_detune` | Layer Detune | f | -50 .. 50 ct *(bipolar exp, mid ±20)* | 0 ct |

### LFO 1

| CLAP id | Name | Label | Type | Range | Default |
|--------:|------|-------|------|-------|---------|
| 42 | `lfo1_shape` | LFO 1 Shape | e {Sine, Tri, Saw+, Saw-, Square, S&H} | 6 variants | `Sine` |
| 43 | `lfo1_rate` | LFO 1 Rate | f | 0.01 .. 40 Hz *(exp, mid 5)* | 5 Hz |
| 44 | `lfo1_sync` | LFO 1 Sync | b | off / on | off |
| 45 | `lfo1_delay_time` | LFO 1 Delay | f | 0 .. 4 s | 0 s |
| 46 | `lfo1_fade` | LFO 1 Fade | f | 0 .. 4 s | 0 s |
| 47 | `lfo1_free_run` | LFO 1 Free | b | off / on | off |

### LFO 2

| CLAP id | Name | Label | Type | Range | Default |
|--------:|------|-------|------|-------|---------|
| 48 | `lfo2_shape` | LFO 2 Shape | e {Sine, Tri, Saw+, Saw-, Square, S&H} | 6 variants | `Sine` |
| 49 | `lfo2_rate` | LFO 2 Rate | f | 0.01 .. 40 Hz *(exp, mid 5)* | 5 Hz |
| 50 | `lfo2_sync` | LFO 2 Sync | b | off / on | off |

### Voice

| CLAP id | Name | Label | Type | Range | Default |
|--------:|------|-------|------|-------|---------|
| 51 | `stack_width` | Width | e {1, 2, 4, 8, 16, 32} | 6 variants | `1` |
| 52 | `voice_mode` | Voice | e {Poly, Solo} | 2 variants | `Poly` |
| 53 | `legato` | Legato | b | off / on | off |
| 54 | `unison_detune` | Detune | f | 0 .. 50 ct | 12 ct |
| 55 | `portamento_time` | Glide Time | f | 0 .. 0.5 s *(exp, mid 0.1)* | 0 s |
| 56 | `spread` | Spread | f | 0 .. 1 | 0 |
| 57 | `stack_phase` | Phase | f | 0 .. 1 | 1 |
| 58 | `stack_distrib` | Distrib | e {Lin, Geo, Rnd} | 3 variants | `Lin` |

### Mod-matrix depths

| CLAP id | Name | Label | Type | Range | Default |
|--------:|------|-------|------|-------|---------|
| 59 | `matrix_slot0_depth` | Slot 1 Depth | f | -1 .. 1 | 0 |
| 60 | `matrix_slot1_depth` | Slot 2 Depth | f | -1 .. 1 | 0 |
| 61 | `matrix_slot2_depth` | Slot 3 Depth | f | -1 .. 1 | 0 |
| 62 | `matrix_slot3_depth` | Slot 4 Depth | f | -1 .. 1 | 0 |
| 63 | `matrix_slot4_depth` | Slot 5 Depth | f | -1 .. 1 | 0 |
| 64 | `matrix_slot5_depth` | Slot 6 Depth | f | -1 .. 1 | 0 |
| 65 | `matrix_slot6_depth` | Slot 7 Depth | f | -1 .. 1 | 0 |
| 66 | `matrix_slot7_depth` | Slot 8 Depth | f | -1 .. 1 | 0 |
| 67 | `matrix_slot8_depth` | Slot 9 Depth | f | -1 .. 1 | 0 |
| 68 | `matrix_slot9_depth` | Slot 10 Depth | f | -1 .. 1 | 0 |
| 69 | `matrix_slot10_depth` | Slot 11 Depth | f | -1 .. 1 | 0 |
| 70 | `matrix_slot11_depth` | Slot 12 Depth | f | -1 .. 1 | 0 |
| 71 | `matrix_slot12_depth` | Slot 13 Depth | f | -1 .. 1 | 0 |
| 72 | `matrix_slot13_depth` | Slot 14 Depth | f | -1 .. 1 | 0 |
| 73 | `matrix_slot14_depth` | Slot 15 Depth | f | -1 .. 1 | 0 |
| 74 | `matrix_slot15_depth` | Slot 16 Depth | f | -1 .. 1 | 0 |

## Global parameters

One instance each, applied to both layers: tuning, master level, the limiter, oversampling and the whole FX chain.

### Pitch bend

| CLAP id | Name | Label | Type | Range | Default |
|--------:|------|-------|------|-------|---------|
| 150 | `pitch_bend_range` | Bend Range | f | 0 .. 12 st | 2 st |

### Master

| CLAP id | Name | Label | Type | Range | Default |
|--------:|------|-------|------|-------|---------|
| 151 | `master_tune` | Master Tune | f | -12 .. 12 st | 0 st |
| 152 | `master_volume` | Volume | f | 0 .. 1 | 0.7 |
| 153 | `master_drift` | Drift | f | 0 .. 1 | 0 |
| 154 | `limiter_on` | Limiter | b | off / on | off |
| 155 | `oversample` | Oversample | e {O/S OFF, 2x, 4x, 8x} | 4 variants | `2x` |

### Chorus

| CLAP id | Name | Label | Type | Range | Default |
|--------:|------|-------|------|-------|---------|
| 156 | `chorus_on` | Chorus | b | off / on | off |
| 157 | `chorus_rate` | Chorus Rate | f | 0.05 .. 8 Hz | 0.6 Hz |
| 158 | `chorus_depth` | Chorus Depth | f | 0 .. 1 | 0.5 |
| 159 | `chorus_mix` | Chorus Mix | f | 0 .. 1 | 0.4 |

### Phaser

| CLAP id | Name | Label | Type | Range | Default |
|--------:|------|-------|------|-------|---------|
| 160 | `phaser_on` | Phaser | b | off / on | off |
| 161 | `phaser_rate` | Phaser Rate | f | 0.05 .. 10 Hz *(exp, mid 1)* | 0.5 Hz |
| 162 | `phaser_depth` | Phaser Depth | f | 0 .. 1 | 0.7 |
| 163 | `phaser_fb` | Phaser FB | f | -0.9 .. 0.9 | 0 |
| 164 | `phaser_mix` | Phaser Mix | f | 0 .. 1 | 0.5 |
| 165 | `phaser_stereo` | Phaser Stereo | f | 0 .. 180 ° | 180 ° |

### Delay

| CLAP id | Name | Label | Type | Range | Default |
|--------:|------|-------|------|-------|---------|
| 166 | `delay_on` | Delay | b | off / on | off |
| 167 | `delay_time` | Delay Time | f | 0.01 .. 2 s | 0.35 s |
| 168 | `delay_feedback` | Delay FB | f | 0 .. 0.95 | 0.4 |
| 169 | `delay_mix` | Delay Mix | f | 0 .. 1 | 0.25 |
| 170 | `delay_sync` | Delay Sync | b | off / on | off |
| 171 | `delay_pingpong` | Ping-Pong | b | off / on | on |

### Reverb

| CLAP id | Name | Label | Type | Range | Default |
|--------:|------|-------|------|-------|---------|
| 172 | `reverb_on` | Reverb | b | off / on | off |
| 173 | `reverb_size` | Reverb Size | f | 0 .. 1 | 0.5 |
| 174 | `reverb_decay` | Reverb Decay | f | 0.2 .. 10 s *(exp, mid 2)* | 2.5 s |
| 175 | `reverb_damp` | Reverb Damp | f | 0 .. 1 | 0.4 |
| 176 | `reverb_mix` | Reverb Mix | f | 0 .. 1 | 0.3 |

### Dynamics

| CLAP id | Name | Label | Type | Range | Default |
|--------:|------|-------|------|-------|---------|
| 177 | `dynamics_on` | Dynamics | b | off / on | off |
| 178 | `dynamics_threshold` | Dyn Threshold | f | -60 .. 0 dB | -12 dB |
| 179 | `dynamics_ratio` | Dyn Ratio | f | 1 .. 20 | 4 |
| 180 | `dynamics_attack` | Dyn Attack | f | 0.1 .. 200 ms *(exp, mid 10)* | 10 ms |
| 181 | `dynamics_release` | Dyn Release | f | 5 .. 1000 ms *(exp, mid 100)* | 100 ms |
| 182 | `dynamics_makeup` | Dyn Makeup | f | 0 .. 24 dB | 0 dB |
| 183 | `dynamics_drive` | Dyn Drive | f | 0 .. 36 dB | 0 dB |
| 184 | `dynamics_mix` | Dyn Mix | f | 0 .. 1 | 1 |

## Mod matrix

16 slots per layer. A slot is a `source → destination` pair with a depth, a curve, and a secondary *scale* source acting as a per-route VCA.

Only the **depths** are host parameters (listed above). The topology — which source feeds which destination, through which curve — lives in the patch state and is edited in the faceplate's matrix overlay. That split is what lets a depth be automated without the routing changing underneath it.

### Sources

| Label | Wire name | Polarity |
|-------|-----------|----------|
| Env 1 | `env1` | unipolar |
| Env 2 | `env2` | unipolar |
| LFO 1 | `lfo1` | bipolar |
| LFO 2 | `lfo2` | bipolar |
| Velocity | `velocity` | unipolar |
| Key | `key` | unipolar |
| Mod Wheel | `mod-wheel` | unipolar |
| Pitch Wheel | `pitch-wheel` | bipolar |
| Aftertouch | `aftertouch` | unipolar |
| Note Rnd | `note-random` | unipolar |
| Spread | `spread` | bipolar |
| Stack Pos | `stack-pos` | bipolar |

### Destinations

| Label | Wire name |
|-------|-----------|
| Pitch | `pitch` |
| Cross Mod Sweep | `xmod-sweep` |
| PWM (Both) | `pwm` |
| Cutoff | `cutoff` |
| Resonance | `resonance` |
| HPF Cutoff | `hpf-cutoff` |
| Amp | `amp` |
| Cross Mod Amt | `cross-mod-amount` |
| Pan | `pan` |
| Osc 1 PWM | `osc1-pwm` |
| Osc 2 PWM | `osc2-pwm` |
| Env 1 Scale | `env1-scale` |
| Env 2 Scale | `env2-scale` |
| LFO 1 Rate | `lfo1-rate` |
| Env 1 Sustain | `env1-sustain` |
| Env 2 Sustain | `env2-sustain` |

### Curves

Lin, Exp, Log, Bipolar

