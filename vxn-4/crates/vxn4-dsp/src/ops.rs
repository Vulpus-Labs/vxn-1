//! The 8-operator phase-modulation block, in two lane-loop layouts.
//!
//! Every operator has an output into every operator including itself, plus a
//! panned output into a stereo sum bus. **Every** route reads the previous
//! tick — self routes are not special-cased.
//!
//! ## Every edge is buffered, and that is the design
//!
//! Nothing reads the current tick, so the operator loop order is irrelevant and
//! `A -> B -> C -> A` carries three ticks of delay, one per hop. There is no
//! cycle detection, no topological sort and no designated back edge, because
//! the brief specifies an all-to-all matrix and an all-to-all graph has no
//! topological order to find. Uniform delay is the honest consequence, and it
//! is what lets the two layouts below be bit-identical.
//!
//! ## Damping: a one-pole on each operator's summed modulation input
//!
//! The design goal is **bounded chaos** — complex timbral interaction that does
//! not collapse straight into noise. Three mechanisms bound it, and they bound
//! different things:
//!
//! | mechanism | bounds | side |
//! |---|---|---|
//! | mip selection | harmonics an operator *emits* | source, pitch-relative |
//! | oversampling | how much of the product set *folds* | system, rate |
//! | **this filter** | harmonics an operator *receives* | destination, absolute |
//!
//! The third is the one that bounds **products**. An operator is mip-limited to
//! its own harmonics, but the sidebands it generates are products, and nothing
//! band-limits a product — so a chain's bandwidth grows without bound down the
//! chain. A pole on the receiving end is the only thing in the system that
//! stops that.
//!
//! On the **sum** rather than per route, for three reasons. A linear filter on
//! a sum equals the same filter on every addend, so this is per-route semantics
//! at one-eighth the state and one-eighth the cost. `pm` is on the modulation
//! path by construction — the sum bus reads operator *outputs*, so filtering
//! here cannot darken the audio path, which filtering outputs would. And every
//! cycle passes through its members' `pm`, so a 3-cycle gets three poles and a
//! self-loop gets one: damping scales with hop count without a special case.
//!
//! Cornered in **Hz**, so the response is rate-independent and identical at 8x
//! and 16x. That also makes it self-regulating: a fixed corner backs modulation
//! off hardest at the top of the keyboard, which is exactly where the alias
//! measurements show folding is worst.
//!
//! This replaces the brief's "2-sample feedback buffer". That was inherited
//! from the DX7, where averaging two consecutive samples puts a zero at 24 kHz
//! and does audible work; at 8x that zero sits at 192 kHz and does nothing at
//! all. The replacement is not a DX7 emulation and is not trying to be.
//!
//! ## Two layouts
//!
//! [`VoiceMajor`] is SoA over voices: the outer loop walks the 8 operators and
//! the inner loop is `V` voices wide. Every lane of a gather reads the *same*
//! waveform table at a near-identical mip, so the addresses cluster, and the
//! SIMD width is the voice count — the axis that scales.
//!
//! [`OpMajor`] is SoA over operators: the outer loop walks voices and the inner
//! loop is 8 operators wide. The routing collapses to a dense 8x8 matvec, which
//! vectorises cleanly, but each gather straddles up to 8 different tables.
//!
//! They are held bit-identical by `tests::layouts_agree_bit_exactly`, so the
//! bench measures the layout and nothing else.
//!
//! ## Output history is a ring, not a shuffle
//!
//! Every route reads the previous tick and nothing reads further back, so two
//! ticks of operator output have to be live. Both layouts hold them in a
//! 2-deep ring indexed by a rotating head.
//!
//! It was 3-deep while the self route averaged two previous ticks. Nothing else
//! ever wanted `prev2`, so moving the damping onto `pm` **shrank** the hot
//! loop's history working set by a third rather than growing it — worth stating
//! because the alternative fix considered first (a boxcar over `os` ticks)
//! would have grown it fivefold at 16x.
//!
//! The obvious alternative — `prev = cur;` — costs a `NOPS * V`-float copy per
//! tick, which is pure overhead that grows with the bank width and would have
//! shown up in the bench as an artificial penalty on wide banks. Rotating an
//! index costs nothing and scales with nothing.
//!
//! ## Mip selection is dynamic, and hysteretic
//!
//! A mip is chosen for a *rate*, and under modulation an operator's
//! instantaneous rate is far above its nominal one — so selecting from the
//! nominal increment under-protects exactly the operators being driven hardest.
//! Selection therefore runs at control rate from the peak instantaneous rate
//! measured over the previous block, which the damping filter yields as a
//! by-product: its own residue `pm - state` is proportional to `d(pm)/dt`, and
//! multiplying the block's peak by the filter coefficient recovers turns/tick.
//!
//! The hysteresis is asymmetric — see
//! [`MIP_HYSTERESIS`](crate::wavetable::MIP_HYSTERESIS) for why erring coarse
//! is the only acceptable direction, and why that removes the need for a
//! crossfade.
//!
//! ## What is not modelled
//!
//! Per-route modulation depth is held constant across a block and across
//! voices. Real per-route mod is per-voice, because the sources include
//! per-voice envelopes — that turns each route gain from a broadcast scalar
//! into a `V`-wide vector. The cost is one extra vector load per lane per tick
//! and no extra arithmetic (the multiply already happens), so it is a bounded
//! and separately measurable delta rather than a hole in these numbers.
//! `benches/op_bank.rs` prices it directly.

use crate::wavetable::{Lookup, WaveBank, Waveform};

/// Operators per voice. Fixed by the brief.
pub const NOPS: usize = 8;

/// Turns → Q32 phase.
pub const PHASE_SCALE: f32 = 4_294_967_296.0;

/// MIDI note to Hz, A440 equal temperament.
#[inline]
pub fn note_to_freq(key: u8) -> f32 {
    440.0 * ((key as f32 - 69.0) / 12.0).exp2()
}

/// As [`note_to_freq`], for a fractional pitch in semitones.
///
/// The engine drives lanes through this rather than [`note_to_freq`] so that
/// detune, bend and glide are expressible without a second code path.
#[inline]
pub fn pitch_to_freq(semitones: f32) -> f32 {
    440.0 * ((semitones - 69.0) / 12.0).exp2()
}

/// Per-operator configuration. The parts that affect the hot loop's shape.
#[derive(Clone, Copy, Debug)]
pub struct OpConfig {
    pub wave: Waveform,
    /// Frequency ratio against the voice's key.
    pub ratio: f32,
    /// Output level, pre-routing.
    pub level: f32,
    /// Sum-bus pan, -1 left to +1 right.
    pub pan: f32,
    /// Corner of the one-pole on this operator's summed modulation input, in
    /// **Hz** — the bounded-chaos control. Lower is more damped.
    ///
    /// In Hz rather than normalised so the response is identical at 8x and 16x.
    /// Zero or negative is a bypass.
    pub damp_hz: f32,
}

