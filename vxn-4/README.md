# vxn-4 — sizing

Synth #4: eight phase-modulation operators, every operator routed to every
operator including itself, plus a panned output per operator into a stereo sum
bus. No filter.

No plugin and no faceplate yet. There is enough engine to **hear it**: voice
allocation, envelopes, five hardwired patches, and the oversampling chain down
through a limiter, rendered offline to WAV.

## Crates

| crate | what it is |
|---|---|
| [`vxn4-dsp`](crates/vxn4-dsp) | band-limited mip-mapped wavetables, and the 8-operator block in two lane-loop layouts |
| [`vxn4-engine`](crates/vxn4-engine) | voice allocation, envelopes, patches, rate plan, limiter |
| [`vxn4-render`](crates/vxn4-render) | offline renderer — plays a note sequence into a WAV |
| [`vxn4-op-bench`](crates/vxn4-op-bench) | the sizing sweep and its criterion counterpart |

```sh
# listen
cargo run --release -p vxn4-render -- --all            # 25 WAVs into ./vxn4-out/
cargo run --release -p vxn4-render -- --patch 2 --seq scale --os 16 bell.wav

# measure
cargo run --release -p vxn4-op-bench --bin sweep       # the decision
cargo bench -p vxn4-op-bench                           # confidence intervals
```

**Build with `rustup run 1.95.0 cargo ...`** — see *Measurement* below for why
plain `cargo` is the wrong compiler on this machine.

## Playing it

Five patches, graded by routing density, and five sequences each chosen to put
one question in front of your ears.

| patch | routes | what it is for |
|---|---|---|
| `sine` | 0 | reference tone. One operator, nothing modulating — really a test of the decimator |
| `epiano` | 4 | two 2-op stacks, the FM idiom |
| `bell` | 6 | inharmonic ratios plus self-feedback |
| `saws` | 11 | assignable waveforms — the thing a DX7 cannot do, and the hardest case for the mips |
| `web` | 64 | every route live; the worst case the sizing bench quotes against |
| `grind` | 4 | **saw modulating saw at high index** — the aliasing torture case, deliberately not musical |

| sequence | what it asks |
|---|---|
| `chord` | does this patch sound like anything? |
| `scale` | five octaves chromatic — mip transitions |
| `arp` | fast onsets; envelope retrigger and lane reuse |
| `steal` | 24 notes against a 16-voice cap. Notes should vanish; nothing should click |
| `vel` | rising velocity into the envelope ceiling |
| `high` | sustained notes at the top of the keyboard — **the 8x/16x A/B** |

Patch gains are set from measurement so a six-note chord lands near -6 dBFS on
every patch. That matches them for loudness — so an A/B is about timbre, not
level — and keeps ordinary playing clear of the limiter, which matters for the
reason in *The limiter placement is a real problem* below.

### Two things to listen for first

- **`grind --seq high`, 8x against 16x.** This is the oversampling decision, and
  it is worth 2x the polyphony. See *Is 8x enough?* below — measured, it is not
  close.
- **`bell`, before and after the feedback fix.** The self-feedback diagonal
  still averages 2 *ticks* as the brief specifies, which at 8x puts its Nyquist
  zero at 192 kHz where it does nothing. `bell` is the patch that will change
  character when that window moves to `os` ticks. Judge it both ways; the fix
  costs one add and one subtract, so the sizing numbers hold either way.

## Is 8x enough?

```sh
cargo run --release -p vxn4-render --bin alias
```

Renders a held note at both qualities and subtracts them. Everything but the
operator rate is identical between the two, so the residue is what oversampling
changed — and in-band that is essentially all aliasing, since the extra
harmonics 16x retains live above 100 kHz and the decimator removes them either
way. Reported as dB relative to signal: more negative means more alike.

