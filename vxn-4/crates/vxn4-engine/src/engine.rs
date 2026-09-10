//! Top-level engine: operator banks, the oversampling chain, and the limiter.
//!
//! ## Rate plan
//!
//! ```text
//!   operators  ──8x──▶  s8  ──4x──▶  limiter  ──4x──▶  s4 ──2x──▶ s2 ──1x──▶ out
//!              ──16x─▶  s16 ──8x──▶  s8  ──▶ (as above)
//! ```
//!
//! Generation runs at 8x or 16x ([`Quality`]); everything from the limiter down
//! runs at 4x, then decimates to 1x. Each [`HalfbandFir`] stage is named for its
//! *input* rate and only ever sees that rate — `s8` takes 8x in both qualities,
//! because at 16x the `s16` stage has already halved it. Sharing one stage
//! across two rates would leave its filter state incoherent across a quality
//! switch.
//!
//! The FX block the brief places at 4x is not here yet; the limiter occupies
//! that slot so the chain shape is real and FX can drop in beside it.
//!
//! ## Why banks rather than 20 independent voices
//!
//! The sizing bench found SIMD-across-voices to beat SIMD-across-operators by
//! 15–24%, the win coming from lanes sharing a waveform table and adjacent
//! mips. So the 20 slots are three [`VoiceMajor`] banks of 8 lanes (24 lanes,
//! 4 unused), all running one patch — which is also why route gains can stay
//! broadcast scalars, with only per-operator *level* varying per lane. That is
//! exactly where the envelopes land.
//!
//! **That measurement has since inverted**: with the damping pass in and the
//! output ring down to 2 deep, op-major wins the *dense* case by 13% at V=8.
//! The engine stays on `VoiceMajor` anyway, because the bench measures dense
//! routing and five of the six patches are sparse — op-major multiplies zero
//! routes through where voice-major skips them, and sparsity is worth ~24%.
//! The sparse arm has never been measured for op-major. See the README's
//! layout section; do not act on the dense number alone.
//!
//! A bank is skipped wholesale when all 8 of its lanes are idle, so the common
//! case of a few notes held costs one bank, not three.

use vxn_core_matrix::eval::eval_dests;
use vxn_core_utils::halfband::HalfbandFir;
use vxn_core_utils::limiter::StereoLimiter;

use vxn4_dsp::ops::{CompiledRouting, NOPS, Routing, SumBus, VoiceMajor};
use vxn4_dsp::wavetable::{ValueSlope, WaveBank};

use crate::alloc::{Action, Alloc, N_SLOTS, Phase};
use crate::matrix::{
    DestId, Matrix, N_DESTS, N_MACROS, Roster, SourceId, damp_dest_index, out_dest_index,
    pan_dest_index, pm_dest_index, ratio_dest_index, spread_dest_index,
};
use crate::patch::{Patch, patch};

/// Lanes per bank. 8 is what the sizing sweep found best for `VoiceMajor`; 4
/// and 16 both measure worse (49.8 / 52.0 / 48.9 voices, dense at 16x).
pub const LANES: usize = 8;

/// Banks needed to cover [`N_SLOTS`].
pub const N_BANKS: usize = N_SLOTS.div_ceil(LANES);

/// Control-rate period in samples at 1x. Envelopes tick once per control block.
pub const CONTROL_PERIOD: usize = 32;

/// Limiter ceiling. Well below unity, for three measured reasons.
///
/// The brief puts the limiter at 4x, so **two halfband stages run after it**.
/// That placement costs more than it looks like it should, and three separate
/// effects stack up — each measured here, none of them visible to a steady-tone
/// test:
///
/// 1. **The limiter overshoots its own threshold on complex material.**
///    `LimiterCore` smooths its gain with a one-pole, which lags a beating
///    waveform. Threshold 0.5 measured 0.582 out; threshold 0.89 measured
///    0.979 — 12–19% over. Against a constant-amplitude sine it holds its
///    threshold to four decimals, which is why this does not show up in the
///    limiter's own tests. Its docs are straight about the intent: a "safety
///    limiter, not a true-peak mastering meter", whose hard guarantee is the
///    `±1` clamp rather than the threshold.
///
/// 2. **Decimation exposes inter-sample peaks.** `StereoLimiter` detects
///    sample-peak only and explicitly leaves inter-sample overshoot downstream.
///    Limiting at 4x and then resampling to 1x lands samples nearer the true
///    continuous peak, so the peaks it declined to detect become real ones.
///
/// 3. **A hard onset clips before the gain converges.** `current_gain` starts
///    at 1.0 with a 2 ms one-pole attack, which cannot travel down to ~0.2
///    inside its 2 ms lookahead. A loud chord arriving in one sample is
///    therefore hard-clipped by the limiter's own `±1`, and the halfbands ring
///    on the squared edges to ~1.02 at 1x. This one is **independent of the
///    ceiling** — sweeping 0.70..0.89 moved the worst-case 1x peak only between
///    1.021 and 1.028 — which is what proves it is clipping and not gain
///    staging.
///
/// So the ceiling is not the whole answer and cannot be. The patches are gain
/// staged (see `Patch::gain`) so that ordinary playing never drives the limiter
/// hard enough for (3), the ceiling here absorbs (1) and (2), and
/// [`Engine::process`] clamps at 1x as a backstop for a fortissimo cluster.
///
/// **The architectural point stands**: a limiter upstream of a resampler cannot
/// be a brickwall. Either it moves to 1x, last in the chain — which is what
/// vxn-1b and vxn-2 do — or it gains true-peak detection. The brief wants FX at
/// 4x with the limiter after them, so this is a live design question.
const CEILING: f32 = 0.80;

/// Bounds on a **modulated** damping corner, in Hz.
///
/// The floor is the load-bearing one. A one-pole at DC has a coefficient of
/// zero, which freezes `pm` at whatever it last held — every route into that
/// operator silently stops working while still costing what it costs. 20 Hz is
/// below anything musical and safely above that cliff.
///
/// The ceiling is above any oversampled Nyquist in play (768 kHz at 16x), so a
/// knob at the bright end reads as bypass rather than as a filter with an
/// arbitrary limit.
pub const MIN_DAMP_HZ: f32 = 20.0;
/// See [`MIN_DAMP_HZ`].
pub const MAX_DAMP_HZ: f32 = 1_000_000.0;

/// Upper bound on [`Engine::set_master_gain`].
///
/// Above unity because the patches are staged to leave the limiter alone at
/// ordinary velocities (see `Patch::gain`), so a player who wants the limiter
/// working needs somewhere to go. +6 dB is enough to drive it audibly and not
/// enough to make the onset clipping in `CEILING`'s note (3) the normal case.
pub const MAX_MASTER_GAIN: f32 = 2.0;

/// Mip-0 wavetable length.
///
/// The sizing sweep found table length to have **no** measurable effect on
/// throughput across a 256..2048 span, so this is chosen purely for quality:
/// 2048 is the longest, costs nothing, and 129 KiB of bank is irrelevant when
/// it is shared across every voice.
pub const TABLE_LEN: usize = 2048;

/// Operator-block oversampling. Switchable at runtime; everything below the
/// limiter runs at 4x regardless.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Quality {
    /// 8x generation. ~109 voices of headroom on one core.
    #[default]
    X8,
    /// 16x generation. Half the headroom, for when 8x audibly aliases.
    X16,
}

impl Quality {
    pub const fn factor(self) -> usize {
        match self {
            Quality::X8 => 8,
            Quality::X16 => 16,
        }
    }

    /// Ticks of the operator block per 4x sample.
    const fn ticks_per_4x(self) -> usize {
        self.factor() / 4
    }
}

/// One decimating channel: the full 16x→1x cascade, with the 4x tap exposed.
///
/// Stages are named for their input rate and each is only ever fed that rate.
struct Chain {
    s16: HalfbandFir,
    s8: HalfbandFir,
    s4: HalfbandFir,
    s2: HalfbandFir,
}

impl Chain {
    fn new() -> Self {
        Self {
            s16: HalfbandFir::default(),
            s8: HalfbandFir::default(),
            s4: HalfbandFir::default(),
            s2: HalfbandFir::default(),
        }
    }