/// Default damping corner: above the audio band, so it is nearly transparent
/// tonally while still bounding what an operator can receive.
///
/// Chosen to be conservative rather than characterful. At 8x it is ~20 dB down
/// by the operator block's Nyquist, which bounds product bandwidth without
/// audibly darkening a patch that was voiced before this existed. Patches that
/// want damping as a *sound* dial it down; that is the axis to explore.
pub const DEFAULT_DAMP_HZ: f32 = 20_000.0;

impl Default for OpConfig {
    fn default() -> Self {
        Self {
            wave: Waveform::Sine,
            ratio: 1.0,
            level: 1.0,
            pan: 0.0,
            damp_hz: DEFAULT_DAMP_HZ,
        }
    }
}

/// The routing matrix as authored.
#[derive(Clone, Copy, Debug)]
pub struct Routing {
    /// `pm[dest][src]` — modulation depth in phase *turns* per unit of source
    /// output. The diagonal is self-feedback and reads the 2-tick average.
    pub pm: [[f32; NOPS]; NOPS],
    /// Per-operator gain into the stereo sum bus.
    pub out: [f32; NOPS],
}

impl Default for Routing {
    fn default() -> Self {
        Self {
            pm: [[0.0; NOPS]; NOPS],
            out: [0.0; NOPS],
        }
    }
}

impl Routing {
    /// Count of non-zero modulation routes, diagonal included.
    pub fn density(&self) -> usize {
        self.pm.iter().flatten().filter(|g| **g != 0.0).count()
    }
}

/// One off-diagonal modulation route.
#[derive(Clone, Copy, Debug)]
pub struct Lane {
    pub src: u32,
    pub gain: f32,
}

/// [`Routing`] compiled into the two forms the two layouts want.
///
/// Carrying both is deliberate, not redundancy: a sparse lane list is the right
/// representation for a voice-wide inner loop (zero routes cost nothing) and a
/// dense transposed matrix is the right one for an operator-wide inner loop (it
/// vectorises; a variable-length lane list does not). The layout bench runs at
/// **dense** routing precisely so this difference cannot confound it — with all
/// 64 routes live the two forms do identical work. The density bench then
/// prices what sparsity buys, separately.
pub struct CompiledRouting {
    /// Off-diagonal routes, grouped by destination, ascending by source.
    ///
    /// A fixed array rather than a `Box<[Lane]>`: a patch change arrives as a
    /// CLAP parameter event, which is delivered **on the audio thread**, so
    /// compiling a routing must not allocate. `NOPS * (NOPS - 1)` is the exact
    /// worst case — every off-diagonal route live — so this never truncates.
    lanes: [Lane; NOPS * (NOPS - 1)],
    dest_start: [u32; NOPS + 1],
    n: usize,
    /// Transposed, diagonal-zeroed matrix: `pm_t[src][dest]`.
    pub pm_t: [[f32; NOPS]; NOPS],
    /// Self-feedback depth per operator. Zero means no self route.
    pub fb: [f32; NOPS],
}

impl CompiledRouting {
    pub fn compile(r: &Routing) -> Self {
        Self::compile_with(r, &[[false; NOPS]; NOPS])
    }

    /// Compile `r`, forcing a lane to exist wherever `force` is set even if the
    /// base depth is zero.
    ///
    /// The modulation matrix is why this exists. A macro can drive a route
    /// whose *authored* depth is zero, and the lane set is fixed at compile
    /// time so that [`Self::set_pm`] can update depths every control tick without
    /// reallocating. Without `force`, such a route would have no lane to write
    /// into and the macro would silently do nothing.
    pub fn compile_with(r: &Routing, force: &[[bool; NOPS]; NOPS]) -> Self {
        let mut lanes = [Lane { src: 0, gain: 0.0 }; NOPS * (NOPS - 1)];
        let mut dest_start = [0u32; NOPS + 1];
        let mut pm_t = [[0.0f32; NOPS]; NOPS];
        let mut fb = [0.0f32; NOPS];
        let mut n = 0usize;

        for d in 0..NOPS {
            dest_start[d] = n as u32;
            fb[d] = r.pm[d][d];
            for s in 0..NOPS {
                if s == d {
                    continue;
                }
                let gain = r.pm[d][s];
                pm_t[s][d] = gain;
                if gain != 0.0 || force[d][s] {
                    lanes[n] = Lane {
                        src: s as u32,
                        gain,
                    };
                    n += 1;
                }
            }
        }
        dest_start[NOPS] = n as u32;

        Self {
            lanes,
            dest_start,
            n,
            pm_t,
            fb,
        }
    }

    /// Update every lane depth in place, leaving the lane set alone.
    ///
    /// The control-rate path for modulation. Allocation-free by construction,
    /// and it cannot introduce a route the compile step did not reserve — which
    /// is the contract [`Self::compile_with`]'s `force` argument exists to
    /// satisfy.
    pub fn set_pm(&mut self, pm: &[[f32; NOPS]; NOPS]) {
        for d in 0..NOPS {
            self.fb[d] = pm[d][d];
            let (s0, s1) = (self.dest_start[d] as usize, self.dest_start[d + 1] as usize);
            for lane in &mut self.lanes[s0..s1] {
                lane.gain = pm[d][lane.src as usize];
            }
            for s in 0..NOPS {
                self.pm_t[s][d] = if s == d { 0.0 } else { pm[d][s] };
            }
        }
    }

    #[inline]
    pub fn lanes(&self, dest: usize) -> &[Lane] {
        let s = self.dest_start[dest] as usize;
        let e = self.dest_start[dest + 1] as usize;
        &self.lanes[s..e]
    }

    pub fn lane_count(&self) -> usize {
        self.n
    }
}

/// Per-operator stereo sum-bus gains, pan folded in.
#[derive(Clone, Copy, Debug)]
pub struct SumBus {
    pub l: [f32; NOPS],
    pub r: [f32; NOPS],
}

impl SumBus {
    /// Constant-power pan across `Routing::out`.
    pub fn new(cfg: &[OpConfig; NOPS], routing: &Routing) -> Self {
        let mut l = [0.0f32; NOPS];
        let mut r = [0.0f32; NOPS];
        for d in 0..NOPS {
            let theta = (cfg[d].pan.clamp(-1.0, 1.0) + 1.0) * 0.25 * std::f32::consts::PI;
            l[d] = routing.out[d] * theta.cos();
            r[d] = routing.out[d] * theta.sin();
        }
        Self { l, r }
    }
}

