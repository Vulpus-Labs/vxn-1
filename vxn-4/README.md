# vxn-4

Synth #4: eight phase-modulation operators, every operator routed to every
operator including itself, plus a panned output per operator into a stereo sum
bus. No filter.

Playable in a CLAP host, and playable offline. **No faceplate** — the plugin
exposes eleven parameters and a host's generic UI is enough to turn them. Under
that: voice allocation, envelopes, seven hardwired patches, a modulation matrix
on eight macro knobs, and the oversampling chain down through a limiter.

Most of this README is the **sizing** work that came first, because the
oversampling factor, the wavetable length and the lane-loop layout move the
polyphony ceiling more than anything else in the design, and they were measured
before they were designed.

## Crates

| crate | what it is |
|---|---|
| [`vxn4-dsp`](crates/vxn4-dsp) | band-limited mip-mapped wavetables, and the 8-operator block in two lane-loop layouts |
| [`vxn4-engine`](crates/vxn4-engine) | voice allocation, envelopes, patches, the mod matrix, rate plan, limiter |
| [`vxn4-clap`](crates/vxn4-clap) | the CLAP plugin shell — eleven params, no faceplate |
| [`vxn4-render`](crates/vxn4-render) | offline renderer — plays a note sequence into a WAV |
| [`vxn4-op-bench`](crates/vxn4-op-bench) | the sizing sweep and its criterion counterpart |

```sh
# play it
./deploy.sh                           # → ~/Library/Audio/Plug-Ins/CLAP/vxn4.clap
./deploy.sh --bundle-only             # stage into target/bundled/, do not install
./deploy.sh --uninstall

# listen offline
cargo run --release -p vxn4-render -- --all            # every patch x sequence
cargo run --release -p vxn4-render -- --patch 2 --seq scale --os 16 bell.wav
cargo run --release -p vxn4-render -- --patch 0 --macro 1=1.0 buzz.wav

# measure
cargo run --release -p vxn4-op-bench --bin sweep       # the decision
cargo bench -p vxn4-op-bench                           # confidence intervals
```

**Build with `rustup run 1.95.0 cargo ...`** — see *Measurement* below for why
plain `cargo` is the wrong compiler on this machine.

## The plugin

Eleven parameters, and the count is the design:

| id | param | range |
|---|---|---|
| 0 | Patch | stepped, `sine`…`supersaw` |
| 1 | Quality | stepped, 8x / 16x |
| 2 | Master Gain | 0 … +6 dB, applied **upstream** of the limiter |
| 3–10 | Macro 1–8 | 0 … 1 |

**Modulation reaches the host as eight knobs and nothing else.** The synth has
96 modulatable destinations — 64 PM depths, then four per-operator families of
eight: sum-bus sends, damping corners, detune and pan — each taking two
sources; exposing that would be several hundred automation lanes, and it would bake the patch's routing topology into every
saved project, so rewiring a patch would leave every lane that named a route
pointing somewhere else. The knobs are matrix *sources*; which routes each one
drives, and how far, is patch state. A lane that says "macro 3" survives the
patch behind it being rewired.

The brief's "two sources per destination, additive and scaling" needed no vxn-4
code at all: it is `MatrixSlot`'s `source` plus `scale_src` in
[`vxn-core-matrix`](../crates/vxn-core-matrix), shared with vxn-1b and vxn-2.
`epiano` and `grind` each wire one scaled route so the VCA path is exercised
rather than merely available.

Two things the plugin does that are worth knowing:

- **Reported latency is constant at 15 samples**, which is the 16x figure. Real
  latency is 14 at 8x. Quality is a live parameter, so an exact report would
  mean renegotiating the host's graph whenever a knob moves; the constant costs
  one sample — 21 µs — at 8x. Named as a choice in `HOST_LATENCY_SAMPLES`
  rather than left as a wrong constant.
- **Re-sending the current patch value is a no-op.** Selecting a patch panics
  every sounding voice, and hosts that push their whole parameter set each
  block exist, so the write is guarded on an actual change. Without the guard
  one of those hosts is a permanent all-notes-off.

## Playing it

Seven patches — five graded by routing density, one off the ladder for
aliasing, one for the spread controls — and six sequences, each chosen to put
one question in front of your ears.