    fn reset(&mut self) {
        self.s16.reset();
        self.s8.reset();
        self.s4.reset();
        self.s2.reset();
    }

    /// Fold one 4x sample's worth of oversampled ticks down to 4x.
    ///
    /// Named `fold_*` rather than `to_*` because these consume filter state and
    /// mutate `self`; clippy reads a `to_*` on a non-`Copy` type as a cheap
    /// conversion, which this is the opposite of.
    #[inline]
    fn fold_to_4x(&mut self, ticks: &[f32], q: Quality) -> f32 {
        match q {
            Quality::X8 => self.s8.process(ticks[0], ticks[1]),
            Quality::X16 => {
                let a = self.s16.process(ticks[0], ticks[1]);
                let b = self.s16.process(ticks[2], ticks[3]);
                self.s8.process(a, b)
            }
        }
    }

    /// 4x → 1x. Consumes four 4x samples.
    #[inline]
    fn fold_to_1x(&mut self, x: [f32; 4]) -> f32 {
        let a = self.s4.process(x[0], x[1]);
        let b = self.s4.process(x[2], x[3]);
        self.s2.process(a, b)
    }
}

/// The latency a **host** is told, in samples: the worst case across qualities.
///
/// Real latency is 14 at 8x and 15 at 16x, and quality is a live parameter, so
/// an exact report would have to change mid-session — which means a main-thread
/// `latency.changed()` from an audio-thread parameter write, and a host free to
/// re-plan its graph whenever a knob moves.
///
/// Reporting the constant worst case buys that away for a **one-sample** error
/// at 8x: 21 µs at 48 kHz, well under any host's own scheduling jitter. The
/// alternative that would be exact — padding the 8x path by a sample so the
/// figure is true in both modes — is a delay line and a state reset for 21 µs,
/// and is not worth it.
///
/// Named separately from [`latency_samples`] so the per-quality truth stays
/// available and this stays visibly a *choice* rather than a wrong constant.
pub const HOST_LATENCY_SAMPLES: u32 = latency_samples(Quality::X16);

/// Base-rate latency of the decimation chain, in samples.
///
/// Each halfband contributes 16 samples of group delay *at its own input rate*,
/// so a stage running at Nx costs `16 / N` base-rate samples.
pub const fn latency_samples(q: Quality) -> u32 {
    // s8: 16/8 = 2, s4: 16/4 = 4, s2: 16/2 = 8.
    let base = 2 + 4 + 8;
    match q {
        // s16 adds 16/16 = 1.
        Quality::X16 => base + 1,
        Quality::X8 => base,
    }
}

/// Which PM routes the matrix can reach, whether or not the patch authors them.
///
/// [`CompiledRouting::compile_with`] takes this so the lane set is fixed for the
/// life of the patch and `set_pm` can update depths on the audio thread without
/// reallocating. A route the mask misses has no lane, and modulation into it is
/// silently dropped — which is exactly the failure `sine` would hit.
///
/// Reads `is_wired`, not `is_active`: a slot switched off still needs its lane
/// reserved, or arming it mid-note would need a recompile.
fn force_mask(m: &Matrix) -> [[bool; NOPS]; NOPS] {
    let mut force = [[false; NOPS]; NOPS];
    for slot in &m.slots {
        if !slot.is_wired() {
            continue;
        }
        let Some(di) = DestId::idx(slot.dest) else {
            continue;
        };
        if di < NOPS * NOPS {
            force[di / NOPS][di % NOPS] = true;
        }
    }
    force
}

pub struct Engine {
    sample_rate: f32,
    quality: Quality,
    patch_index: usize,
    patch: Patch,

    waves: WaveBank,
    banks: [VoiceMajor<LANES>; N_BANKS],
    routing: CompiledRouting,
    bus: SumBus,

    /// The eight macro knobs — the only modulation sources the host can see.
    macros: [f32; N_MACROS],
    /// Per-destination totals from the last matrix evaluation.
    dests: [f32; N_DESTS],
    /// Constant-power pan factors, one pair per operator. Cached at patch
    /// change so rebuilding the sum bus after a macro move is eight multiplies
    /// rather than eight `sin`/`cos` pairs.
    pan_c: [f32; NOPS],
    pan_s: [f32; NOPS],
    /// Set when a macro or the patch has moved; cleared once the totals have
    /// been pushed into the routing. Nothing else writes the routing, so a
    /// clean flag means the compiled depths are already correct.
    mod_dirty: bool,
    /// Whether the patch has any active slot at all. False for a patch with an
    /// empty table, and the whole modulation path is then skipped.
    modulated: bool,
    /// Whether any active slot targets a `Damp` destination.
    ///
    /// Separate from [`Self::modulated`] because resolving damping costs eight
    /// `exp` calls per bank, and a patch whose macros only touch PM depths
    /// should not pay them on every knob move. [`Self::detuned`] and
    /// [`Self::panned`] earn their own flags for the same reason — detune
    /// re-derives an increment for every sounding lane, which is the most
    /// expensive of the three.
    damp_routed: bool,
    /// Whether any active slot targets a `Ratio` destination.
    detuned: bool,
    /// Whether any active slot targets a `Pan` destination.
    panned: bool,
    /// Whether any active slot targets a `Spread` destination.
    spread_routed: bool,
    /// Per-operator ratios with the detune total already folded in.
    ///
    /// Cached because `cook_lane` derives an increment from the patch's
    /// *nominal* ratio, so every note-on and every quality switch would
    /// otherwise wipe the detune — and `mod_dirty` is already clear by then, so
    /// nothing would put it back. A note played after the knob moved would come
    /// out at concert pitch while the ones already sounding stayed detuned.
    live_ratios: [f32; NOPS],

    alloc: Alloc,

    left: Chain,
    right: Chain,
    limiter: StereoLimiter,

    /// 4x samples awaiting the final 4→1 fold.
    quad_l: [f32; 4],
    quad_r: [f32; 4],

    /// Samples until the next control tick.
    control_countdown: usize,

    /// Output trim, multiplied in **before** the limiter.
    ///
    /// Before, not after, because the limiter is the only thing standing
    /// between this and full scale — a trim applied downstream of it would put
    /// the user's knob outside the one guarantee the chain makes. See
    /// [`CEILING`].
    master_gain: f32,
}

impl Engine {
    pub fn new(sample_rate: f32) -> Self {
        let p = patch(0);
        // The limiter sits at 4x, so that is the rate it must be told about —
        // its lookahead and release are in samples.
        let mut limiter = StereoLimiter::new(sample_rate * 4.0);
        limiter.set_threshold(CEILING);
        let mut e = Self {
            sample_rate,
            quality: Quality::default(),
            patch_index: 0,
            routing: CompiledRouting::compile(&p.routing),
            bus: SumBus::new(&p.ops, &p.routing),
            patch: p,
            waves: WaveBank::new(TABLE_LEN),
            banks: [(); N_BANKS].map(|_| VoiceMajor::<LANES>::new()),
            macros: [0.0; N_MACROS],
            dests: [0.0; N_DESTS],
            pan_c: [1.0; NOPS],
            pan_s: [1.0; NOPS],
            mod_dirty: true,
            modulated: false,
            damp_routed: false,
            detuned: false,
            panned: false,
            spread_routed: false,
            live_ratios: [1.0; NOPS],
            alloc: Alloc::new(),
            left: Chain::new(),
            right: Chain::new(),
            limiter,
            quad_l: [0.0; 4],
            quad_r: [0.0; 4],
            control_countdown: 0,
            master_gain: 1.0,
        };
        e.load_patch(0);
        e
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn quality(&self) -> Quality {
        self.quality
    }

    pub fn patch_index(&self) -> usize {
        self.patch_index
    }

    pub fn patch_name(&self) -> &'static str {
        self.patch.name
    }

    pub fn active_voices(&self) -> usize {
        self.alloc.active_count()
    }

    pub fn latency_samples(&self) -> u32 {
        latency_samples(self.quality)
    }