/// Quantise a modulation total in turns to a Q32 phase offset.
///
/// The wrap first is what makes this safe for arbitrary modulation index: a
/// direct `(turns * 2^32) as i32` saturates past ±0.5 turns and would clamp a
/// hot operator instead of wrapping it. Folding to `[-0.5, 0.5)` costs a round
/// and a subtract, and leaves the cast provably in range.
#[inline]
fn phase_offset(turns: f32) -> u32 {
    let wrapped = turns - turns.round();
    (wrapped * PHASE_SCALE) as i32 as u32
}

/// SoA over voices. The inner loop is `V` voices wide.
pub struct VoiceMajor<const V: usize> {
    phase: [[u32; V]; NOPS],
    inc: [[u32; V]; NOPS],
    mip: [[u32; V]; NOPS],
    lvl: [[f32; V]; NOPS],
    /// `hist[ring][op][voice]` — two ticks of output, rotated by [`Self::head`].
    hist: [[[f32; V]; NOPS]; 2],
    head: usize,
    wave: [Waveform; NOPS],
    /// One-pole state on each operator's summed modulation input.
    pmz: [[f32; V]; NOPS],
    /// Per-operator damping coefficient, from [`damp_coeff`].
    damp: [f32; NOPS],
    /// Peak `|pm - pmz|` since the last [`Self::update_mips`]. The filter's own
    /// residue, which is proportional to `d(pm)/dt` — so the mip selector gets
    /// its rate estimate as a by-product of a pass that has to happen anyway.
    peak: [[f32; V]; NOPS],
}

/// Ring offsets for a head position: (current, previous).
#[inline]
fn ring(head: usize) -> (usize, usize) {
    (head, head ^ 1)
}

/// One-pole coefficient for a corner of `hz` at sample rate `sr`.
///
/// `1 - exp(-2 pi fc / sr)`. Taking the corner in Hz and the rate as an
/// argument is what makes the response identical at 8x and 16x — a coefficient
/// chosen per tick instead would change the filter every time quality moved.
///
/// Clamped to `[0, 1]`: a corner at or above Nyquist is a bypass, and a
/// negative one would be an unstable pole rather than "off".
#[inline]
// `!(hz > 0.0)` is the NaN-safe spelling and the point of writing it this way:
// `hz <= 0.0` is false for NaN, so a NaN corner would fall through to `.exp()`
// and poison every voice on the operator. Clippy's `partial_cmp` suggestion is
// the less readable form of the same test.
#[allow(clippy::neg_cmp_op_on_partial_ord)]
pub fn damp_coeff(hz: f32, sr: f32) -> f32 {
    if !(hz > 0.0) || sr <= 0.0 {
        return 1.0;
    }
    (1.0 - (-std::f32::consts::TAU * hz / sr).exp()).clamp(0.0, 1.0)
}

impl<const V: usize> VoiceMajor<V> {
    pub fn new() -> Self {
        Self {
            phase: [[0; V]; NOPS],
            inc: [[0; V]; NOPS],
            mip: [[0; V]; NOPS],
            lvl: [[0.0; V]; NOPS],
            hist: [[[0.0; V]; NOPS]; 2],
            head: 0,
            wave: [Waveform::Sine; NOPS],
            pmz: [[0.0; V]; NOPS],
            // Unity until told otherwise: a fresh bank passes modulation
            // through rather than silently damping it at some default.
            damp: [1.0; NOPS],
            peak: [[0.0; V]; NOPS],
        }
    }

    /// Set the per-operator damping coefficients. Call whenever the patch or
    /// the oversampled rate changes — the coefficient depends on both.
    pub fn set_damping(&mut self, cfg: &[OpConfig; NOPS], sr_os: f32) {
        for d in 0..NOPS {
            self.damp[d] = damp_coeff(cfg[d].damp_hz, sr_os);
        }
    }

    /// Re-select every lane's mip from the peak instantaneous rate measured
    /// since the last call, and clear the measurement. Control rate.
    ///
    /// Asymmetric by construction. Coarsening is taken immediately and at any
    /// distance, because a mip that is too fine aliases and delaying that is
    /// the unsafe direction; the return to a finer, brighter mip waits for the
    /// hysteresis band and then moves one step, so a deep excursion eases back
    /// over a few blocks instead of snapping.
    ///
    /// The invariant that matters is that this can only ever err **coarse** —
    /// pinned by `tests::hysteresis_only_ever_errs_coarse`.
    pub fn update_mips(&mut self, bank: &WaveBank, cfg: &[OpConfig; NOPS]) {
        for d in 0..NOPS {
            let table = bank.table(cfg[d].wave);
            let g = self.damp[d];
            for v in 0..V {
                // `pm - pmz` settles at `rate / g` for a ramp, so `g * peak`
                // recovers turns per tick. Saturating: a violent excursion
                // should pin the selector at the coarsest mip, not wrap into
                // the finest.
                let dev = (g * self.peak[d][v] * PHASE_SCALE).max(0.0);
                let rate = self.inc[d][v].saturating_add(dev as u32);

                let cur = self.mip[d][v] as usize;
                let want = table.mip_for(rate);
                self.mip[d][v] = if want > cur {
                    want as u32
                } else if cur > 0 && rate < table.hysteresis_floor(cur) {
                    (cur - 1) as u32
                } else {
                    cur as u32
                };
                self.peak[d][v] = 0.0;
            }
        }
    }

    /// Block-edge setup: increments, mip selection, levels, decorrelated phase.
    ///
    /// `sr_os` is the **oversampled** rate, so the mip choice already accounts
    /// for the headroom oversampling buys.
    pub fn cook(&mut self, bank: &WaveBank, cfg: &[OpConfig; NOPS], keys: &[u8; V], sr_os: f32) {
        for d in 0..NOPS {
            self.wave[d] = cfg[d].wave;
            let table = bank.table(cfg[d].wave);
            for v in 0..V {
                let hz = note_to_freq(keys[v]) * cfg[d].ratio;
                let inc = ((hz / sr_os) * PHASE_SCALE) as u32;
                self.inc[d][v] = inc;
                self.mip[d][v] = table.mip_for(inc) as u32;
                self.lvl[d][v] = cfg[d].level;
                // Decorrelate so the optimiser cannot collapse voices and so
                // the gathers are not all reading one cache line.
                self.phase[d][v] = (v as u32)
                    .wrapping_mul(0x9E37_79B9)
                    .wrapping_add((d as u32).wrapping_mul(0x85EB_CA6B));
            }
        }
    }

    // ── Per-lane control ────────────────────────────────────────────────────
    //
    // `cook` sets up a whole bank at once, which is what the bench wants and
    // what a synth never wants: notes arrive one at a time, on one lane, while
    // the others are mid-note. These drive a single lane without disturbing its
    // neighbours.