| patch | routes | what it is for |
|---|---|---|
| `sine` | 0 | reference tone. One operator, nothing modulating — really a test of the decimator |
| `epiano` | 4 | two 2-op stacks, the FM idiom |
| `bell` | 6 | inharmonic ratios plus self-feedback |
| `saws` | 11 | assignable waveforms — the thing a DX7 cannot do, and the hardest case for the mips |
| `web` | 64 | every route live; the worst case the sizing bench quotes against |
| `grind` | 4 | **saw modulating saw at high index** — the aliasing torture case, deliberately not musical |
| `supersaw` | 0 | seven detuned saws plus a sine; the whole spread is macros |

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

### What the macros do

Each patch wires a few. All macros at zero **is** the patch as its table writes
it, which is what keeps the patch source readable on its own — a default of 0.5
would mean no patch ever sounded as authored without pulling eight knobs down.

Four kinds of destination are under the knobs: **PM depths** (how much
modulation), **sum-bus sends** (how loud an operator is), **damping corners**
(how much modulation survives to the phase accumulator), and **detune / pan**
(where an operator sits in pitch and in the image). Damping is in *octaves* and
detune in *semitones*, both against the patch's authored value; damping routes
are at negative depth, so knob up is always darker.

| patch | M1 | M2 | M3 | M4 | M5 |
|---|---|---|---|---|---|
| `sine` | self-feedback on op0 — a sine to a buzz | **damp op0** | | | |
| `epiano` | brightness on both stacks | cross-feed | *gates M2* | **damp both carriers** | |
| `bell` | feedback on the inharmonic modulator | brightness | **damp carriers** | **damp the feedback modulator** | |
| `saws` | triangle modulator into both saws | sub level (a sum-bus send) | op4 → op3 | op4 → op2, a route the patch does not author | **damp both saw carriers** |
| `web` | four feedback diagonals at once | the two corner routes | **damp all eight — the master control** | | |
| `grind` | index past 1.7 turns, **both carriers** | self-feedback | *gates M2* | **the fizz control** | |
| `supersaw` | **detune** | **width** | **level taper** | **modulation amount** | **modulation rolloff** |

Measured range, as the render's difference from the knob-at-zero case (higher
is a bigger change; `+` means the difference exceeds the original signal):

| patch | knob | 0 → 1 |
|---|---|---|
| `grind` | M4 | **+4.7 dB** — a different instrument |
| `sine` | M2 | -2.7 dB |
| `web` | M3 | -9.5 dB |
| `supersaw` | M1 detune | -5.6 dB |
| `supersaw` | M2 width | -7.9 dB, and mono → -7.3 dB side/mid |
| `supersaw` | M3 taper | -8.9 dB |

`sine` M1+M2 is the clearest pair for hearing what damping *is*: one operator,
one feedback route, nothing else moving. `grind` M4 is the widest.

Two of those are load-bearing beyond the sound:

- `sine`'s M1 and `saws`' M4 both drive routes whose authored depth is **zero**.
  The lane set is fixed when a patch is compiled, so that a depth update on the
  audio thread cannot reach an allocator — which means a route the matrix can
  reach must have a lane reserved for it up front, whether or not the patch uses
  it. `saws` M4 is the off-diagonal case that actually needs the reservation;
  `sine` M1 is a feedback diagonal, which lives in its own array and would work
  either way.
- `epiano` M3 and `grind` M3 are scale VCAs. M2 does nothing until M3 opens it.

### Two things to listen for first

- **`grind --seq high`, 8x against 16x.** This is the oversampling decision, and
  it is worth 2x the polyphony. See *Is 8x enough?* below — measured, it is not
  close.
- **`bell`, before and after the feedback fix.** The self-feedback diagonal
  still averages 2 *ticks* as the brief specifies, which at 8x puts its Nyquist
  zero at 192 kHz where it does nothing. `bell` is the patch that will change
  character when the feedback path is damped properly. Judge it both ways —
  and see *Open questions* for why the two candidate fixes do not cost the
  same.

## Is 8x enough?

```sh
cargo run --release -p vxn4-render --bin alias
```

Renders a held note at both qualities and subtracts them. Reported as dB
relative to signal: more negative means more alike.