    /// Switch oversampling. Resets only the stage that changes rate role.
    ///
    /// `s16` is idle at 8x, so entering 16x with stale state in it would splice
    /// a fragment of an older signal into the new one. The lower stages keep
    /// running at unchanged rates and keep their state.
    pub fn set_quality(&mut self, q: Quality) {
        if q == self.quality {
            return;
        }
        self.quality = q;
        self.left.s16.reset();
        self.right.s16.reset();

        // Re-cook every sounding lane. A phase increment is per *tick*, and
        // changing quality changes how many ticks there are per second, so an
        // increment cooked at the old rate plays an octave out at the new one.
        // Switching quality under a held note is the whole point of it being a
        // runtime control, so this cannot wait for the next note-on.
        let sr_os = self.sr_os();

        // The damping coefficient is per tick and the corner is in Hz, so it
        // has to be re-derived at the new rate for the *response* to be
        // unchanged. Missing this would make quality a tone control, which is
        // the same class of bug as the increments above.
        for b in self.banks.iter_mut() {
            b.set_damping(&self.patch.ops, sr_os);
        }

        for slot in 0..N_SLOTS {
            if self.alloc.voices[slot].is_idle() {
                continue;
            }
            let pitch = self.alloc.voices[slot].pitch;
            let (bank, lane) = (slot / LANES, slot % LANES);
            self.banks[bank].cook_lane(&self.waves, &self.patch.ops, lane, pitch, sr_os);
            self.restore_detune(slot);
        }
    }

    /// Select one of the six hardwired patches.
    ///
    /// Kills all sound: the patch defines the operator topology, so voices in
    /// flight are running a routing that is about to stop existing.
    pub fn set_patch(&mut self, index: usize) {
        self.load_patch(index);
        self.panic();
    }

    /// Install a factory patch by index.
    fn load_patch(&mut self, index: usize) {
        self.patch_index = index % crate::patch::N_PATCHES;
        self.install_patch(patch(index));
    }

    /// Install a patch's topology. Does not touch voices — [`Self::set_patch`]
    /// owns that decision, and construction has none to touch.
    ///
    /// Split from [`Self::load_patch`] so a patch that did not come from the
    /// factory bank can be installed: 0383's preset round-trip renders a
    /// *decoded* patch and compares the samples against the original's, which
    /// an index-only entry point cannot express. Deliberately not `pub` — who
    /// owns a live patch is 0382's question, not this one's.
    pub(crate) fn install_patch(&mut self, p: Patch) {
        // Reserve a lane for every route the matrix can reach, live or not.
        // `sine` authors no PM at all and still has a macro on its feedback
        // diagonal; without the mask that knob would compile away to nothing.
        let force = force_mask(&p.matrix);
        self.routing = CompiledRouting::compile_with(&p.routing, &force);
        let live = |s: &vxn_core_matrix::slot::MatrixSlot<SourceId, DestId>| {
            s.is_active() && s.depth != 0.0
        };
        self.modulated = p.matrix.slots.iter().any(live);
        // Half-open ranges, not `>=`. The damping predicate was
        // `i >= damp_dest_index(0)` while damping was the last family; adding
        // detune and pan after it would have made every one of those routes
        // read as a damping route. `matrix::tests::dest_indices_match_the_enum`
        // pins the family boundaries this relies on.
        let targets = |lo: usize, hi: usize| {
            p.matrix
                .slots
                .iter()
                .any(|s| live(s) && DestId::idx(s.dest).is_some_and(|i| i >= lo && i < hi))
        };
        self.damp_routed = targets(damp_dest_index(0), ratio_dest_index(0));
        self.detuned = targets(ratio_dest_index(0), pan_dest_index(0));
        self.panned = targets(pan_dest_index(0), spread_dest_index(0));
        self.spread_routed = targets(spread_dest_index(0), N_DESTS);

        for d in 0..NOPS {
            let theta = (p.ops[d].pan.clamp(-1.0, 1.0) + 1.0) * 0.25 * std::f32::consts::PI;
            self.pan_c[d] = theta.cos();
            self.pan_s[d] = theta.sin();
        }
        self.bus = SumBus::new(&p.ops, &p.routing);

        let sr_os = self.sr_os();
        for b in self.banks.iter_mut() {
            b.set_waves(&p.ops);
            b.set_phases(&p.ops);
            b.set_damping(&p.ops, sr_os);
        }
        self.live_ratios = std::array::from_fn(|d| p.ops[d].ratio);
        self.patch = p;
        // Macro positions survive a patch change — they are host automation,
        // and a lane that holds macro 1 at 0.7 across a patch switch must not
        // have the new patch snap back to its authored depths for a block.
        self.mod_dirty = true;
        self.apply_modulation();
    }

    /// A macro knob, `0..=7`, in `[0, 1]`. Out-of-range indices are ignored.
    pub fn set_macro(&mut self, index: usize, value: f32) {
        if index >= N_MACROS {
            return;
        }
        let v = value.clamp(0.0, 1.0);
        if self.macros[index] == v {
            return;
        }
        self.macros[index] = v;
        self.mod_dirty = true;
    }

    pub fn macro_value(&self, index: usize) -> f32 {
        self.macros.get(index).copied().unwrap_or(0.0)
    }

    /// Output trim, applied upstream of the limiter. Clamped to
    /// [`MAX_MASTER_GAIN`].
    pub fn set_master_gain(&mut self, gain: f32) {
        self.master_gain = gain.clamp(0.0, MAX_MASTER_GAIN);
    }

    pub fn master_gain(&self) -> f32 {
        self.master_gain
    }

    /// Evaluate the matrix and push the totals into the live routing.
    ///
    /// Totals are **added** to the authored depths, so every macro at zero is
    /// the patch exactly as written — which is what makes the patch table
    /// readable on its own and keeps `set_macro` from being load-bearing for a
    /// patch to sound right.
    ///
    /// Runs at control rate and only when something moved. Skipping it when
    /// clean is not just an optimisation: with no active slots this path never
    /// runs at all, so an unmodulated patch pays nothing for the matrix
    /// existing.
    fn apply_modulation(&mut self) {
        if !self.mod_dirty {
            return;
        }
        self.mod_dirty = false;
        if !self.modulated {
            return;
        }

        eval_dests::<Roster, SourceId, DestId, N_MACROS, N_DESTS>(
            &self.patch.matrix.slots,
            &self.macros,
            &mut self.dests,
        );

        let base: &Routing = &self.patch.routing;
        let mut pm = [[0.0f32; NOPS]; NOPS];
        for (d, row) in pm.iter_mut().enumerate() {
            for (s, cell) in row.iter_mut().enumerate() {
                // Unclamped: a modulation index is not bounded by anything
                // musical, and `phase_offset` wraps rather than saturating
                // precisely so a hot route stays a sound instead of a clamp.
                *cell = base.pm[d][s] + self.dests[pm_dest_index(d, s)];
            }
        }
        self.routing.set_pm(&pm);

        for d in 0..NOPS {
            // Sum-bus sends *are* clamped. Below zero is a phase flip rather
            // than silence, which is not what a send fader means, and the
            // upper bound keeps a stack of modulated sends from walking into
            // the limiter — see `CEILING` for why that is expensive here.
            let g = (base.out[d] + self.dests[out_dest_index(d)]).clamp(0.0, 1.0);
            self.bus.l[d] = g * self.pan_c[d];
            self.bus.r[d] = g * self.pan_s[d];
        }

        if self.damp_routed {
            // Damping totals are in **octaves**, so they shift the corner
            // multiplicatively rather than adding Hz to it — a corner is a
            // log-frequency control and an additive Hz total would put the
            // whole usable range in the last few percent of the knob.
            //
            // Clamped to a musically reachable span. The floor stops a knob at
            // full travel resolving to DC, which would freeze `pm` at its
            // current value and silently disconnect every route into the
            // operator; the ceiling is well above any oversampled Nyquist in
            // play, so it reads as bypass.
            let sr_os = self.sr_os();
            let hz: [f32; NOPS] = std::array::from_fn(|d| {
                let shift = self.dests[damp_dest_index(d)];
                (self.patch.ops[d].damp_hz * shift.exp2()).clamp(MIN_DAMP_HZ, MAX_DAMP_HZ)
            });
            for b in self.banks.iter_mut() {
                b.set_damping_hz(&hz, sr_os);
            }
        }

        if self.detuned {
            // Semitones, so the shift is multiplicative on the ratio for the
            // same reason damping's is on the corner: pitch is logarithmic and
            // an additive total would not be a detune at all.
            let sr_os = self.sr_os();
            self.live_ratios = std::array::from_fn(|d| {
                let semis = self.dests[ratio_dest_index(d)];
                self.patch.ops[d].ratio * (semis / 12.0).exp2()
            });
            let ratios = self.live_ratios;
            // Every sounding lane, because an increment is per lane: the ratio
            // is patch-wide but the frequency it lands on is not. One `exp2`
            // per lane for the base pitch, then a multiply per operator.
            for slot in 0..N_SLOTS {
                if self.alloc.voices[slot].is_idle() {
                    continue;
                }
                let hz0 = vxn4_dsp::ops::pitch_to_freq(self.alloc.voices[slot].pitch);
                let (bank, lane) = (slot / LANES, slot % LANES);
                self.banks[bank].set_lane_incs(lane, hz0, &ratios, sr_os);
            }
        }

        if self.spread_routed {
            // Pushed into the banks so it is in place for the *next* note-on —
            // `reset_lane` is where a start phase exists at all. Nothing about
            // a sounding note changes, which is why this needs no re-cook the
            // way detune does.
            let spread: [f32; NOPS] = std::array::from_fn(|d| {
                (self.patch.ops[d].phase_spread + self.dests[spread_dest_index(d)]).clamp(0.0, 1.0)
            });
            for b in self.banks.iter_mut() {
                b.set_phase_spread(&spread);
            }
        }

        if self.panned {
            // Pan moves the constant-power factors, so the cached pair has to
            // be recomputed rather than reused — and then the sends above have
            // to be reapplied through the new factors, since they were written
            // with the old ones a few lines up.
            for d in 0..NOPS {
                let pan = (self.patch.ops[d].pan + self.dests[pan_dest_index(d)]).clamp(-1.0, 1.0);
                let theta = (pan + 1.0) * 0.25 * std::f32::consts::PI;
                self.pan_c[d] = theta.cos();
                self.pan_s[d] = theta.sin();
                let g = (base.out[d] + self.dests[out_dest_index(d)]).clamp(0.0, 1.0);
                self.bus.l[d] = g * self.pan_c[d];
                self.bus.r[d] = g * self.pan_s[d];
            }
        }
    }