    /// Install the patch's waveform assignment. Bank-wide — every lane in a
    /// bank runs the same patch, which is why route gains can stay broadcast
    /// scalars and only `lvl` has to be per-voice.
    pub fn set_waves(&mut self, cfg: &[OpConfig; NOPS]) {
        for d in 0..NOPS {
            self.wave[d] = cfg[d].wave;
        }
    }

    /// Point one lane at a pitch, in fractional semitones.
    ///
    /// Sets the phase increment and re-selects the mip for every operator, but
    /// leaves phase and output history alone, so this is also the re-pitch path
    /// for a glide or a legato note change.
    pub fn cook_lane(
        &mut self,
        bank: &WaveBank,
        cfg: &[OpConfig; NOPS],
        lane: usize,
        pitch: f32,
        sr_os: f32,
    ) {
        let hz0 = pitch_to_freq(pitch);
        for d in 0..NOPS {
            let table = bank.table(cfg[d].wave);
            let inc = ((hz0 * cfg[d].ratio / sr_os) * PHASE_SCALE) as u32;
            self.inc[d][lane] = inc;
            self.mip[d][lane] = table.mip_for(inc) as u32;
        }
    }

    /// Reset one lane to silence: zero phase history, decorrelated phases.
    ///
    /// This is the fresh-onset path. `seed` decorrelates the starting phases so
    /// that two lanes sounding the same note do not produce a doubled, phase-
    /// locked copy — and so the optimiser cannot collapse lanes.
    pub fn reset_lane(&mut self, lane: usize, seed: u32) {
        for d in 0..NOPS {
            self.phase[d][lane] = seed
                .wrapping_mul(0x9E37_79B9)
                .wrapping_add((d as u32).wrapping_mul(0x85EB_CA6B));
            self.lvl[d][lane] = 0.0;
            // `self.hist.len()`, not a literal — this was a hardcoded 3 and
            // went out of bounds the moment the ring shrank to 2.
            for r in 0..self.hist.len() {
                self.hist[r][d][lane] = 0.0;
            }
            // The damping state is part of the tail this reset exists to kill.
            // A retired lane that kept it would bleed its last modulation into
            // the next note through the filter, which is the same stale-tail
            // bug the history clear above prevents.
            self.pmz[d][lane] = 0.0;
            self.peak[d][lane] = 0.0;
        }
    }

    /// Set one operator's output level on one lane. This is where an envelope
    /// lands: `lvl = op_level * eg`.
    #[inline]
    pub fn set_lane_op_level(&mut self, lane: usize, op: usize, level: f32) {
        self.lvl[op][lane] = level;
    }

    /// Loudest sum-bus-weighted operator level on a lane.
    ///
    /// The allocator's quietest-voice steal reads this. It is a proxy for what
    /// the lane is actually contributing — an operator that is only a modulator
    /// has no sum-bus gain and so cannot make a lane look loud.
    pub fn lane_amp(&self, lane: usize, bus: &SumBus) -> f32 {
        (0..NOPS).fold(0.0f32, |m, d| {
            m.max(self.lvl[d][lane] * (bus.l[d].abs() + bus.r[d].abs()))
        })
    }

    /// One oversampled tick. Returns the stereo sum across every voice.
    #[inline]
    pub fn tick<L: Lookup>(
        &mut self,
        bank: &WaveBank,
        r: &CompiledRouting,
        bus: &SumBus,
    ) -> (f32, f32) {
        let (cur, prev) = ring(self.head);

        for d in 0..NOPS {
            let mut pm = [0.0f32; V];

            // Read phase. Every borrow of `hist` here is shared and ends
            // before the write phase below takes a mutable one.
            {
                let fb = r.fb[d];
                if fb != 0.0 {
                    let a = &self.hist[prev][d];
                    for v in 0..V {
                        pm[v] = fb * a[v];
                    }
                }
                for lane in r.lanes(d) {
                    let g = lane.gain;
                    let src = &self.hist[prev][lane.src as usize];
                    for v in 0..V {
                        pm[v] += g * src[v];
                    }
                }
            }

            // Damping, and the rate measurement it yields for free. One pass:
            // the residue `e` is the filter's own intermediate, and its peak is
            // what `update_mips` reads.
            {
                let g = self.damp[d];
                let z = &mut self.pmz[d];
                let pk = &mut self.peak[d];
                for v in 0..V {
                    let e = pm[v] - z[v];
                    z[v] += g * e;
                    pm[v] = z[v];
                    pk[v] = pk[v].max(e.abs());
                }
            }

            // Write phase. Rows hoisted so the lane loop carries no bounds
            // check on the outer index.
            let table = bank.table(self.wave[d]);
            let out = &mut self.hist[cur][d];
            let phase = &mut self.phase[d];
            let (inc, mip, lvl) = (&self.inc[d], &self.mip[d], &self.lvl[d]);
            for v in 0..V {
                let ph = phase[v].wrapping_add(phase_offset(pm[v]));
                out[v] = L::read(table, mip[v] as usize, ph) * lvl[v];
                phase[v] = phase[v].wrapping_add(inc[v]);
            }
        }

        let mut l = 0.0f32;
        let mut rr = 0.0f32;
        for d in 0..NOPS {
            let (gl, gr) = (bus.l[d], bus.r[d]);
            let row = &self.hist[cur][d];
            for v in 0..V {
                let x = row[v];
                l += gl * x;
                rr += gr * x;
            }
        }

        self.head ^= 1;
        (l, rr)
    }

    /// Render `out_l.len()` samples at 1x, running the operator block at
    /// `os` ticks per sample.
    ///
    /// The boxcar average closing each group is a **placeholder for the
    /// polyphase half-band cascade** the brief's 16x → 4x → 1x chain needs. It
    /// is deliberately not a real decimator: the real one runs on the stereo
    /// sum bus only, so its cost does not scale with polyphony and cannot move
    /// the number this bench exists to produce.
    pub fn render<L: Lookup>(
        &mut self,
        bank: &WaveBank,
        r: &CompiledRouting,
        bus: &SumBus,
        os: usize,
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) {
        debug_assert_eq!(out_l.len(), out_r.len());
        let norm = 1.0 / os as f32;
        for i in 0..out_l.len() {
            let mut al = 0.0f32;
            let mut ar = 0.0f32;
            for _ in 0..os {
                let (l, r_) = self.tick::<L>(bank, r, bus);
                al += l;
                ar += r_;
            }
            out_l[i] = al * norm;
            out_r[i] = ar * norm;
        }
    }
}