> **This metric no longer measures only aliasing.** Mip selection is dynamic
> now, and at 16x there is twice the Nyquist headroom — so the selector
> *correctly* picks a finer, brighter mip than it does at 8x. The two renders
> then differ in table content, which the subtraction reads as divergence
> indistinguishably from fold-back. The columns below are still useful as
> "how different do 8x and 16x sound", which is the question a user asks; they
> are no longer a clean aliasing figure. Pinning the mip across both arms would
> restore that, and is a change to the measuring tool rather than the synth.

| patch | note 48 | 72 | 84 | 96 | 102 | 108 |
|---|---|---|---|---|---|---|
| `sine` | -117 | -98 | -70 | -74 | -71 | -62 |
| `epiano` | -71 | -59 | -53 | -46 | -44 | -41 |
| `bell` | -46 | -38 | -34 | -32 | -31 | -30 |
| `saws` | **-24** | -34 | -32 | -32 | -33 | -25 |
| `web` | -44 | -33 | -35 | -38 | -37 | -43 |
| `grind` | **-11** | -35 | -33 | -27 | **-20** | -26 |

`sine` is the control: no PM, so damping does nothing and its row is unchanged
to the dB from the pre-damping table. That is what says the two chains are
otherwise identical and the method is still sound.

### What damping bought, separated from what the mip selector cost

The two changes landed together and pull in opposite directions on this metric,
so they were measured apart by disabling the mip update:

| | `saws` 48 | `saws` 102 | `grind` 48 | `grind` 102 |
|---|---|---|---|---|
| before either | -36 | -32 | -32 | **-10** |
| damping only | -27 | **-37** | **-37** | **-25** |
| damping + dynamic mips | -24 | -33 | **-11** | -20 |

**The damping is a clear win**: `grind` at note 102 improves 15 dB, from -10.4
to -25.3, and `bell` and `epiano` improve 6–11 dB across the board. That was the
hypothesis — a pole on each operator's modulation input removes ultrasonic
content that could only ever fold — and it held.

**The dynamic mip selector is what moved `grind` 48 to -11 dB**, and it is not
a defect: 85% of that residue sits below 2 kHz on a 131 Hz note, which is not
what fold-back looks like. It is the two arms running different mips. The
selector is doing exactly what it should; the metric is what stopped being able
to tell the difference.

**The divergence still grows with pitch and with modulator brightness**, but far
less steeply than it did. `grind` at the top of the keyboard was -10 dB — two
renders barely 10 dB apart, with ~40% of the residue in 6–12 kHz where nothing
masks it. It is now -20 dB with the selector in play and -25 dB without.

Two reasons it was easy to miss in the first place, both of which describe the
*rest* of the table rather than excusing it:

- Below note 84 almost everything sits at -30 dB or lower, and roughly half of
  that residue is above 12 kHz, where it may not survive the monitoring chain or
  adult hearing.
- The other sequences under-expose it. `chord` tops out at note 70, `arp` at 72,
  `vel` at 60; `scale` reaches 95 but holds each note for 81 ms, too short to
  judge timbre. That is why `high` exists.

**Provisional read: 8x is fine for ordinary material, and the case against it at
the top of the keyboard is now much weaker than it was.** Damping closed most of
the gap the original table found. `Quality` stays the user-facing switch it was
always planned as, defaulting to 8x; the machinery is there and switching is
safe under a held note.

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

### The layout result **inverted** when damping landed, and is now open

Dense routing on both sides, so this is layout and not sparsity. Both layouts
are pinned bit-identical by `ops::tests::layouts_agree_bit_exactly`.

| layout | V=4 | V=8 | V=16 |
|---|---|---|---|
| voice-major, before damping | 54.1 | **54.8** | 51.4 |
| op-major, before damping | 47.0 | 44.4 | 42.4 |
| voice-major, now | 49.8 | 52.0 | 48.9 |
| op-major, now | **60.8** | **58.7** | 56.0 |

Voice-major used to win at every width by 15–24%. It now **loses** by 13% at
V=8 and 22% at V=4. Reproduced across two runs, agreeing to 0.5%.