    /// Silence everything immediately.
    pub fn panic(&mut self) {
        self.alloc.clear();
        for b in self.banks.iter_mut() {
            for lane in 0..LANES {
                b.reset_lane(lane, 0);
            }
        }
        self.left.reset();
        self.right.reset();
        self.limiter.reset();
        self.quad_l = [0.0; 4];
        self.quad_r = [0.0; 4];
        self.control_countdown = 0;
    }

    pub fn note_on(&mut self, note: u8, velocity: u8) {
        if velocity == 0 {
            self.note_off(note);
            return;
        }
        let action = self.alloc.note_on(&self.patch.eg, note, velocity);
        let (slot, fresh) = match action {
            Action::Start { slot } => (slot, true),
            Action::Reuse { slot } => (slot, false),
        };
        let (bank, lane) = (slot / LANES, slot % LANES);
        let pitch = self.alloc.voices[slot].pitch;
        if fresh {
            // Decorrelate by slot and note so two lanes on the same pitch do
            // not phase-lock into a doubled copy.
            let seed = (slot as u32).wrapping_mul(0x2545_F491) ^ (note as u32).wrapping_mul(0x9E37);
            self.banks[bank].reset_lane(lane, seed);
        }
        self.banks[bank].cook_lane(&self.waves, &self.patch.ops, lane, pitch, self.sr_os());
        self.restore_detune(slot);
    }

    /// Re-apply the live detune to one lane after something cooked it from the
    /// patch's nominal ratios. See [`Self::live_ratios`].
    fn restore_detune(&mut self, slot: usize) {
        if !self.detuned {
            return;
        }
        let hz0 = vxn4_dsp::ops::pitch_to_freq(self.alloc.voices[slot].pitch);
        let (bank, lane) = (slot / LANES, slot % LANES);
        let (ratios, sr_os) = (self.live_ratios, self.sr_os());
        self.banks[bank].set_lane_incs(lane, hz0, &ratios, sr_os);
    }

    pub fn note_off(&mut self, note: u8) {
        self.alloc.note_off(note);
    }

    pub fn all_notes_off(&mut self) {
        self.alloc.all_notes_off();
    }

    /// Oversampled operator rate.
    fn sr_os(&self) -> f32 {
        self.sample_rate * self.quality.factor() as f32
    }

    /// Advance envelopes and push the resulting levels into the banks.
    fn control_tick(&mut self) {
        // Before the steal weights are read: modulation moves the sum-bus
        // sends, and those sends are what weight an operator's envelope in the
        // heuristic below. Evaluating after would weight this block against
        // last block's routing.
        self.apply_modulation();

        let dt = CONTROL_PERIOD as f32 / self.sample_rate;
        // Weight each operator's envelope by its sum-bus presence, so a pure
        // modulator cannot make a voice look loud to the steal heuristic.
        let mut weight = [0.0f32; NOPS];
        for (d, w) in weight.iter_mut().enumerate() {
            *w = self.bus.l[d].abs() + self.bus.r[d].abs();
        }
        let retired = self.alloc.control_tick(dt, &weight);

        // Re-select every lane's mip from the peak instantaneous rate the last
        // block actually saw. This is the 32-sample quantum the whole scheme is
        // built around: fast enough to track an attack, slow enough that
        // hysteresis is tractable and the selector is not in the tick loop.
        for b in self.banks.iter_mut() {
            b.update_mips(&self.waves, &self.patch.ops);
        }

        for slot in 0..N_SLOTS {
            let (bank, lane) = (slot / LANES, slot % LANES);
            if retired & (1 << slot) != 0 {
                // Clear the history ring too — a retired lane that keeps its
                // last outputs would feed a stale tail into the next note
                // through the feedback diagonal.
                self.banks[bank].reset_lane(lane, slot as u32);
                continue;
            }
            let v = &self.alloc.voices[slot];
            if v.phase == Phase::Idle {
                continue;
            }
            for (d, eg) in v.eg.iter().enumerate() {
                self.banks[bank].set_lane_op_level(lane, d, self.patch.ops[d].level * eg.level);
            }
        }
    }

    /// True when every lane in `bank` is idle, so the bank can be skipped.
    fn bank_is_silent(&self, bank: usize) -> bool {
        (0..LANES).all(|lane| {
            let slot = bank * LANES + lane;
            slot >= N_SLOTS || self.alloc.voices[slot].is_idle()
        })
    }