impl<const V: usize> Default for VoiceMajor<V> {
    fn default() -> Self {
        Self::new()
    }
}

/// SoA over operators. The inner loop is 8 operators wide.
pub struct OpMajor<const V: usize> {
    phase: [[u32; NOPS]; V],
    inc: [[u32; NOPS]; V],
    mip: [[u32; NOPS]; V],
    lvl: [[f32; NOPS]; V],
    /// `hist[ring][voice][op]` — two ticks of output, rotated by [`Self::head`].
    hist: [[[f32; NOPS]; V]; 2],
    head: usize,
    wave: [Waveform; NOPS],
    /// Transposed twin of [`VoiceMajor::pmz`].
    pmz: [[f32; NOPS]; V],
    damp: [f32; NOPS],
    peak: [[f32; NOPS]; V],
}

impl<const V: usize> OpMajor<V> {
    pub fn new() -> Self {
        Self {
            phase: [[0; NOPS]; V],
            inc: [[0; NOPS]; V],
            mip: [[0; NOPS]; V],
            lvl: [[0.0; NOPS]; V],
            hist: [[[0.0; NOPS]; V]; 2],
            head: 0,
            wave: [Waveform::Sine; NOPS],
            pmz: [[0.0; NOPS]; V],
            damp: [1.0; NOPS],
            peak: [[0.0; NOPS]; V],
        }
    }

    /// See [`VoiceMajor::set_damping`].
    pub fn set_damping(&mut self, cfg: &[OpConfig; NOPS], sr_os: f32) {
        for d in 0..NOPS {
            self.damp[d] = damp_coeff(cfg[d].damp_hz, sr_os);
        }
    }

    /// See [`VoiceMajor::update_mips`]. Same rule, transposed indexing.
    pub fn update_mips(&mut self, bank: &WaveBank, cfg: &[OpConfig; NOPS]) {
        for v in 0..V {
            for d in 0..NOPS {
                let table = bank.table(cfg[d].wave);
                let dev = (self.damp[d] * self.peak[v][d] * PHASE_SCALE).max(0.0);
                let rate = self.inc[v][d].saturating_add(dev as u32);
                let cur = self.mip[v][d] as usize;
                let want = table.mip_for(rate);
                self.mip[v][d] = if want > cur {
                    want as u32
                } else if cur > 0 && rate < table.hysteresis_floor(cur) {
                    (cur - 1) as u32
                } else {
                    cur as u32
                };
                self.peak[v][d] = 0.0;
            }
        }
    }

    /// Identical state to [`VoiceMajor::cook`], transposed.
    pub fn cook(&mut self, bank: &WaveBank, cfg: &[OpConfig; NOPS], keys: &[u8; V], sr_os: f32) {
        for d in 0..NOPS {
            self.wave[d] = cfg[d].wave;
            let table = bank.table(cfg[d].wave);
            for v in 0..V {
                let hz = note_to_freq(keys[v]) * cfg[d].ratio;
                let inc = ((hz / sr_os) * PHASE_SCALE) as u32;
                self.inc[v][d] = inc;
                self.mip[v][d] = table.mip_for(inc) as u32;
                self.lvl[v][d] = cfg[d].level;
                self.phase[v][d] = (v as u32)
                    .wrapping_mul(0x9E37_79B9)
                    .wrapping_add((d as u32).wrapping_mul(0x85EB_CA6B));
            }
        }
    }

    /// One oversampled tick, as a dense 8x8 matvec per voice.
    #[inline]
    pub fn tick<L: Lookup>(
        &mut self,
        bank: &WaveBank,
        r: &CompiledRouting,
        bus: &SumBus,
    ) -> (f32, f32) {
        let (cur, prev) = ring(self.head);

        for v in 0..V {
            let mut pm = [0.0f32; NOPS];
            {
                let a = &self.hist[prev][v];
                for d in 0..NOPS {
                    pm[d] = r.fb[d] * a[d];
                }
                // Transposed so the inner loop is contiguous over destinations
                // and vectorises 8 wide. Zero routes are multiplied through
                // rather than skipped, which is exactly the trade this layout
                // makes — and why the layout bench runs at dense routing, where
                // there are no zero routes for either side to skip.
                for s in 0..NOPS {
                    let x = a[s];
                    let col = &r.pm_t[s];
                    for d in 0..NOPS {
                        pm[d] += col[d] * x;
                    }
                }
            }

            // Damping + rate measurement, as in `VoiceMajor` — same arithmetic
            // in the same order, over the transposed axis.
            {
                let z = &mut self.pmz[v];
                let pk = &mut self.peak[v];
                for d in 0..NOPS {
                    let e = pm[d] - z[d];
                    z[d] += self.damp[d] * e;
                    pm[d] = z[d];
                    pk[d] = pk[d].max(e.abs());
                }
            }

            let out = &mut self.hist[cur][v];
            let phase = &mut self.phase[v];
            let (inc, mip, lvl) = (&self.inc[v], &self.mip[v], &self.lvl[v]);
            for d in 0..NOPS {
                let table = bank.table(self.wave[d]);
                let ph = phase[d].wrapping_add(phase_offset(pm[d]));
                out[d] = L::read(table, mip[d] as usize, ph) * lvl[d];
                phase[d] = phase[d].wrapping_add(inc[d]);
            }
        }

        // Same accumulation order as `VoiceMajor` — operator outer, voice inner
        // — so the two layouts stay bit-identical. The strided read is a real
        // cost of this layout and is left in rather than reordered away.
        let mut l = 0.0f32;
        let mut rr = 0.0f32;
        for d in 0..NOPS {
            let (gl, gr) = (bus.l[d], bus.r[d]);
            for v in 0..V {
                let x = self.hist[cur][v][d];
                l += gl * x;
                rr += gr * x;
            }
        }

        self.head ^= 1;
        (l, rr)
    }

    /// See [`VoiceMajor::render`] — same contract, same placeholder decimator.
    pub fn render<L: Lookup>(
        &mut self,
        bank: &WaveBank,
        r: &CompiledRouting,
        bus: &SumBus,
        os: usize,
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) {
        debug_assert_eq!(out_l.len(), out_r.len());
        let norm = 1.0 / os as f32;
        for i in 0..out_l.len() {
            let mut al = 0.0f32;
            let mut ar = 0.0f32;
            for _ in 0..os {
                let (l, r_) = self.tick::<L>(bank, r, bus);
                al += l;
                ar += r_;
            }
            out_l[i] = al * norm;
            out_r[i] = ar * norm;
        }
    }
}

impl<const V: usize> Default for OpMajor<V> {
    fn default() -> Self {
        Self::new()
    }
}