Voice-major lost ~5% to the damping pass, which is expected. Op-major gained
~32%, which is not. The likely cause is the ring: op-major's feedback read was
`hist[prev][v]` **and** `hist[prev2][v]`, two unconditional reads from ring
slots a whole `V * NOPS` block apart, once per voice. Dropping the 2-tick
average removed one of them from the layout that was already the more
memory-bound of the two. That is a hypothesis, not a measurement.

**The engine is still on voice-major, and should stay there until this is
settled**, for a specific reason: the bench measures the **dense** case, and
five of the six patches are sparse. Voice-major skips zero routes; op-major
multiplies them through by design. At `epiano`'s 4 routes op-major would do 64
multiplies where voice-major does 4, and the density section below shows
sparsity is worth ~24% — comfortably enough to swing 13% back. That arm has
never been measured for op-major.

Switching the engine on a dense-only number is exactly the mistake the density
section exists to prevent. **Measure op-major sparse first.**

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
- **Envelopes, the FX block, the limiter.** Per-operator and global envelopes
  add real per-voice cost; the FX block will not, being post-sum.
- **Voice allocation and note handling.**

The **mod matrix** has since landed and is not on that list any more, because it
turned out to cost nothing worth measuring. Every destination is `patch_global`,
so a macro move is 16 slot evaluations, 64 depth writes and 8 sum-bus multiplies
— once per control block, and only when a knob has actually moved. It touches
nothing per voice and nothing per operator tick. That stays true only while
every source is patch-wide; the day a per-voice source lands (an envelope, a
velocity) the whole tier column in `matrix.rs` has to be revisited, and the
coherence predicate is there to fail loudly when it is not.

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

- **The 2-tick feedback average does nothing under oversampling.** Averaging two
  consecutive samples puts a zero at Nyquist, which at 48 kHz damps the feedback
  loop and at 768 kHz does not. Implemented as the brief literally specifies so
  the discrepancy is visible. **Primarily a tonal decision** — but the two
  candidate fixes have different costs, and one of them invalidates the table
  above:
  - a **boxcar over `os` ticks** needs an `os`-deep output ring instead of the
    3-deep one, a 5x larger history working set in the hot loop at 16x. These
    numbers would have to be re-measured under it.
  - a **one-pole cornered in Hz** is rate-independent by construction, costs one
    multiply-add and one `[f32; V]` of state per operator, and leaves the ring
    at 3 — so the numbers hold. Different phase response, which is the part that
    has to be judged by ear.

  An earlier revision of this bullet said the fix costs "one add and one
  subtract" either way, which is only true of a running-sum boxcar and silently
  assumed the deeper ring it needs.
- **Mip-selection hysteresis.** Selection is at block rate from the phase
  increment. Pitch modulation dithering across a boundary will switch mips every
  block; needs hysteresis, and a crossfade only if that proves audible (it
  doubles the lookup count).
- **Band-limiting policy.** Mips are band-limited against the oversampled rate,
  so at 16x a low operator carries up to `len/2` harmonics — the table length
  binds before Nyquist does. Whether that is the right cap is a tonal question.

## Still to build

- **The brief's other mod sources.** Two LFOs and two envelopes, as matrix
  sources beside the macros. That is new rows in `SourceId`, not a new
  mechanism — but see the tier note above: an envelope is `per_lane`, and every
  destination is currently `patch_global`, so it is the routing *tiers* that
  need the work, not the sources.
- **Smoothing.** Every destination declares `smooth = block`, which is the truth
  and not a placeholder: totals are applied once per control block and held, so
  a fast macro sweep zippers at the control rate. `vxn-core-matrix` has a
  smoother bank; wiring it up is the follow-up. The column is declared honestly
  meanwhile rather than claiming a filter that is not running.
- **The FX block at 4x.** The chain shape is real — the limiter occupies that
  slot — so FX drop in beside it. See *The limiter placement is a real problem*
  for the question that has to be settled first.
- **Editable patches, and a preset format.** The six are ear-fodder. When they
  become editable the state blob gains a payload and a version bump; its header
  is shaped for that.
- **A faceplate.** Not obviously the next thing. An 8×8 routing grid with two
  sources per cell is a real UI problem, and the host's generic UI is enough to
  keep making ear-driven choices meanwhile.