    /// Render `out_l.len()` samples at the host rate.
    pub fn process(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        debug_assert_eq!(out_l.len(), out_r.len());
        let ticks_per_4x = self.quality.ticks_per_4x();
        let gain = self.patch.gain * self.master_gain;
        let mut ticks = [0.0f32; 8];

        for i in 0..out_l.len() {
            if self.control_countdown == 0 {
                self.control_tick();
                self.control_countdown = CONTROL_PERIOD;
            }
            self.control_countdown -= 1;

            // Four 4x samples make one output sample.
            for q in 0..4 {
                for t in 0..ticks_per_4x {
                    let mut l = 0.0f32;
                    let mut r = 0.0f32;
                    for b in 0..N_BANKS {
                        if self.bank_is_silent(b) {
                            continue;
                        }
                        let (bl, br) =
                            self.banks[b].tick::<ValueSlope>(&self.waves, &self.routing, &self.bus);
                        l += bl;
                        r += br;
                    }
                    ticks[t] = l * gain;
                    ticks[t + 4] = r * gain;
                }
                // Interleaved into one scratch array so the two channels share a
                // loop; the halves never overlap because ticks_per_4x <= 4.
                self.quad_l[q] = self.left.fold_to_4x(&ticks[..ticks_per_4x], self.quality);
                self.quad_r[q] = self
                    .right
                    .fold_to_4x(&ticks[4..4 + ticks_per_4x], self.quality);
            }

            // Limiter at 4x — four samples per output sample.
            for q in 0..4 {
                let (l, r) = self.limiter.process(self.quad_l[q], self.quad_r[q]);
                self.quad_l[q] = l;
                self.quad_r[q] = r;
            }

            // Backstop only — see `CEILING`. Two halfband stages run downstream
            // of the limiter, so the limiter cannot itself bound this.
            out_l[i] = self.left.fold_to_1x(self.quad_l).clamp(-1.0, 1.0);
            out_r[i] = self.right.fold_to_1x(self.quad_r).clamp(-1.0, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn render(e: &mut Engine, samples: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (vec![0.0; samples], vec![0.0; samples]);
        e.process(&mut l, &mut r);
        (l, r)
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    #[test]
    fn silence_in_silence_out() {
        let mut e = Engine::new(SR);
        let (l, r) = render(&mut e, 1024);
        assert_eq!(peak(&l), 0.0);
        assert_eq!(peak(&r), 0.0);
    }

    #[test]
    fn a_note_makes_sound_on_every_patch_at_both_qualities() {
        for p in 0..crate::patch::N_PATCHES {
            for q in [Quality::X8, Quality::X16] {
                let mut e = Engine::new(SR);
                e.set_patch(p);
                e.set_quality(q);
                e.note_on(60, 100);
                let (l, r) = render(&mut e, 4096);
                let pk = peak(&l).max(peak(&r));
                assert!(
                    pk > 0.01,
                    "patch {} ({}) at {:?} produced peak {pk}",
                    p,
                    e.patch_name(),
                    q
                );
                assert!(
                    pk <= 1.0,
                    "patch {} exceeded full scale: {pk}",
                    e.patch_name()
                );
                assert!(l.iter().all(|s| s.is_finite()));
                assert!(r.iter().all(|s| s.is_finite()));
            }
        }
    }

    /// Under musical load the output must stay clear of full scale entirely —
    /// no clamping anywhere in the chain.
    #[test]
    fn a_musical_chord_never_approaches_full_scale() {
        for p in 0..crate::patch::N_PATCHES {
            for q in [Quality::X8, Quality::X16] {
                let mut e = Engine::new(SR);
                e.set_patch(p);
                e.set_quality(q);
                for n in [48u8, 55, 60, 63, 67, 70] {
                    e.note_on(n, 100);
                }
                let (l, r) = render(&mut e, 16_384);
                let pk = peak(&l).max(peak(&r));
                assert!(pk < 0.999, "patch {p} at {q:?} reached {pk}");
            }
        }
    }

    /// Twenty notes landing in a single sample is a ~14x step into the limiter,
    /// and its 2 ms lookahead cannot fully track a 5 ms attack, so its own
    /// internal clamp engages briefly at the onset. That is a safety limiter
    /// behaving correctly. What must hold is that it is confined to the onset:
    /// once the gain envelope has converged, the steady state has to sit under
    /// the ceiling rather than riding the clamp.
    #[test]
    fn an_extreme_onset_clamps_only_during_the_transient() {
        let settle = (SR * 0.05) as usize;
        for p in 0..crate::patch::N_PATCHES {
            for q in [Quality::X8, Quality::X16] {
                let mut e = Engine::new(SR);
                e.set_patch(p);
                e.set_quality(q);
                for n in 0..N_SLOTS {
                    e.note_on(40 + n as u8 * 2, 127);
                }
                let (l, r) = render(&mut e, 32_768);
                // The hard guarantee, everywhere.
                assert!(
                    peak(&l).max(peak(&r)) <= 1.0,
                    "patch {p} exceeded full scale"
                );
                // The real check: steady state is limited, not clipped.
                let tail = peak(&l[settle..]).max(peak(&r[settle..]));
                assert!(
                    tail < 0.999,
                    "patch {p} at {q:?} still riding the clamp after settling ({tail})"
                );
            }
        }
    }

    #[test]
    fn output_stays_finite_and_bounded_under_a_dense_chord() {
        let mut e = Engine::new(SR);
        e.set_patch(4); // web — all 64 routes
        for n in 0..N_SLOTS {
            e.note_on(48 + n as u8, 127);
        }
        let (l, r) = render(&mut e, 8192);
        assert!(l.iter().chain(r.iter()).all(|s| s.is_finite()));
        assert!(peak(&l) <= 1.0 && peak(&r) <= 1.0, "limiter let it through");
    }

    #[test]
    fn a_released_note_decays_to_silence_and_frees_its_voice() {
        let mut e = Engine::new(SR);
        e.set_patch(1);
        e.note_on(60, 100);
        render(&mut e, 2048);
        assert_eq!(e.active_voices(), 1);
        e.note_off(60);
        let (l, _) = render(&mut e, (SR * 3.0) as usize);
        assert_eq!(e.active_voices(), 0, "voice never retired");
        // Tail of the render must be silent, not merely quiet.
        let tail = &l[l.len() - 512..];
        assert!(peak(tail) < 1e-5, "tail peak {}", peak(tail));
    }

    #[test]
    fn polyphony_caps_at_sixteen() {
        let mut e = Engine::new(SR);
        e.set_patch(1);
        for n in 0..24u8 {
            e.note_on(40 + n, 100);
            render(&mut e, 64);
        }
        assert_eq!(e.active_voices(), crate::alloc::N_ACTIVE);
    }

    #[test]
    fn panic_silences_everything() {
        let mut e = Engine::new(SR);
        e.set_patch(3);
        for n in 0..8u8 {
            e.note_on(50 + n, 110);
        }
        render(&mut e, 512);
        e.panic();
        let (l, r) = render(&mut e, 1024);
        assert_eq!(peak(&l), 0.0);
        assert_eq!(peak(&r), 0.0);
        assert_eq!(e.active_voices(), 0);
    }

    /// A quality switch must not splice stale filter state into the output.
    #[test]
    fn switching_quality_mid_note_does_not_glitch() {
        let mut e = Engine::new(SR);
        e.set_patch(0);
        e.note_on(60, 100);
        let (a, _) = render(&mut e, 2048);
        e.set_quality(Quality::X16);
        let (b, _) = render(&mut e, 2048);
        assert!(b.iter().all(|s| s.is_finite()));
        // The seam must not produce a sample far outside the signal's own range.
        let bound = peak(&a) * 2.0 + 0.05;
        assert!(
            peak(&b) < bound,
            "switch spiked to {} (bound {bound})",
            peak(&b)
        );
    }

    /// Pitch must survive a quality switch under a held note.
    ///
    /// Increments are per tick, and quality changes the tick rate, so a lane
    /// that is not re-cooked plays an octave out. Measured by Goertzel at the
    /// note's own frequency rather than by peak level, because the level is
    /// unchanged by the bug — which is why the earlier glitch test passed
    /// straight through it.
    #[test]
    fn a_held_note_keeps_its_pitch_across_a_quality_switch() {
        let f0 = vxn4_dsp::ops::note_to_freq(69); // A440
        let mut e = Engine::new(SR);
        e.set_patch(0);
        e.note_on(69, 100);
        render(&mut e, 4096);
        e.set_quality(Quality::X16);
        let (l, _) = render(&mut e, 8192);

        let total = total_energy(&l);
        let fund = energy_at(&l, f0);
        let octave_up = energy_at(&l, f0 * 2.0);
        assert!(
            fund / total > 0.9,
            "fundamental holds only {:.3} of the energy after the switch",
            fund / total
        );
        assert!(
            octave_up < fund * 0.01,
            "energy appeared an octave up ({octave_up:.2e} vs {fund:.2e}) — \
             lanes were not re-cooked"
        );
    }

    /// Energy at `hz`, by Goertzel. Used to see modulation as spectrum rather
    /// than as level, which is what the PM destinations actually change.
    ///
    /// Scaled to compare against **`total_energy`** — the sum of squares, not
    /// the mean. Getting that wrong makes `fund` larger than the whole signal
    /// by a factor of `len`, and every ratio test built on it passes without
    /// measuring anything.
    fn energy_at(x: &[f32], hz: f32) -> f32 {
        let w = 2.0 * std::f32::consts::PI * hz / SR;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for &v in x {
            let s0 = v + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        (s1 * s1 + s2 * s2 - coeff * s1 * s2).abs() * 2.0 / x.len() as f32
    }

    fn total_energy(x: &[f32]) -> f32 {
        x.iter().map(|s| s * s).sum()
    }

    /// Every macro at zero must render the patch exactly as authored. If this
    /// fails the patch table has stopped being readable on its own, because
    /// what you hear depends on knob positions the table does not mention.
    #[test]
    fn macros_at_zero_are_the_patch_as_written() {
        for p in 0..crate::patch::N_PATCHES {
            let mut a = Engine::new(SR);
            a.set_patch(p);
            a.note_on(60, 100);
            let (la, _) = render(&mut a, 4096);

            let mut b = Engine::new(SR);
            b.set_patch(p);
            for m in 0..N_MACROS {
                b.set_macro(m, 0.0);
            }
            b.note_on(60, 100);
            let (lb, _) = render(&mut b, 4096);

            assert_eq!(la, lb, "patch {p} moved with every macro at zero");
        }
    }

    /// A macro on a route the patch does not author must still reach it.
    ///
    /// `sine` has an empty `Routing` and a matrix slot on its own feedback
    /// diagonal, so this is the force-mask contract end to end: without the
    /// mask there is no lane for `set_pm` to write and the knob is inert.
    /// Measured as harmonic content, because self-feedback on a sine adds
    /// harmonics rather than level.
    #[test]
    fn a_macro_reaches_a_route_the_patch_never_authored() {
        let f0 = vxn4_dsp::ops::note_to_freq(69);
        let harmonics = |e: &mut Engine| {
            e.note_on(69, 100);
            render(e, 8192);
            let (l, _) = render(e, 8192);
            let total = total_energy(&l);
            (total - energy_at(&l, f0)).max(0.0) / total
        };

        let mut off = Engine::new(SR);
        off.set_patch(0);
        let clean = harmonics(&mut off);

        let mut on = Engine::new(SR);
        on.set_patch(0);
        on.set_macro(0, 1.0);
        let dirty = harmonics(&mut on);

        assert!(
            clean < 0.03,
            "the sine patch was not clean to begin with ({clean})"
        );
        assert!(
            dirty > 0.20,
            "macro 1 did not open the feedback diagonal ({dirty} of energy off the fundamental)"
        );
    }

    /// The force mask must have something in the patch set that needs it, or
    /// it is untested machinery that will rot.
    ///
    /// What needs it is an **off-diagonal** PM route the patch does not author:
    /// the diagonal lives in `CompiledRouting::fb`, which always has a slot per
    /// operator, so a feedback route is reachable with or without the mask.
    #[test]
    fn the_force_mask_is_exercised_by_the_patch_set() {
        let mut found = Vec::new();
        for p in 0..crate::patch::N_PATCHES {
            let pat = crate::patch::patch(p);
            for slot in &pat.matrix.slots {
                if !slot.is_active() || slot.depth == 0.0 {
                    continue;
                }
                let Some(di) = DestId::idx(slot.dest) else {
                    continue;
                };
                if di >= NOPS * NOPS {
                    continue; // a sum-bus send, not a PM route
                }
                let (d, s) = (di / NOPS, di % NOPS);
                if d != s && pat.routing.pm[d][s] == 0.0 {
                    found.push((pat.name, d, s));
                }
            }
        }
        assert!(
            !found.is_empty(),
            "no patch routes a macro onto an unauthored off-diagonal route, so \
             `force_mask` is dead code as far as the tests can see"
        );
    }

    /// The force mask, end to end: a macro on an off-diagonal route the patch
    /// authors at zero must still be audible.
    #[test]
    fn a_macro_reaches_an_unauthored_off_diagonal_route() {
        let run = |m4: f32| {
            let mut e = Engine::new(SR);
            e.set_patch(3); // saws — macro 4 is on pm[2][4], authored at zero
            e.set_macro(3, m4);
            e.note_on(52, 100);
            render(&mut e, 4096);
            let (l, _) = render(&mut e, 4096);
            l
        };
        assert_ne!(run(1.0), run(0.0), "macro 4 had no lane to write into");
    }

    /// A macro on a `Damp` destination must change the sound, and in the right
    /// direction: knob up is darker.
    ///
    /// Measured as the RMS of the output's first difference — a crude
    /// high-pass, which is the direct reading of "how bright is this". `sine`
    /// with macro 1 open is the cleanest case in the set: one operator, one
    /// feedback route, and macro 2 on its damping, so nothing else can move.
    #[test]
    fn a_macro_on_damping_changes_brightness() {
        let brightness = |m2: f32| {
            let mut e = Engine::new(SR);
            e.set_patch(0);
            e.set_macro(0, 1.0); // open the feedback so there is something to damp
            e.set_macro(1, m2); // damping
            e.note_on(69, 100);
            render(&mut e, 8192);
            let (l, _) = render(&mut e, 8192);
            let hf: f32 = l.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
            (hf / l.len() as f32).sqrt()
        };

        let open = brightness(0.0);
        let mid = brightness(0.5);
        let dark = brightness(1.0);

        assert!(
            open > 0.0,
            "the undamped case had no high-frequency content"
        );
        assert!(
            mid < open,
            "half damping ({mid}) was not darker than none ({open})"
        );
        assert!(
            dark < mid,
            "full damping ({dark}) was not darker than half ({mid})"
        );
    }

    /// Every patch must stay well-behaved with the damping knobs at both ends —
    /// including fully closed, which is the setting that could freeze `pm` if
    /// the corner were allowed to reach DC.
    #[test]
    fn damping_at_both_extremes_stays_finite_and_audible() {
        for p in 0..crate::patch::N_PATCHES {
            for m in [0.0f32, 1.0] {
                let mut e = Engine::new(SR);
                e.set_patch(p);
                for k in 0..N_MACROS {
                    e.set_macro(k, m);
                }
                for n in [48u8, 60, 67, 72] {
                    e.note_on(n, 100);
                }
                let (l, r) = render(&mut e, 16_384);
                assert!(
                    l.iter().chain(r.iter()).all(|s| s.is_finite()),
                    "patch {p} went non-finite with every macro at {m}"
                );
                let pk = peak(&l).max(peak(&r));
                assert!(pk <= 1.0, "patch {p} at macro {m} reached {pk}");
                assert!(pk > 0.001, "patch {p} at macro {m} went silent ({pk})");
            }
        }
    }

    /// The corner is in Hz and the coefficient is per tick, so a quality switch
    /// has to re-derive it. Missing that would make Quality a tone control —
    /// the same class of bug as the increments, which did ship once.
    #[test]
    fn damping_survives_a_quality_switch() {
        let brightness_at = |q: Quality| {
            let mut e = Engine::new(SR);
            e.set_patch(0);
            e.set_quality(q);
            e.set_macro(0, 1.0);
            e.set_macro(1, 0.7);
            e.note_on(69, 100);
            render(&mut e, 8192);
            let (l, _) = render(&mut e, 8192);
            let hf: f32 = l.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
            (hf / l.len() as f32).sqrt()
        };
        let a = brightness_at(Quality::X8);
        let b = brightness_at(Quality::X16);
        assert!(
            (a / b - 1.0).abs() < 0.20,
            "damping is rate-dependent: {a} at 8x vs {b} at 16x"
        );
    }

    /// Detune, pan and level taper must each do something on `supersaw`, and
    /// pan must specifically produce **stereo width** — the patch is perfectly
    /// mono until M2 opens.
    #[test]
    fn the_supersaw_spread_controls_all_work() {
        let render_macro = |m: usize| {
            let mut e = Engine::new(SR);
            e.set_patch(6);
            if m > 0 {
                e.set_macro(m - 1, 1.0);
            }
            for n in [48u8, 55, 60] {
                e.note_on(n, 100);
            }
            render(&mut e, 16_384)
        };
        let width = |(l, r): &(Vec<f32>, Vec<f32>)| {
            let mid: f32 = l.iter().zip(r).map(|(a, b)| ((a + b) * 0.5).powi(2)).sum();
            let side: f32 = l.iter().zip(r).map(|(a, b)| ((a - b) * 0.5).powi(2)).sum();
            if mid > 0.0 { side / mid } else { 0.0 }
        };

        let base = render_macro(0);
        for (m, what) in [(1, "detune"), (3, "taper")] {
            let out = render_macro(m);
            assert_ne!(out.0, base.0, "macro {m} ({what}) changed nothing");
            assert!(out.0.iter().chain(out.1.iter()).all(|s| s.is_finite()));
        }

        // **Width requires detune**, and that is physics rather than a defect.
        // The saws start phase-coherent (see `OpConfig::phase`), so with M1 at
        // zero all seven carry the identical signal — and panning identical
        // signals symmetrically sums to dead centre no matter how far apart you
        // put them. M2 has nothing to separate until M1 makes the seven saws
        // different from each other.
        // A tolerance, not an exact zero: the constant-power gains differ per
        // operator, so the two channels sum through different arithmetic and
        // land ~1e-15 apart rather than bit-identical.
        const MONO: f32 = 1e-9;
        assert!(width(&base) < MONO, "the authored patch should be mono");
        assert!(
            width(&render_macro(2)) < MONO,
            "pan alone should still be mono — identical signals cannot be widened"
        );

        let mut e = Engine::new(SR);
        e.set_patch(6);
        e.set_macro(0, 1.0); // detune, so the saws stop being identical
        e.set_macro(1, 1.0); // width
        for n in [48u8, 55, 60] {
            e.note_on(n, 100);
        }
        let spread = render(&mut e, 16_384);
        assert!(
            width(&spread) > 0.01,
            "pan produced no width even with the saws detuned apart"
        );
    }

    /// `supersaw`'s M4 must **brighten** audibly, and M5 must pull it back.
    ///
    /// Pinned because the first version of this patch failed it silently in the
    /// worst way: M4 measured a +5 dB waveform difference and was inaudible.
    /// The modulator was at 1:1, so every sideband landed on a harmonic the saw
    /// already had — the waveform changed completely and the spectrum barely
    /// moved. A difference metric cannot see that; a brightness metric can.
    #[test]
    fn the_supersaw_modulator_brightens_and_rolls_off() {
        let brightness = |m4: f32, m5: f32| {
            let mut e = Engine::new(SR);
            e.set_patch(6);
            e.set_macro(3, m4);
            e.set_macro(4, m5);
            for n in [48u8, 55, 60] {
                e.note_on(n, 100);
            }
            let (l, _) = render(&mut e, 16_384);
            let tail = &l[l.len() / 3..];
            let hf: f32 = tail.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
            let tot: f32 = tail.iter().map(|s| s * s).sum();
            if tot > 0.0 { hf / tot } else { 0.0 }
        };

        let plain = brightness(0.0, 0.0);
        let bright = brightness(1.0, 0.0);
        let rolled = brightness(1.0, 1.0);

        assert!(plain > 0.0, "the authored patch was silent");
        assert!(
            bright > plain * 1.3,
            "M4 did not brighten: {plain} -> {bright}. A modulator whose \
             sidebands land on harmonics the carrier already has changes the \
             waveform without changing what you hear."
        );
        assert!(
            rolled < bright,
            "M5 did not roll the modulation off: {bright} -> {rolled}"
        );
    }

    /// `phase_spread == 1.0` must be **bit-identical** to the unconditional
    /// hash it replaced, or making phase configurable silently re-voiced the
    /// onset of all six patches that were written against it.
    ///
    /// Guarded by fixed point: the Q16 scale is exactly `1 << 16`, so
    /// `(hash * 65536) >> 16` is `hash`. Scaling through an `f32` would drop
    /// eight bits of a `u32` and this would fail.
    #[test]
    fn full_spread_reproduces_the_historical_onset() {
        for p in 0..crate::patch::N_PATCHES {
            // `supersaw` is the one patch deliberately voiced away from it.
            if crate::patch::patch(p).name == "supersaw" {
                continue;
            }
            let mut e = Engine::new(SR);
            e.set_patch(p);
            for d in 0..NOPS {
                assert_eq!(
                    e.patch.ops[d].phase_spread, 1.0,
                    "patch {p} op {d} drifted off the historical onset"
                );
                assert_eq!(e.patch.ops[d].phase, 0.0);
            }
        }
        // And the arithmetic itself, at the boundary.
        use vxn4_dsp::ops::{phase_hash, spread_scale_q16};
        for d in 0..NOPS {
            let scaled = ((phase_hash(d) as u64 * spread_scale_q16(1.0)) >> 16) as u32;
            assert_eq!(scaled, phase_hash(d), "op {d} lost bits at full spread");
            assert_eq!((phase_hash(d) as u64 * spread_scale_q16(0.0)) >> 16, 0);
        }
    }

    /// The phase-spread knob must travel between the two ends it exists to
    /// join: coherent (loud, mono) and decorrelated (wide, quieter).
    ///
    /// Read at note onset, so the notes have to be played **after** the knob
    /// moves — which is the property this also pins.
    #[test]
    fn the_phase_spread_knob_travels_between_coherent_and_scattered() {
        // M2 stays open throughout. Phase spread makes the seven saws
        // *different*; pan is what places them apart. Neither produces width
        // alone, and asserting spread-without-pan would be the same mistake as
        // asserting pan-without-detune.
        let run = |m6: f32| {
            let mut e = Engine::new(SR);
            e.set_patch(6);
            e.set_macro(1, 1.0); // width
            e.set_macro(5, m6); // phase spread
            render(&mut e, 2048); // let the control tick land the spread first
            for n in [48u8, 55, 60] {
                e.note_on(n, 100);
            }
            let (l, r) = render(&mut e, 16_384);
            let pk = peak(&l).max(peak(&r));
            let mid: f32 = l.iter().zip(&r).map(|(a, b)| ((a + b) * 0.5).powi(2)).sum();
            let side: f32 = l.iter().zip(&r).map(|(a, b)| ((a - b) * 0.5).powi(2)).sum();
            (pk, if mid > 0.0 { side / mid } else { 0.0 })
        };

        let (coherent_pk, coherent_w) = run(0.0);
        let (scattered_pk, scattered_w) = run(1.0);

        assert!(
            coherent_pk > 0.0 && scattered_pk > 0.0,
            "one end went silent"
        );
        assert!(
            coherent_pk > scattered_pk * 1.2,
            "coherent ({coherent_pk}) should be clearly louder than scattered \
             ({scattered_pk}) — seven aligned saws sum, seven scattered ones cancel"
        );
        // The point of the knob: pan is armed but inert while the seven saws
        // carry an identical signal, and scattering their phases is what gives
        // it something to separate.
        assert!(
            coherent_w < 1e-9,
            "coherent should still be mono even with pan open ({coherent_w})"
        );
        assert!(
            scattered_w > 0.01,
            "scattering the phases did not unlock pan ({scattered_w})"
        );
    }

    /// A note started **after** a detune knob has moved must be detuned too.
    ///
    /// `cook_lane` derives the increment from the patch's nominal ratio, and by
    /// the time a later note arrives `mod_dirty` is long clear — so without the
    /// `live_ratios` cache the new note plays at concert pitch against a
    /// detuned stack, which reads as one voice being out of tune rather than as
    /// a broken knob.
    #[test]
    fn detune_reaches_a_note_started_after_the_knob_moved() {
        let run = |m1: f32| {
            let mut e = Engine::new(SR);
            e.set_patch(6);
            e.set_macro(0, m1);
            render(&mut e, 4096); // clears mod_dirty
            e.note_on(60, 100);
            render(&mut e, 16_384).0
        };
        assert_ne!(
            run(1.0),
            run(0.0),
            "a late note ignored the detune the knob had already set"
        );
    }

    /// The scale VCA gates its route: `grind`'s macro 2 does nothing until
    /// macro 3 opens it, and then it does something.
    ///
    /// This is the brief's additive-plus-scaling pair, which comes from the
    /// shared `MatrixSlot` rather than from any vxn-4 code — so what is under
    /// test is the wiring, not the arithmetic.
    #[test]
    fn a_scaled_route_is_gated_by_its_vca() {
        let run = |m2: f32, m3: f32| {
            let mut e = Engine::new(SR);
            e.set_patch(5); // grind
            e.set_macro(1, m2);
            e.set_macro(2, m3);
            e.note_on(69, 100);
            render(&mut e, 4096);
            let (l, _) = render(&mut e, 4096);
            l
        };

        let closed = run(1.0, 0.0);
        let shut = run(0.0, 0.0);
        assert_eq!(closed, shut, "macro 2 was audible with its VCA closed");

        let open = run(1.0, 1.0);
        assert_ne!(open, shut, "opening the VCA changed nothing");
    }

    /// An out-dest route moves the sum bus, and the clamp holds at both ends.
    #[test]
    fn a_macro_on_a_sum_bus_send_changes_level() {
        let level = |m2: f32| {
            let mut e = Engine::new(SR);
            e.set_patch(3); // saws — macro 2 is on Out2, the sub
            e.set_macro(1, m2);
            e.note_on(48, 100);
            render(&mut e, 4096);
            let (l, r) = render(&mut e, 4096);
            peak(&l).max(peak(&r))
        };
        let lo = level(0.0);
        let hi = level(1.0);
        assert!(hi > lo * 1.05, "the send did not move ({lo} -> {hi})");
        assert!(hi <= 1.0);
    }

    /// Macro positions are host automation and must survive a patch change.
    /// A lane holding macro 1 at 0.7 across a switch must not snap back to the
    /// new patch's authored depths, even for one control block.
    #[test]
    fn macros_survive_a_patch_change() {
        let mut e = Engine::new(SR);
        e.set_macro(0, 0.7);
        e.set_patch(2);
        assert_eq!(e.macro_value(0), 0.7);
        e.set_patch(4);
        assert_eq!(e.macro_value(0), 0.7);
    }

    #[test]
    fn macro_indices_out_of_range_are_ignored() {
        let mut e = Engine::new(SR);
        e.set_macro(N_MACROS, 1.0);
        e.set_macro(999, 1.0);
        assert_eq!(e.macro_value(N_MACROS), 0.0);
        for m in 0..N_MACROS {
            assert_eq!(e.macro_value(m), 0.0);
        }
    }

    /// Modulation must not be able to drive the output past full scale, on any
    /// patch, with every macro pinned open and every voice sounding. This is
    /// the case the gain staging was measured against and modulation is the
    /// one thing that can walk out of it.
    #[test]
    fn every_macro_open_still_stays_in_range() {
        let settle = (SR * 0.05) as usize;
        for p in 0..crate::patch::N_PATCHES {
            let mut e = Engine::new(SR);
            e.set_patch(p);
            for m in 0..N_MACROS {
                e.set_macro(m, 1.0);
            }
            for n in 0..N_SLOTS {
                e.note_on(40 + n as u8 * 2, 127);
            }
            let (l, r) = render(&mut e, 32_768);
            assert!(
                l.iter().chain(r.iter()).all(|s| s.is_finite()),
                "patch {p} went non-finite"
            );
            let tail = peak(&l[settle..]).max(peak(&r[settle..]));
            assert!(
                tail < 0.999,
                "patch {p} rides the clamp with every macro open ({tail})"
            );
        }
    }

    /// The trim scales, clamps at both ends, and the limiter still holds the
    /// output at the top of its range — which is the point of it being applied
    /// upstream rather than on the way out.
    #[test]
    fn master_gain_scales_and_stays_bounded() {
        let at = |g: f32| {
            let mut e = Engine::new(SR);
            e.set_patch(1);
            e.set_master_gain(g);
            for n in [48u8, 55, 60, 64, 67] {
                e.note_on(n, 100);
            }
            let (l, r) = render(&mut e, 16_384);
            (peak(&l).max(peak(&r)), l, r)
        };

        let (silent, l, r) = at(0.0);
        assert_eq!(silent, 0.0, "zero trim was not silent");
        assert!(l.iter().chain(r.iter()).all(|s| *s == 0.0));

        let (unity, ..) = at(1.0);
        let (half, ..) = at(0.5);
        assert!(
            (half / unity - 0.5).abs() < 0.02,
            "half trim gave {half} against {unity}"
        );

        let mut e = Engine::new(SR);
        e.set_master_gain(99.0);
        assert_eq!(e.master_gain(), MAX_MASTER_GAIN, "trim was not clamped");
        e.set_master_gain(-1.0);
        assert_eq!(e.master_gain(), 0.0);

        let (hot, ..) = at(MAX_MASTER_GAIN);
        assert!(hot <= 1.0, "the limiter let a hot trim through at {hot}");
        assert!(hot > unity, "the trim did not push into the limiter");
    }

    #[test]
    fn latency_is_reported_per_quality() {
        assert_eq!(latency_samples(Quality::X8), 14);
        assert_eq!(latency_samples(Quality::X16), 15);
        // The host is told the worst case, so it never has to be renegotiated
        // when quality moves. See `HOST_LATENCY_SAMPLES`.
        assert_eq!(HOST_LATENCY_SAMPLES, 15);
        assert!(HOST_LATENCY_SAMPLES >= latency_samples(Quality::X8));
        let mut e = Engine::new(SR);
        assert_eq!(e.latency_samples(), 14);
        e.set_quality(Quality::X16);
        assert_eq!(e.latency_samples(), 15);
    }

    /// Block size must not change the output — the control-rate countdown has
    /// to survive a block boundary.
    #[test]
    fn block_size_does_not_change_the_render() {
        let render_with = |block: usize| {
            let mut e = Engine::new(SR);
            e.set_patch(2);
            e.note_on(64, 100);
            let mut out = Vec::new();
            let (mut l, mut r) = (vec![0.0; block], vec![0.0; block]);
            for _ in 0..(4096 / block) {
                e.process(&mut l, &mut r);
                out.extend_from_slice(&l);
            }
            out
        };
        let a = render_with(64);
        let b = render_with(128);
        assert_eq!(a.len(), b.len());
        assert_eq!(a, b, "render depends on block size");
    }

    /// Level must be even across the keyboard. Mip boundaries are crossed
    /// several times over five octaves, and a mip normalised against the wrong
    /// reference would show up here as a step at one specific pitch.
    ///
    /// One note at a time, with `panic` between, which matters: measuring this
    /// from a played chromatic run does not work. The patch has a 0.30 s
    /// release against 0.09 s between notes, so four notes overlap and beat,
    /// and the interference swamps the effect being looked for — that produced
    /// a 2.8x spread with nothing wrong with the tables at all.
    ///
    /// The per-mip normalisation guarantee this rests on is asserted directly
    /// in `vxn4_dsp::wavetable::tests::every_mip_is_normalised_and_bounded`.
    #[test]
    fn level_is_even_across_the_keyboard() {
        let mut e = Engine::new(SR);
        e.set_patch(0); // sine: level is purely the table read
        let settle = (SR * 0.02) as usize;
        let mut peaks = Vec::new();
        for note in 36..96u8 {
            e.panic();
            e.note_on(note, 100);
            let (l, _) = render(&mut e, (SR * 0.06) as usize);
            peaks.push(peak(&l[settle..]));
        }
        let lo = peaks.iter().cloned().fold(f32::MAX, f32::min);
        let hi = peaks.iter().cloned().fold(0.0f32, f32::max);
        assert!(lo > 0.05, "some notes were silent (min {lo})");
        assert!(
            hi / lo < 1.15,
            "level spread {:.3}x across five octaves (min {lo:.4}, max {hi:.4})",
            hi / lo
        );
        for (i, w) in peaks.windows(2).enumerate() {
            let ratio = w[1] / w[0];
            assert!(
                (0.92..1.09).contains(&ratio),
                "level stepped {ratio:.3}x at note {}",
                36 + i
            );
        }
    }

    /// The sine patch is the reference tone: one operator, nothing modulating.
    /// It must come out clean, which is really a test of the decimator.
    #[test]
    fn the_sine_patch_is_spectrally_clean() {
        let mut e = Engine::new(SR);
        e.set_patch(0);
        e.note_on(69, 127); // A440
        render(&mut e, 8192); // let the envelope settle
        let (l, _) = render(&mut e, 4096);

        // Goertzel at 440 Hz vs total energy: a clean sine puts nearly all of
        // its energy in the fundamental.
        let energy: f32 = l.iter().map(|s| s * s).sum();
        let w = 2.0 * std::f32::consts::PI * 440.0 / SR;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for &x in &l {
            let s0 = x + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        let fund = (s1 * s1 + s2 * s2 - coeff * s1 * s2) * 2.0 / l.len() as f32;
        let ratio = fund / energy;
        assert!(ratio > 0.97, "only {ratio} of energy at the fundamental");
    }
}