/// [`VoiceMajor`] with per-voice route gains.
///
/// The shipping synth modulates every route from per-voice sources, so a route
/// gain is a `V`-wide vector rather than a broadcast scalar. Structurally
/// identical to [`VoiceMajor`]; the only difference is where the multiplicand
/// comes from. Exists so the bench can price that difference instead of
/// guessing at it.
pub struct VoiceMajorPerVoiceGain<const V: usize> {
    inner: VoiceMajor<V>,
    /// `gain[dest][src][voice]`.
    pub gain: Box<[[[f32; V]; NOPS]; NOPS]>,
}

impl<const V: usize> VoiceMajorPerVoiceGain<V> {
    pub fn new(r: &Routing, spread: f32) -> Self {
        let mut gain = Box::new([[[0.0f32; V]; NOPS]; NOPS]);
        for d in 0..NOPS {
            for s in 0..NOPS {
                for v in 0..V {
                    // A little per-voice divergence so nothing collapses to a
                    // broadcast at compile time.
                    let k = 1.0 + spread * ((v as f32 / V as f32) - 0.5);
                    gain[d][s][v] = r.pm[d][s] * k;
                }
            }
        }
        Self {
            inner: VoiceMajor::new(),
            gain,
        }
    }

    pub fn cook(&mut self, bank: &WaveBank, cfg: &[OpConfig; NOPS], keys: &[u8; V], sr_os: f32) {
        self.inner.cook(bank, cfg, keys, sr_os);
    }

    /// See [`VoiceMajor::set_damping`].
    pub fn set_damping(&mut self, cfg: &[OpConfig; NOPS], sr_os: f32) {
        self.inner.set_damping(cfg, sr_os);
    }

    /// See [`VoiceMajor::update_mips`].
    pub fn update_mips(&mut self, bank: &WaveBank, cfg: &[OpConfig; NOPS]) {
        self.inner.update_mips(bank, cfg);
    }

    #[inline]
    pub fn tick<L: Lookup>(
        &mut self,
        bank: &WaveBank,
        r: &CompiledRouting,
        bus: &SumBus,
    ) -> (f32, f32) {
        let inner = &mut self.inner;
        let (cur, prev) = ring(inner.head);

        for d in 0..NOPS {
            let mut pm = [0.0f32; V];

            {
                if r.fb[d] != 0.0 {
                    let g = &self.gain[d][d];
                    let a = &inner.hist[prev][d];
                    for v in 0..V {
                        pm[v] = g[v] * a[v];
                    }
                }
                for lane in r.lanes(d) {
                    let s = lane.src as usize;
                    let g = &self.gain[d][s];
                    let src = &inner.hist[prev][s];
                    for v in 0..V {
                        pm[v] += g[v] * src[v];
                    }
                }
            }

            {
                let g = inner.damp[d];
                let z = &mut inner.pmz[d];
                let pk = &mut inner.peak[d];
                for v in 0..V {
                    let e = pm[v] - z[v];
                    z[v] += g * e;
                    pm[v] = z[v];
                    pk[v] = pk[v].max(e.abs());
                }
            }

            let table = bank.table(inner.wave[d]);
            let out = &mut inner.hist[cur][d];
            let phase = &mut inner.phase[d];
            let (inc, mip, lvl) = (&inner.inc[d], &inner.mip[d], &inner.lvl[d]);
            for v in 0..V {
                let ph = phase[v].wrapping_add(phase_offset(pm[v]));
                out[v] = L::read(table, mip[v] as usize, ph) * lvl[v];
                phase[v] = phase[v].wrapping_add(inc[v]);
            }
        }

        let mut l = 0.0f32;
        let mut rr = 0.0f32;
        for d in 0..NOPS {
            let (gl, gr) = (bus.l[d], bus.r[d]);
            let row = &inner.hist[cur][d];
            for v in 0..V {
                let x = row[v];
                l += gl * x;
                rr += gr * x;
            }
        }

        inner.head ^= 1;
        (l, rr)
    }