| patch | note 48 | 72 | 84 | 96 | 102 | 108 |
|---|---|---|---|---|---|---|
| `sine` | -117 | -98 | -70 | -74 | -71 | -62 |
| `epiano` | -65 | -53 | -47 | -41 | -38 | -35 |
| `bell` | -35 | -27 | -24 | -22 | -21 | -21 |
| `saws` | -36 | -33 | -31 | -35 | -32 | -26 |
| `web` | -40 | -37 | -32 | -34 | -31 | -32 |
| `grind` | -32 | -28 | -25 | **-19** | **-10** | **-18** |

`sine` is the control: no PM, no aliasing, -117 dB, which is what says the two
chains are otherwise identical and the method is sound.

**The divergence grows steeply with pitch and with modulator brightness.** At
the top, `grind` differs by -10 dB — the two renders are barely 10 dB apart, and
~40% of that residue lands in 6–12 kHz where nothing masks it. That is not a
subtle difference.

Two reasons it is easy to miss, both of which describe the *rest* of the table
rather than excusing it:

- Below note 84 almost everything sits at -30 dB or lower, and roughly half of
  that residue is above 12 kHz, where it may not survive the monitoring chain or
  adult hearing.
- The other sequences under-expose it. `chord` tops out at note 70, `arp` at 72,
  `vel` at 60; `scale` reaches 95 but holds each note for 81 ms, too short to
  judge timbre. That is why `high` exists.

**Provisional read: 8x is fine for ordinary material and not fine for bright
modulators at the top of the keyboard.** Which suggests the real answer is
neither — it is that `Quality` should be the user-facing quality switch it was
always planned as, defaulting to 8x. The machinery is already there and
switching is now safe under a held note.

## Voice allocation

vxn-2's behaviour ([`vxn2_engine::alloc`], ADR §3), reimplemented against
vxn-4's voice model rather than ported — vxn-2 allocates lane-packed *stacks*
with glide, solo mode, a sustain pedal and pitch bend, none of which vxn-4 has.
What carries over is what decides how it feels to play:

- **16 active voices**, counting only `Held` and `Releasing`, plus **4 spares**
  that only ever hold declick tails — so a burst of steals cannot eat the
  polyphony budget with fades.
- A stolen voice **declicks in place** over 5 ms, keeping its own state so its
  tail rings out continuously, while the new note starts clean on a spare.
- **Quietest-voice stealing, key-up first**: a `Releasing` voice is shed before
  one the player is still holding, ties broken by age.
- Every operator on a stolen voice gets the same wall-clock deadline, so the
  voice collapses evenly. A staggered collapse reads as a timbre sweep, not a
  fade.

Deliberately not carried over: glide, solo/legato, sustain pedal, pitch bend.
`Voice::pitch` is an `f32` in semitones so they have somewhere to land.

## Rate plan

```text
  operators ──8x──▶  s8  ──4x──▶  limiter  ──4x──▶  s4 ──2x──▶ s2 ──1x──▶ out
            ──16x─▶  s16 ──8x──▶  s8  ──▶ (as above)
```

Each halfband stage is named for its input rate and only ever sees that rate —
`s8` takes 8x in both qualities, because at 16x `s16` has already halved it.
Sharing a stage across two rates would leave its filter state incoherent across
a quality switch. Latency is 14 samples at 8x, 15 at 16x.

The FX block the brief puts at 4x is not here yet; the limiter occupies that
slot so the chain shape is real.

## The limiter placement is a real problem

The brief puts the limiter at 4x, which means **two halfband stages run after
it**. Three effects stack up, none of them visible to a steady-tone test, all
measured here:

1. **The limiter overshoots its own threshold on complex material** — its
   one-pole gain smoothing lags a beating waveform. Threshold 0.5 measured
   0.582 out; threshold 0.89 measured 0.979. Against a constant-amplitude sine
   it holds to four decimals, which is why its own tests do not show this.
2. **Decimation exposes inter-sample peaks.** `StereoLimiter` detects
   sample-peak only and says so; resampling to 1x lands samples nearer the true
   continuous peak, so the peaks it declined to detect become real ones.