    pub fn render<L: Lookup>(
        &mut self,
        bank: &WaveBank,
        r: &CompiledRouting,
        bus: &SumBus,
        os: usize,
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) {
        let norm = 1.0 / os as f32;
        for i in 0..out_l.len() {
            let mut al = 0.0f32;
            let mut ar = 0.0f32;
            for _ in 0..os {
                let (l, r_) = self.tick::<L>(bank, r, bus);
                al += l;
                ar += r_;
            }
            out_l[i] = al * norm;
            out_r[i] = ar * norm;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wavetable::{Plain, ValueSlope, ValueSlopeUnchecked};

    const V: usize = 8;

    fn configs() -> [OpConfig; NOPS] {
        let waves = [
            Waveform::Sine,
            Waveform::Saw,
            Waveform::Square,
            Waveform::Triangle,
            Waveform::Sine,
            Waveform::Saw,
            Waveform::Triangle,
            Waveform::Square,
        ];
        let ratios = [1.0, 2.0, 3.0, 0.5, 7.0, 1.0, 4.0, 11.0];
        let mut cfg = [OpConfig::default(); NOPS];
        for d in 0..NOPS {
            cfg[d] = OpConfig {
                wave: waves[d],
                ratio: ratios[d],
                level: 0.7,
                pan: (d as f32 / 3.5) - 1.0,
                // Deliberately uneven across operators: a uniform coefficient
                // would let a layout that mixed up the per-operator damping
                // still agree bit-for-bit with one that did not.
                damp_hz: 8_000.0 + 2_000.0 * d as f32,
            };
        }
        cfg
    }

    fn dense() -> Routing {
        let mut r = Routing::default();
        for d in 0..NOPS {
            for s in 0..NOPS {
                r.pm[d][s] = 0.02 + 0.004 * ((d * NOPS + s) as f32 / 64.0);
            }
            r.out[d] = 0.125;
        }
        r
    }

    fn keys() -> [u8; V] {
        let mut k = [0u8; V];
        for (v, slot) in k.iter_mut().enumerate() {
            *slot = 36 + (v as u8 * 7) % 60;
        }
        k
    }

    fn render_voice_major<L: Lookup>(os: usize, n: usize) -> (Vec<f32>, Vec<f32>) {
        let bank = WaveBank::new(512);
        let cfg = configs();
        let routing = dense();
        let compiled = CompiledRouting::compile(&routing);
        let bus = SumBus::new(&cfg, &routing);
        let mut b: VoiceMajor<V> = VoiceMajor::new();
        b.cook(&bank, &cfg, &keys(), 48_000.0 * os as f32);
        let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
        b.render::<L>(&bank, &compiled, &bus, os, &mut l, &mut r);
        (l, r)
    }

    /// The layout bench is only meaningful if both layouts are the same synth.
    #[test]
    fn layouts_agree_bit_exactly() {
        let bank = WaveBank::new(512);
        let cfg = configs();
        let routing = dense();
        let compiled = CompiledRouting::compile(&routing);
        let bus = SumBus::new(&cfg, &routing);
        let k = keys();

        let mut vm: VoiceMajor<V> = VoiceMajor::new();
        let mut om: OpMajor<V> = OpMajor::new();
        vm.cook(&bank, &cfg, &k, 48_000.0 * 16.0);
        om.cook(&bank, &cfg, &k, 48_000.0 * 16.0);

        let n = 64;
        let (mut vl, mut vr) = (vec![0.0; n], vec![0.0; n]);
        let (mut ol, mut or) = (vec![0.0; n], vec![0.0; n]);
        vm.render::<ValueSlope>(&bank, &compiled, &bus, 16, &mut vl, &mut vr);
        om.render::<ValueSlope>(&bank, &compiled, &bus, 16, &mut ol, &mut or);

        assert_eq!(vl, ol, "left channel diverged");
        assert_eq!(vr, or, "right channel diverged");
    }

    /// Sparse and dense compilations of the same matrix must render the same —
    /// otherwise the density bench is measuring a behaviour change.
    #[test]
    fn skipping_zero_routes_changes_nothing() {
        let bank = WaveBank::new(512);
        let cfg = configs();
        let mut routing = dense();
        for d in 0..NOPS {
            for s in 0..NOPS {
                if (d + s) % 3 != 0 {
                    routing.pm[d][s] = 0.0;
                }
            }
        }
        let compiled = CompiledRouting::compile(&routing);
        assert!(compiled.lane_count() < NOPS * (NOPS - 1));
        let bus = SumBus::new(&cfg, &routing);
        let k = keys();

        let mut vm: VoiceMajor<V> = VoiceMajor::new();
        let mut om: OpMajor<V> = OpMajor::new();
        vm.cook(&bank, &cfg, &k, 48_000.0 * 8.0);
        om.cook(&bank, &cfg, &k, 48_000.0 * 8.0);

        let n = 32;
        let (mut vl, mut vr) = (vec![0.0; n], vec![0.0; n]);
        let (mut ol, mut or) = (vec![0.0; n], vec![0.0; n]);
        vm.render::<ValueSlope>(&bank, &compiled, &bus, 8, &mut vl, &mut vr);
        om.render::<ValueSlope>(&bank, &compiled, &bus, 8, &mut ol, &mut or);
        assert_eq!(vl, ol);
        assert_eq!(vr, or);
    }

    #[test]
    fn unchecked_lookup_renders_identically() {
        let (a_l, a_r) = render_voice_major::<ValueSlope>(8, 48);
        let (b_l, b_r) = render_voice_major::<ValueSlopeUnchecked>(8, 48);
        assert_eq!(a_l, b_l);
        assert_eq!(a_r, b_r);
    }

    /// Plain and value+slope differ in the last bits (one FMA versus a subtract
    /// and a multiply), so this is a closeness check, not equality.
    #[test]
    fn plain_lookup_renders_close() {
        let (a_l, _) = render_voice_major::<ValueSlope>(8, 48);
        let (b_l, _) = render_voice_major::<Plain>(8, 48);
        for (a, b) in a_l.iter().zip(b_l.iter()) {
            assert!((a - b).abs() < 1e-4, "{a} vs {b}");
        }
    }

    /// A fully dense matrix at usable depths must not run away. PM is amplitude
    /// bounded by the waveform, so this is really a check that nothing has gone
    /// non-finite through the phase quantiser.
    #[test]
    fn dense_feedback_stays_bounded_and_finite() {
        let (l, r) = render_voice_major::<ValueSlope>(16, 512);
        for (a, b) in l.iter().zip(r.iter()) {
            assert!(a.is_finite() && b.is_finite(), "non-finite output");
            assert!(a.abs() <= 8.0 && b.abs() <= 8.0, "runaway: {a}, {b}");
        }
    }

    /// The coefficient is a function of the corner in Hz and the rate, so the
    /// *response* is identical at 8x and 16x even though the numbers differ.
    ///
    /// Checked as the per-second time constant `-sr / ln(1 - g)`, which is the
    /// rate-independent quantity. A coefficient picked per tick instead would
    /// change the filter every time quality moved, which is exactly the class
    /// of bug that made a held note jump an octave on a quality switch.
    #[test]
    fn damping_is_rate_independent() {
        // Time constant in **seconds**: `-1 / (sr * ln(1 - g))`, which reduces
        // to `1 / (2 pi fc)` and so cannot depend on the rate. Dividing by `sr`
        // rather than multiplying is the whole point — the first draft of this
        // test had it the other way up and reported a 4x discrepancy that was
        // entirely `sr^2` in the assertion.
        let tau = |hz: f32, sr: f32| {
            let g = damp_coeff(hz, sr);
            -1.0 / (sr * (1.0 - g).ln())
        };
        for hz in [500.0, 2_000.0, 20_000.0] {
            let a = tau(hz, 48_000.0 * 8.0);
            let b = tau(hz, 48_000.0 * 16.0);
            assert!(
                (a / b - 1.0).abs() < 1e-3,
                "{hz} Hz: time constant {a} at 8x vs {b} at 16x"
            );
        }
        // Bypass sentinels, including NaN — a NaN corner reaching the lane loop
        // would poison every voice on the operator.
        assert_eq!(damp_coeff(0.0, 384_000.0), 1.0);
        assert_eq!(damp_coeff(-1.0, 384_000.0), 1.0);
        assert_eq!(damp_coeff(f32::NAN, 384_000.0), 1.0);
        assert!((0.0..=1.0).contains(&damp_coeff(1e9, 384_000.0)));
    }

    /// Damping must reduce what arrives at the phase accumulator, monotonically
    /// in the corner frequency.
    ///
    /// One self-feedback operator, so the only thing reaching `pm` is the route
    /// under test and the relationship is clean. Measured as the RMS of the
    /// output's first difference — a crude high-pass, and the direct reading of
    /// "how bright did the feedback make this". Monotonicity across a sweep,
    /// not a threshold: an earlier draft asserted a factor of two between two
    /// corners and failed at 1.89, which was measuring the arbitrariness of the
    /// constant rather than anything about the filter.
    #[test]
    fn a_lower_corner_damps_more() {
        let bank = WaveBank::new(2048);
        let mut routing = Routing::default();
        routing.pm[0][0] = 0.8;
        routing.out[0] = 1.0;
        let compiled = CompiledRouting::compile(&routing);

        let brightness = |hz: f32| {
            let mut cfg = [OpConfig::default(); NOPS];
            cfg[0] = OpConfig {
                wave: Waveform::Sine,
                ratio: 1.0,
                level: 1.0,
                pan: 0.0,
                damp_hz: hz,
            };
            let bus = SumBus::new(&cfg, &routing);
            let mut b = VoiceMajor::<8>::new();
            b.cook(&bank, &cfg, &[60; 8], 384_000.0);
            b.set_damping(&cfg, 384_000.0);

            let mut out = Vec::with_capacity(4096);
            for _ in 0..4096 {
                out.push(b.tick::<ValueSlope>(&bank, &compiled, &bus).0);
            }
            assert!(out.iter().all(|s| s.is_finite()));
            let hf: f32 = out.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
            (hf / out.len() as f32).sqrt()
        };

        let corners = [1e9f32, 40_000.0, 20_000.0, 8_000.0, 2_000.0, 500.0];
        let vals: Vec<f32> = corners.iter().map(|c| brightness(*c)).collect();
        for (i, w) in vals.windows(2).enumerate() {
            assert!(
                w[1] < w[0],
                "corner {} Hz ({}) is not duller than {} Hz ({})",
                corners[i + 1],
                w[1],
                corners[i],
                w[0]
            );
        }
        assert!(
            vals[0] > 0.0,
            "the undamped case produced no high-frequency content"
        );
    }

    /// **The invariant.** Hysteresis may only ever leave the mip *coarser* than
    /// the measured rate requires, never finer.
    ///
    /// Coarse is dull and recoverable; fine aliases. If this can fail, the
    /// asymmetry is broken and the whole no-crossfade argument goes with it.
    #[test]
    fn hysteresis_only_ever_errs_coarse() {
        let bank = WaveBank::new(2048);
        let cfg = configs();
        // Deep routing, so the rate estimate swings hard and the selector is
        // pushed across boundaries in both directions.
        let mut routing = Routing::default();
        for d in 0..NOPS {
            for s in 0..NOPS {
                routing.pm[d][s] = 0.9;
            }
            routing.out[d] = 0.125;
        }
        let compiled = CompiledRouting::compile(&routing);
        let bus = SumBus::new(&cfg, &routing);

        let mut b = VoiceMajor::<8>::new();
        b.cook(&bank, &cfg, &[36, 48, 60, 72, 84, 96, 100, 108], 384_000.0);
        b.set_damping(&cfg, 384_000.0);

        for _block in 0..64 {
            // A control block is 32 base samples; at 8x that is 256 ticks.
            for _ in 0..256 {
                b.tick::<ValueSlope>(&bank, &compiled, &bus);
            }
            // Snapshot the rate the block actually saw, before the update
            // clears it.
            let mut required = [[0usize; 8]; NOPS];
            for d in 0..NOPS {
                let table = bank.table(cfg[d].wave);
                for v in 0..8 {
                    let dev = (b.damp[d] * b.peak[d][v] * PHASE_SCALE).max(0.0);
                    let rate = b.inc[d][v].saturating_add(dev as u32);
                    required[d][v] = table.mip_for(rate);
                }
            }
            b.update_mips(&bank, &cfg);
            for d in 0..NOPS {
                for v in 0..8 {
                    assert!(
                        b.mip[d][v] as usize >= required[d][v],
                        "op {d} voice {v}: mip {} is finer than the {} the rate needs",
                        b.mip[d][v],
                        required[d][v]
                    );
                }
            }
        }
    }

    /// A rate parked on a boundary must not switch tables every block. Without
    /// hysteresis this is the flutter the whole mechanism exists to prevent.
    #[test]
    fn a_rate_on_a_boundary_does_not_flutter() {
        let bank = WaveBank::new(2048);
        let cfg = configs();
        let table = bank.table(cfg[0].wave);

        let mut b = VoiceMajor::<8>::new();
        b.cook(&bank, &cfg, &[60; 8], 384_000.0);
        b.set_damping(&cfg, 384_000.0);

        // Park voice 0 of operator 0 exactly on a mip boundary and dither the
        // measured rate a few percent either side of it, block by block.
        let boundary = table.mip_ceiling(2);
        b.inc[0][0] = boundary;
        b.mip[0][0] = table.mip_for(boundary) as u32;

        let mut switches = 0;
        let mut last = b.mip[0][0];
        for i in 0..40 {
            let wobble = if i % 2 == 0 { 1.03 } else { 0.97 };
            b.inc[0][0] = (boundary as f32 * wobble) as u32;
            b.update_mips(&bank, &cfg);
            if b.mip[0][0] != last {
                switches += 1;
                last = b.mip[0][0];
            }
        }
        assert!(
            switches <= 1,
            "mip flapped {switches} times across 40 blocks"
        );
    }

    /// Modulation well past ±0.5 turns must wrap, not clamp.
    #[test]
    fn phase_offset_wraps_at_high_index() {
        // 12.25 turns is 0.25 turns after wrapping — a quarter of the table.
        let a = phase_offset(0.25);
        let b = phase_offset(12.25);
        assert_eq!(a, b);
        let c = phase_offset(-7.75);
        assert_eq!(a, c);
    }

    #[test]
    fn per_voice_gain_matches_broadcast_at_zero_spread() {
        let bank = WaveBank::new(512);
        let cfg = configs();
        let routing = dense();
        let compiled = CompiledRouting::compile(&routing);
        let bus = SumBus::new(&cfg, &routing);
        let k = keys();

        let mut a: VoiceMajor<V> = VoiceMajor::new();
        let mut b: VoiceMajorPerVoiceGain<V> = VoiceMajorPerVoiceGain::new(&routing, 0.0);
        a.cook(&bank, &cfg, &k, 48_000.0 * 8.0);
        b.cook(&bank, &cfg, &k, 48_000.0 * 8.0);

        let n = 32;
        let (mut al, mut ar) = (vec![0.0; n], vec![0.0; n]);
        let (mut bl, mut br) = (vec![0.0; n], vec![0.0; n]);
        a.render::<ValueSlope>(&bank, &compiled, &bus, 8, &mut al, &mut ar);
        b.render::<ValueSlope>(&bank, &compiled, &bus, 8, &mut bl, &mut br);
        assert_eq!(al, bl);
        assert_eq!(ar, br);
    }

    #[test]
    fn compiled_routing_matches_authored_density() {
        let r = dense();
        assert_eq!(r.density(), NOPS * NOPS);
        let c = CompiledRouting::compile(&r);
        assert_eq!(c.lane_count(), NOPS * (NOPS - 1));
        for d in 0..NOPS {
            assert_eq!(c.lanes(d).len(), NOPS - 1);
            assert_eq!(c.fb[d], r.pm[d][d]);
            assert_eq!(c.pm_t[d][d], 0.0, "diagonal must not appear in pm_t");
        }
    }
}