3. **A hard onset clips before the gain converges.** `current_gain` starts at
   1.0 with a 2 ms attack and cannot reach ~0.2 inside its 2 ms lookahead, so a
   loud chord arriving in one sample is hard-clipped by the limiter's own `±1`
   and the halfbands ring on the squared edges to ~1.02 at 1x. This is
   **independent of the ceiling** — sweeping 0.70..0.89 moved the worst 1x peak
   only between 1.021 and 1.028, which is what proves it is clipping rather
   than gain staging.

Mitigated for now by gain-staging the patches, a 0.80 ceiling, and a backstop
clamp at 1x. **The architectural point stands: a limiter upstream of a resampler
cannot be a brickwall.** Either it moves to 1x last in the chain — which is what
vxn-1b and vxn-2 do — or it gains true-peak detection. The brief wants FX at 4x
with the limiter after them, so this needs deciding.

## Results

Apple M1 (4 performance cores), macOS 26.5.2, rustc 1.95.0, `[profile.release]`
(thin LTO, `codegen-units = 1`). Polyphony is voices sustainable at 48 kHz on
**one core at 100%**, so divide by the host headroom factor for a real budget.
Every comparison below is within a single process; see *Measurement* for why
that matters.

### Oversampling costs exactly what it should. Table length costs nothing.

| mip-0 length | working set / voice | 8x | 16x |
|---|---|---|---|
| 256 | 8 KiB | 108.7 | 54.6 |
| 512 | 16 KiB | 109.3 | 54.8 |
| 2048 | 64 KiB | 109.0 | 54.6 |

16x is a clean 2.00x the cost of 8x — the block is compute-bound, with no
memory cliff hiding in it. **Table length does not register at all**, across an
8x span of working-set size. So the plan to trade table length against
oversampling has nothing to buy: at 64 KiB per voice the gathers are already
landing, and shrinking to 8 KiB buys 0%.

Two independent confirmations that this is real and not a flat-lined
measurement: the isolated lookup bench reports identical throughput at mip 0 and
mip 4 (a 16x difference in resident size), and the layout table below moves
freely on the same fixture.

The reason is that the working set that matters is not the table, it is the
*hot region* of the table. An operator's phase advances ~1.2 entries per tick at
16x, so between gathers it moves within a line or two, and a 2048-entry mip is
touched a few lines at a time like a 256-entry one.

### Voice-major beats operator-major by 24%

Dense routing on both sides, so this is layout and not sparsity. Both layouts
are pinned bit-identical by `ops::tests::layouts_agree_bit_exactly`.

| layout | V=4 | V=8 | V=16 |
|---|---|---|---|
| voice-major (SIMD across voices) | 54.1 | **54.8** | 51.4 |
| op-major (SIMD across operators) | 47.0 | 44.4 | 42.4 |

Voice-major wins at every width, by 15–24%. It also scales the right way: its
SIMD width *is* the voice count, whereas op-major is stuck at the fixed operator
count of 8 and degrades as the bank widens. V=8 is the sweet spot; V=16 costs
6%, most likely because 16 distinct keys spread across more mips and more table
regions than 8 do.

### The lookup: layout is worth 26%, the bounds check is worth 4.6%

Isolated (scattered phase, no PM, no matrix, no sum bus):

| reader | bounds checks | widest NEON op | M lookups/s |
|---|---|---|---|
| value+slope, unchecked | 0 | `fmul.4s` | 2693 |
| plain f32, unchecked | 0 | `fmul.4s` | 2131 |
| value+slope, checked | 9 | `fmul.2s` | 1750 |
| plain f32, checked | 18 | none — scalar | 1347 |

Widths are from `objdump` on the linked binary, per ADR 0002 §4. The gather
cannot vectorise on NEON either way, but the index and interpolation arithmetic
around it can, and **each bounds check halves the width it survives at**. That,
not the branch, is what the check costs.

With checks removed from both, value+slope still beats the plain two-load form
by 26% — so the layout is a genuine win and not an artefact of it carrying half
as many checks. That comparison is the only reason `PlainUnchecked` exists.

In the full operator block, where the lookup is one cost among many, the same
differences compress hard:

| reader | polyphony |
|---|---|
| value+slope, unchecked | 57.4 |
| plain f32, unchecked | 56.8 |
| value+slope, checked | **54.9** |
| plain f32, checked | 52.0 |

**Recommendation: value+slope, bounds-checked.** The check costs 54% in
isolation but 4.6% in the block, which is not enough to buy `unsafe` into a DSP
crate that currently has none. The unchecked readers stay as bench arms.

### Route density is the biggest single lever, and per-voice modulation is free

| scenario | live routes | polyphony |
|---|---|---|
| dense, shared route gains | 64 | 54.9 |
| dense, per-voice route gains | 64 | 51.5 |
| sparse (DX7-shaped), shared gains | 9 | 68.8 |
| sparse, per-voice gains | 9 | 67.3 |

A typical patch (9 routes) runs 25% cheaper than the worst case (64 routes) —
a smaller spread than 64/9 suggests, because the lookups and phase arithmetic
are per-operator and do not care how many routes fed them. Size against the
dense figure; it is only a quarter away.

Per-voice route gains — which is what the brief's per-route modulation actually
requires, since the sources include per-voice envelopes — cost 2–6%. The
multiply already happens; only the multiplicand changes from a broadcast to a
vector load. This was the largest unpriced item in the design and it is close to
free.

## What these numbers do not include

- **The decimator.** It runs on the stereo sum bus, not per voice, so it is a
  fixed cost that does not scale with polyphony and cannot change the answer
  here. `render` closes each oversampled group with a boxcar as a placeholder.
- **Envelopes, the mod matrix, the FX block, the limiter.** Per-operator and
  global envelopes will add real per-voice cost; the FX block will not, being
  post-sum.
- **Voice allocation and note handling.**

## Measurement

Best-of-5 timed runs per point, after a 400 ms spin-up and a per-point warmup.
Back-to-back runs agree to 0.2%. Three methodology faults were found and fixed
while producing the table above, each of which had produced a confident wrong
answer first:

1. **No spin-up.** The first point measured came in 11% low against an identical
   later point, which read as "a 256-entry table is slower than a 512-entry one".
2. **A constant phase stride in the isolated lookup bench** made the index
   sequence affine, LLVM vectorised the whole loop, and all readers collapsed to
   one L1-bandwidth figure — identical to four significant figures across
   implementations that cannot cost the same. Scattered phases from an array fix
   it, and model the real case, where the index depends on a modulation total
   not known until the previous tick.
3. **A single accumulator** in the same bench serialised it on `f32` add latency,
   which is slower than any lookup, reproducing the same identical-figures
   symptom. Eight independent accumulators fix it.

A fourth is a live hazard rather than a fixed fault: **`cargo` on PATH here is
Homebrew's 1.94.1, which is not the rustup shim and silently ignores
`rust-toolchain.toml`.** The pin exists because this workspace's goldens and
codegen are toolchain-sensitive. Measure with `rustup run 1.95.0 cargo ...`, and
check for a `Compiling` line — a stale binary from the other toolchain will
otherwise be re-timed without comment.

Do not compare figures across processes without re-running both; a run started
immediately after a compile reads low.

## Open questions this does not answer

- **The 2-tick feedback average is wrong at 16x.** Averaging two consecutive
  samples puts a zero at Nyquist, which at 48 kHz damps the feedback loop and at
  768 kHz does nothing. The window should be `os` ticks, or the feedback path
  should delay in 1x samples. It is implemented as the brief literally specifies
  so the discrepancy is visible; the fix costs one add and one subtract, so
  these numbers hold under it. **This is a tonal decision, not a perf one.**
- **Mip-selection hysteresis.** Selection is at block rate from the phase
  increment. Pitch modulation dithering across a boundary will switch mips every
  block; needs hysteresis, and a crossfade only if that proves audible (it
  doubles the lookup count).
- **Band-limiting policy.** Mips are band-limited against the oversampled rate,
  so at 16x a low operator carries up to `len/2` harmonics — the table length
  binds before Nyquist does. Whether that is the right cap is a tonal question.
