//! Mod matrix engine (ticket 0008) — the central modulation router.
//!
//! Per ADR §6 this is the **only** mechanism for dynamic parameter modulation
//! in VXN2; no hard-wired routes. Each layer holds a fixed 16-slot table
//! (`MatrixTable`) of `MatrixSlot { source, dest, depth, curve }`. The
//! [`PatchMatrix`] wrapper carries one table per layer (Upper + Lower); in
//! Whole mode the engine drives all voices from the Upper table.
//!
//! ## Source granularity
//!
//! Sources split into three strides:
//!
//! - **Patch-global** ([`PatchSources`]): `lfo1`, `mod_wheel`, `aftertouch`.
//!   One scalar per patch, broadcast across all stacks and lanes.
//! - **Per-stack** ([`StackScalarSources`]): `pitch_eg`, `mod_env`,
//!   `velocity`, `key`. One scalar per played stack, broadcast across lanes.
//! - **Per-lane** ([`LaneSources`]): `lfo2`, `voice_idx`, `voice_spread`,
//!   `voice_rand`. One value per lane of the 8-lane stack.
//!
//! [`eval_sources`] fans these out into a single `[[f32; N_SOURCES];
//! STACK_LANES]` lookup table per stack — the slot eval inner loop reads from
//! one contiguous matrix regardless of source kind. Broadcast cost is paid
//! once per block at the source-eval site, never inside the per-slot loop.
//!
//! ## Destination application
//!
//! Two tiers per ADR §6 Consequences:
//!
//! - **Per block** ([`eval_dests`] writes into [`LaneDestVals`]): every
//!   non-zipper-sensitive destination is summed into a per-lane accumulator
//!   once per control block. Engine reads the accumulator at block start and
//!   applies it before the per-sample render.
//! - **Per sample** ([`PitchSmoother`]): pitch-shaped destinations (global
//!   pitch, per-op ratio, per-op detune, lfo2_phase) get one-pole smoothing
//!   from the block accumulator down to per-sample so the audio loop sees a
//!   ramp, not a step. Time constant matches one control block — same idiom
//!   as VXN1's [`vxn2_dsp::smoother::Smoothed`].
//!
//! ## Vectorisation note
//!
//! Per-slot inner loops walk 8 lanes. Curve dispatch happens once per slot
//! (outside the lane loop), so the lane-strided code in each curve arm is
//! straight-line FMA + add — autovectorises to NEON on AArch64.
//!
//! ## CLAP exposure
//!
//! Slots 1–8 `depth` per layer are CLAP-automatable (16 params total across
//! Upper + Lower); slots 9–16 `depth` and all slot `source` / `dest` / `curve`
//! fields are patch state only. Topology (source/dest/curve) isn't a
//! continuous control. See [`N_CLAP_DEPTH_SLOTS`] and the wire-up in ticket
//! 0012 (Master & Params). Slot depth, even when CLAP-automatable, is treated
//! as a per-block constant by the matrix engine — matrix-routing a slot's
//! depth via the matrix itself isn't supported in v1 (sidesteps cycle
//! detection per ticket Notes).

use vxn2_dsp::smoother::one_pole_coeff;
use vxn2_dsp::stack::STACK_LANES;

use crate::modulation::ModBlock;

/// Slot count per layer table. ADR §6 sets this at 16 for v1.
pub const N_SLOTS: usize = 16;

/// Number of CLAP-automatable depth slots per layer (slots 1..=N). Slots past
/// this count are patch-state only.
pub const N_CLAP_DEPTH_SLOTS: usize = 8;

// --- Source enum ----------------------------------------------------------

/// Modulation source. `None` is the "empty slot" sentinel — slots whose
/// source is `None` skip evaluation cheaply.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum SourceId {
    #[default]
    None = 0,
    Lfo1 = 1,
    Lfo2 = 2,
    PitchEg = 3,
    ModEnv = 4,
    ModWheel = 5,
    Aftertouch = 6,
    Velocity = 7,
    Key = 8,
    VoiceIdx = 9,
    VoiceSpread = 10,
    VoiceRand = 11,
}

/// Count of non-sentinel sources (i.e. `SourceId::None` excluded).
pub const N_SOURCES: usize = 11;

impl SourceId {
    /// Index into the per-lane source lookup, or `None` for the sentinel.
    #[inline]
    pub fn idx(self) -> Option<usize> {
        match self {
            SourceId::None => None,
            _ => Some(self as usize - 1),
        }
    }
}

// --- Destination enum -----------------------------------------------------

/// Modulation destination. `None` is the "empty slot" sentinel.
///
/// Per-op dests are laid out in op-major order (`op1_*` block, then `op2_*`,
/// …). 6 ops × 5 dests each = 30 op dests. Plus 4 global, 2 stack-macro,
/// 2 FX = 38 total.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum DestId {
    #[default]
    None = 0,
    Op1Ratio = 1, Op1Level, Op1Detune, Op1Pan, Op1Feedback,
    Op2Ratio, Op2Level, Op2Detune, Op2Pan, Op2Feedback,
    Op3Ratio, Op3Level, Op3Detune, Op3Pan, Op3Feedback,
    Op4Ratio, Op4Level, Op4Detune, Op4Pan, Op4Feedback,
    Op5Ratio, Op5Level, Op5Detune, Op5Pan, Op5Feedback,
    Op6Ratio, Op6Level, Op6Detune, Op6Pan, Op6Feedback,
    GlobalPitch,
    Lfo1Rate,
    Lfo2Rate,
    Lfo2Phase,
    StackDetune,
    StackSpread,
    DelayMix,
    ReverbMix,
}

/// Count of non-sentinel destinations.
pub const N_DESTS: usize = 38;

impl DestId {
    #[inline]
    pub fn idx(self) -> Option<usize> {
        match self {
            DestId::None => None,
            _ => Some(self as usize - 1),
        }
    }

    /// Pitch-shaped destinations are zipper-sensitive: per-sample smoothing
    /// applies. All others apply at block boundary.
    #[inline]
    pub fn is_pitch_shaped(self) -> bool {
        matches!(
            self,
            DestId::GlobalPitch
                | DestId::Lfo2Phase
                | DestId::Op1Ratio
                | DestId::Op2Ratio
                | DestId::Op3Ratio
                | DestId::Op4Ratio
                | DestId::Op5Ratio
                | DestId::Op6Ratio
                | DestId::Op1Detune
                | DestId::Op2Detune
                | DestId::Op3Detune
                | DestId::Op4Detune
                | DestId::Op5Detune
                | DestId::Op6Detune
        )
    }
}

/// Pitch-shaped destinations in canonical order. [`PitchSmoother`] iterates
/// over this list when copying targets out of [`LaneDestVals`].
pub const PITCH_DESTS: [DestId; N_PITCH_DESTS] = [
    DestId::GlobalPitch,
    DestId::Lfo2Phase,
    DestId::Op1Ratio,
    DestId::Op2Ratio,
    DestId::Op3Ratio,
    DestId::Op4Ratio,
    DestId::Op5Ratio,
    DestId::Op6Ratio,
    DestId::Op1Detune,
    DestId::Op2Detune,
    DestId::Op3Detune,
    DestId::Op4Detune,
    DestId::Op5Detune,
    DestId::Op6Detune,
];

pub const N_PITCH_DESTS: usize = 14;

// --- Curve ----------------------------------------------------------------

/// Curve applied to a source value before depth scaling.
///
/// - `Lin` — identity (passthrough).
/// - `Exp` — signed square: `sign(v) · v²`. More extreme excursions.
/// - `Log` — signed square root: `sign(v) · √|v|`. Compresses toward 0.
/// - `Bipolar` — AC-couple a unipolar `[0, 1]` source to `[-1, +1]` via
///   `2v - 1`. Useful when routing mod-wheel / aftertouch into a pitch dest
///   that wants centred swing.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum CurveKind {
    #[default]
    Lin = 0,
    Exp,
    Log,
    Bipolar,
}

// --- Slot / Table ---------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct MatrixSlot {
    pub source: SourceId,
    pub dest: DestId,
    pub depth: f32,
    pub curve: CurveKind,
}

impl Default for MatrixSlot {
    fn default() -> Self {
        Self {
            source: SourceId::None,
            dest: DestId::None,
            depth: 0.0,
            curve: CurveKind::Lin,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MatrixTable {
    pub slots: [MatrixSlot; N_SLOTS],
}

impl Default for MatrixTable {
    fn default() -> Self {
        Self {
            slots: [MatrixSlot::default(); N_SLOTS],
        }
    }
}

/// Layer enum stub. Per ADR §8 a patch has two layer tables (Upper, Lower);
/// in Whole mode the engine drives all voices from Upper. Ticket 0009 lands
/// the voicing-mode logic that gates which table feeds which stack — this
/// enum exists so the matrix API can carry the discriminator today.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Layer {
    #[default]
    Upper,
    Lower,
}

/// One matrix table per layer. Whole mode reads Upper; Layer/Split mode
/// dispatches per-stack by which layer the stack belongs to.
#[derive(Clone, Copy, Debug, Default)]
pub struct PatchMatrix {
    pub upper: MatrixTable,
    pub lower: MatrixTable,
}

impl PatchMatrix {
    #[inline]
    pub fn table(&self, layer: Layer) -> &MatrixTable {
        match layer {
            Layer::Upper => &self.upper,
            Layer::Lower => &self.lower,
        }
    }

    #[inline]
    pub fn table_mut(&mut self, layer: Layer) -> &mut MatrixTable {
        match layer {
            Layer::Upper => &mut self.upper,
            Layer::Lower => &mut self.lower,
        }
    }
}

// --- Source inputs --------------------------------------------------------

/// Patch-global scalar sources. Broadcast across every stack and every lane
/// inside [`eval_sources`].
#[derive(Clone, Copy, Debug, Default)]
pub struct PatchSources {
    pub lfo1: f32,
    pub mod_wheel: f32,
    pub aftertouch: f32,
}

impl PatchSources {
    /// Pull LFO1 from a [`ModBlock`]; mod wheel + aftertouch supplied by the
    /// host MIDI layer.
    #[inline]
    pub fn from_modblock(mb: &ModBlock, mod_wheel: f32, aftertouch: f32) -> Self {
        Self {
            lfo1: mb.lfo1,
            mod_wheel,
            aftertouch,
        }
    }
}

/// Per-stack scalar sources. Broadcast across the stack's 8 lanes.
#[derive(Clone, Copy, Debug, Default)]
pub struct StackScalarSources {
    /// Pitch EG output in semitones (raw — depth already applied by the EG).
    pub pitch_eg_st: f32,
    /// Mod env output in `[0, 1]`.
    pub mod_env: f32,
    /// Velocity normalised to `[0, 1]`.
    pub velocity: f32,
    /// Key (MIDI note) normalised to `[0, 1]`.
    pub key: f32,
}

/// Per-lane sources. One value per lane in the 8-lane stack.
#[derive(Clone, Copy, Debug, Default)]
pub struct LaneSources {
    pub lfo2: [f32; STACK_LANES],
    /// Lane index normalised to `[0, 1]`. Matrix consumers expect normalised
    /// shapes; the raw `u8` index lives on the stack for other consumers.
    pub voice_idx: [f32; STACK_LANES],
    /// Lane-symmetric position in `[-1, +1]`.
    pub voice_spread: [f32; STACK_LANES],
    /// Per-lane note-on random in `[0, 1)`.
    pub voice_rand: [f32; STACK_LANES],
}

// --- Source / dest lookup tables ------------------------------------------

/// Per-lane source lookup populated by [`eval_sources`].
pub type LaneSourceVals = [[f32; N_SOURCES]; STACK_LANES];

/// Per-lane destination accumulator populated by [`eval_dests`].
pub type LaneDestVals = [[f32; N_DESTS]; STACK_LANES];

/// Fan patch + stack + lane sources into a per-lane lookup the slot eval
/// loop can read with one index per source.
#[inline]
pub fn eval_sources(
    patch: &PatchSources,
    stack: &StackScalarSources,
    lanes: &LaneSources,
    out: &mut LaneSourceVals,
) {
    // Index expressions evaluate at compile time — `SourceId::Lfo1 as usize`
    // is a constant. Each lane assignment is straight stores.
    for k in 0..STACK_LANES {
        let v = &mut out[k];
        v[(SourceId::Lfo1 as usize) - 1] = patch.lfo1;
        v[(SourceId::Lfo2 as usize) - 1] = lanes.lfo2[k];
        v[(SourceId::PitchEg as usize) - 1] = stack.pitch_eg_st;
        v[(SourceId::ModEnv as usize) - 1] = stack.mod_env;
        v[(SourceId::ModWheel as usize) - 1] = patch.mod_wheel;
        v[(SourceId::Aftertouch as usize) - 1] = patch.aftertouch;
        v[(SourceId::Velocity as usize) - 1] = stack.velocity;
        v[(SourceId::Key as usize) - 1] = stack.key;
        v[(SourceId::VoiceIdx as usize) - 1] = lanes.voice_idx[k];
        v[(SourceId::VoiceSpread as usize) - 1] = lanes.voice_spread[k];
        v[(SourceId::VoiceRand as usize) - 1] = lanes.voice_rand[k];
    }
}

/// Walk slots, accumulate `source · curve · depth` into `out`. Zeroes `out`
/// before accumulating, so the caller can hand in any buffer. Empty slots
/// (source = `None` or dest = `None` or depth = 0) are skipped.
///
/// Curve match happens once per slot — lane loop inside each arm is
/// straight-line, autovectorises to NEON on AArch64.
#[inline]
pub fn eval_dests(table: &MatrixTable, sources: &LaneSourceVals, out: &mut LaneDestVals) {
    for k in 0..STACK_LANES {
        out[k].fill(0.0);
    }
    for slot in &table.slots {
        let Some(si) = slot.source.idx() else {
            continue;
        };
        let Some(di) = slot.dest.idx() else {
            continue;
        };
        if slot.depth == 0.0 {
            continue;
        }
        let depth = slot.depth;
        match slot.curve {
            CurveKind::Lin => {
                for k in 0..STACK_LANES {
                    out[k][di] += sources[k][si] * depth;
                }
            }
            CurveKind::Exp => {
                for k in 0..STACK_LANES {
                    let v = sources[k][si];
                    out[k][di] += v.abs() * v * depth;
                }
            }
            CurveKind::Log => {
                for k in 0..STACK_LANES {
                    let v = sources[k][si];
                    let mag = v.abs().sqrt();
                    let shaped = if v < 0.0 { -mag } else { mag };
                    out[k][di] += shaped * depth;
                }
            }
            CurveKind::Bipolar => {
                for k in 0..STACK_LANES {
                    out[k][di] += (2.0 * sources[k][si] - 1.0) * depth;
                }
            }
        }
    }
}

// --- Per-sample pitch smoother --------------------------------------------

/// Per-lane × per-pitch-dest one-pole IIR. Block-rate `set_targets_from`
/// updates targets; per-sample `tick` glides state toward them.
#[derive(Clone, Copy, Debug)]
pub struct PitchSmoother {
    state: [[f32; STACK_LANES]; N_PITCH_DESTS],
    coeff: f32,
}

impl Default for PitchSmoother {
    fn default() -> Self {
        Self {
            state: [[0.0; STACK_LANES]; N_PITCH_DESTS],
            coeff: 1.0,
        }
    }
}

impl PitchSmoother {
    /// Time constant matches the control block: smooth over ~1 block (one
    /// tau ≈ block duration). At 64 samples / 48 kHz that's ~1.33 ms — fast
    /// enough that block edges read smooth, slow enough that an LFO at S&H
    /// reads as steps with sloped edges rather than instant jumps.
    pub fn new(block_secs: f32, sample_rate: f32) -> Self {
        Self {
            state: [[0.0; STACK_LANES]; N_PITCH_DESTS],
            coeff: one_pole_coeff(block_secs * 1000.0, sample_rate),
        }
    }

    /// Pull the latest block target out of `dest_block` for each pitch dest.
    pub fn targets_from(&self, dest_block: &LaneDestVals) -> [[f32; STACK_LANES]; N_PITCH_DESTS] {
        let mut tgt = [[0.0; STACK_LANES]; N_PITCH_DESTS];
        for (i, d) in PITCH_DESTS.iter().enumerate() {
            let di = d.idx().expect("PITCH_DESTS entries are never None");
            for k in 0..STACK_LANES {
                tgt[i][k] = dest_block[k][di];
            }
        }
        tgt
    }

    /// Advance one sample toward `target`, return current smoothed state.
    #[inline]
    pub fn tick(
        &mut self,
        target: &[[f32; STACK_LANES]; N_PITCH_DESTS],
    ) -> &[[f32; STACK_LANES]; N_PITCH_DESTS] {
        let a = self.coeff;
        for i in 0..N_PITCH_DESTS {
            for k in 0..STACK_LANES {
                self.state[i][k] += a * (target[i][k] - self.state[i][k]);
            }
        }
        &self.state
    }

    /// Snap state to `target` without smoothing (preset load, voice steal).
    pub fn snap_to(&mut self, target: &[[f32; STACK_LANES]; N_PITCH_DESTS]) {
        self.state = *target;
    }

    pub fn current(&self) -> &[[f32; STACK_LANES]; N_PITCH_DESTS] {
        &self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_slot(source: SourceId, dest: DestId, depth: f32, curve: CurveKind) -> MatrixSlot {
        MatrixSlot { source, dest, depth, curve }
    }

    fn default_lane_sources() -> LaneSourceVals {
        let patch = PatchSources {
            lfo1: 0.5,
            mod_wheel: 0.3,
            aftertouch: 0.1,
        };
        let stack = StackScalarSources {
            pitch_eg_st: 1.5,
            mod_env: 0.7,
            velocity: 0.9,
            key: 0.45,
        };
        let mut lanes = LaneSources::default();
        for k in 0..STACK_LANES {
            lanes.lfo2[k] = -1.0 + (k as f32) * 0.25;
            lanes.voice_idx[k] = k as f32 / 7.0;
            lanes.voice_spread[k] = -1.0 + (k as f32) * 0.286;
            lanes.voice_rand[k] = (k as f32) * 0.127;
        }
        let mut out = [[0.0; N_SOURCES]; STACK_LANES];
        eval_sources(&patch, &stack, &lanes, &mut out);
        out
    }

    #[test]
    fn source_idx_skips_none_and_packs_others() {
        assert_eq!(SourceId::None.idx(), None);
        assert_eq!(SourceId::Lfo1.idx(), Some(0));
        assert_eq!(SourceId::VoiceRand.idx(), Some(N_SOURCES - 1));
    }

    #[test]
    fn dest_idx_skips_none_and_packs_others() {
        assert_eq!(DestId::None.idx(), None);
        assert_eq!(DestId::Op1Ratio.idx(), Some(0));
        assert_eq!(DestId::ReverbMix.idx(), Some(N_DESTS - 1));
    }

    #[test]
    fn pitch_shaped_set_matches_constant() {
        for d in PITCH_DESTS {
            assert!(d.is_pitch_shaped(), "{d:?} missing from is_pitch_shaped");
        }
        // Spot-check non-pitch-shaped dests.
        assert!(!DestId::Op1Level.is_pitch_shaped());
        assert!(!DestId::DelayMix.is_pitch_shaped());
        assert!(!DestId::StackDetune.is_pitch_shaped());
    }

    #[test]
    fn eval_sources_broadcasts_scalars_and_keeps_lane_values() {
        let sources = default_lane_sources();
        // Patch + stack scalars: same across lanes.
        for k in 0..STACK_LANES {
            assert_eq!(sources[k][SourceId::Lfo1.idx().unwrap()], 0.5);
            assert_eq!(sources[k][SourceId::ModWheel.idx().unwrap()], 0.3);
            assert_eq!(sources[k][SourceId::PitchEg.idx().unwrap()], 1.5);
            assert_eq!(sources[k][SourceId::Velocity.idx().unwrap()], 0.9);
        }
        // Lane-strided sources differ.
        let mut lfo2_vals = std::collections::HashSet::new();
        for k in 0..STACK_LANES {
            lfo2_vals.insert(sources[k][SourceId::Lfo2.idx().unwrap()].to_bits());
        }
        assert_eq!(lfo2_vals.len(), STACK_LANES);
    }

    #[test]
    fn empty_table_writes_zero_accumulator() {
        let table = MatrixTable::default();
        let sources = default_lane_sources();
        let mut out = [[42.0; N_DESTS]; STACK_LANES];
        eval_dests(&table, &sources, &mut out);
        for k in 0..STACK_LANES {
            for d in 0..N_DESTS {
                assert_eq!(out[k][d], 0.0, "lane {k} dest {d}");
            }
        }
    }

    #[test]
    fn single_lin_slot_writes_only_target_dest() {
        let mut table = MatrixTable::default();
        table.slots[0] = full_slot(SourceId::Lfo1, DestId::GlobalPitch, 0.5, CurveKind::Lin);
        let sources = default_lane_sources();
        let mut out = [[0.0; N_DESTS]; STACK_LANES];
        eval_dests(&table, &sources, &mut out);
        let pitch_idx = DestId::GlobalPitch.idx().unwrap();
        for k in 0..STACK_LANES {
            // Lfo1 = 0.5, depth = 0.5, lin → 0.25 across every lane.
            assert!(
                (out[k][pitch_idx] - 0.25).abs() < 1e-6,
                "lane {k} got {}",
                out[k][pitch_idx]
            );
            for d in 0..N_DESTS {
                if d == pitch_idx {
                    continue;
                }
                assert_eq!(out[k][d], 0.0, "lane {k} non-target dest {d}");
            }
        }
    }

    #[test]
    fn two_slots_into_same_dest_accumulate() {
        let mut table = MatrixTable::default();
        table.slots[0] = full_slot(SourceId::Lfo1, DestId::GlobalPitch, 0.5, CurveKind::Lin);
        table.slots[1] = full_slot(SourceId::ModWheel, DestId::GlobalPitch, 1.0, CurveKind::Lin);
        let sources = default_lane_sources();
        let mut out = [[0.0; N_DESTS]; STACK_LANES];
        eval_dests(&table, &sources, &mut out);
        let want = 0.5 * 0.5 + 1.0 * 0.3;
        for k in 0..STACK_LANES {
            assert!((out[k][DestId::GlobalPitch.idx().unwrap()] - want).abs() < 1e-6);
        }
    }

    #[test]
    fn per_lane_source_writes_distinct_lane_values() {
        let mut table = MatrixTable::default();
        table.slots[0] =
            full_slot(SourceId::VoiceSpread, DestId::Op1Pan, 1.0, CurveKind::Lin);
        let sources = default_lane_sources();
        let mut out = [[0.0; N_DESTS]; STACK_LANES];
        eval_dests(&table, &sources, &mut out);
        let pan_idx = DestId::Op1Pan.idx().unwrap();
        let mut distinct = std::collections::HashSet::new();
        for k in 0..STACK_LANES {
            distinct.insert(out[k][pan_idx].to_bits());
        }
        assert_eq!(distinct.len(), STACK_LANES);
    }

    #[test]
    fn empty_slot_skipped_when_source_none() {
        let mut table = MatrixTable::default();
        table.slots[0] = MatrixSlot {
            source: SourceId::None,
            dest: DestId::GlobalPitch,
            depth: 99.0,
            curve: CurveKind::Lin,
        };
        let sources = default_lane_sources();
        let mut out = [[0.0; N_DESTS]; STACK_LANES];
        eval_dests(&table, &sources, &mut out);
        for k in 0..STACK_LANES {
            assert_eq!(out[k][DestId::GlobalPitch.idx().unwrap()], 0.0);
        }
    }

    #[test]
    fn empty_slot_skipped_when_dest_none() {
        let mut table = MatrixTable::default();
        table.slots[0] = MatrixSlot {
            source: SourceId::Lfo1,
            dest: DestId::None,
            depth: 99.0,
            curve: CurveKind::Lin,
        };
        let sources = default_lane_sources();
        let mut out = [[0.0; N_DESTS]; STACK_LANES];
        eval_dests(&table, &sources, &mut out);
        for k in 0..STACK_LANES {
            for d in 0..N_DESTS {
                assert_eq!(out[k][d], 0.0);
            }
        }
    }

    #[test]
    fn zero_depth_short_circuits() {
        let mut table = MatrixTable::default();
        table.slots[0] = full_slot(SourceId::Lfo1, DestId::GlobalPitch, 0.0, CurveKind::Lin);
        let sources = default_lane_sources();
        let mut out = [[0.0; N_DESTS]; STACK_LANES];
        eval_dests(&table, &sources, &mut out);
        for k in 0..STACK_LANES {
            assert_eq!(out[k][DestId::GlobalPitch.idx().unwrap()], 0.0);
        }
    }

    #[test]
    fn curve_exp_more_extreme_than_lin() {
        // Source = 0.5 → lin = 0.5, exp = 0.25 (less extreme magnitude-wise
        // for |v| < 1, but characterised by the signed-square shape, not by
        // gain). Just verify it's different from lin.
        let mut lin_t = MatrixTable::default();
        lin_t.slots[0] = full_slot(SourceId::ModWheel, DestId::GlobalPitch, 1.0, CurveKind::Lin);
        let mut exp_t = MatrixTable::default();
        exp_t.slots[0] = full_slot(SourceId::ModWheel, DestId::GlobalPitch, 1.0, CurveKind::Exp);
        let sources = default_lane_sources();
        let mut lin_out = [[0.0; N_DESTS]; STACK_LANES];
        let mut exp_out = [[0.0; N_DESTS]; STACK_LANES];
        eval_dests(&lin_t, &sources, &mut lin_out);
        eval_dests(&exp_t, &sources, &mut exp_out);
        let pi = DestId::GlobalPitch.idx().unwrap();
        assert!(
            (lin_out[0][pi] - 0.3).abs() < 1e-6,
            "lin {} != 0.3",
            lin_out[0][pi]
        );
        assert!(
            (exp_out[0][pi] - 0.09).abs() < 1e-6,
            "exp {} != 0.09",
            exp_out[0][pi]
        );
    }

    #[test]
    fn curve_log_compresses_toward_zero() {
        let mut log_t = MatrixTable::default();
        log_t.slots[0] = full_slot(SourceId::ModWheel, DestId::GlobalPitch, 1.0, CurveKind::Log);
        let sources = default_lane_sources();
        let mut out = [[0.0; N_DESTS]; STACK_LANES];
        eval_dests(&log_t, &sources, &mut out);
        // ModWheel = 0.3, sqrt(0.3) ≈ 0.5477.
        let want = (0.3_f32).sqrt();
        assert!((out[0][DestId::GlobalPitch.idx().unwrap()] - want).abs() < 1e-6);
    }

    #[test]
    fn curve_bipolar_shifts_unipolar_source() {
        let mut bp_t = MatrixTable::default();
        bp_t.slots[0] = full_slot(SourceId::ModWheel, DestId::GlobalPitch, 1.0, CurveKind::Bipolar);
        let sources = default_lane_sources();
        let mut out = [[0.0; N_DESTS]; STACK_LANES];
        eval_dests(&bp_t, &sources, &mut out);
        // ModWheel = 0.3 → 2*0.3 - 1 = -0.4.
        assert!((out[0][DestId::GlobalPitch.idx().unwrap()] - (-0.4)).abs() < 1e-6);
    }

    #[test]
    fn curve_preserves_sign_for_lin_exp_log() {
        // Negative source preserves sign through Lin/Exp/Log.
        let patch = PatchSources::default();
        let stack = StackScalarSources::default();
        let mut lanes = LaneSources::default();
        for k in 0..STACK_LANES {
            lanes.voice_spread[k] = -0.5;
        }
        let mut sources = [[0.0; N_SOURCES]; STACK_LANES];
        eval_sources(&patch, &stack, &lanes, &mut sources);
        for curve in [CurveKind::Lin, CurveKind::Exp, CurveKind::Log] {
            let mut table = MatrixTable::default();
            table.slots[0] = full_slot(SourceId::VoiceSpread, DestId::GlobalPitch, 1.0, curve);
            let mut out = [[0.0; N_DESTS]; STACK_LANES];
            eval_dests(&table, &sources, &mut out);
            let v = out[0][DestId::GlobalPitch.idx().unwrap()];
            assert!(v < 0.0, "{curve:?} dropped sign: {v}");
        }
    }

    // --- PitchSmoother ---------------------------------------------------

    #[test]
    fn smoother_targets_from_picks_pitch_dest_columns() {
        let mut dest = [[0.0; N_DESTS]; STACK_LANES];
        let pitch_idx = DestId::GlobalPitch.idx().unwrap();
        let ratio_idx = DestId::Op1Ratio.idx().unwrap();
        for k in 0..STACK_LANES {
            dest[k][pitch_idx] = 1.0;
            dest[k][ratio_idx] = 0.25;
        }
        let s = PitchSmoother::default();
        let tgt = s.targets_from(&dest);
        let pidx = PITCH_DESTS.iter().position(|&d| d == DestId::GlobalPitch).unwrap();
        let ridx = PITCH_DESTS.iter().position(|&d| d == DestId::Op1Ratio).unwrap();
        for k in 0..STACK_LANES {
            assert_eq!(tgt[pidx][k], 1.0);
            assert_eq!(tgt[ridx][k], 0.25);
        }
    }

    #[test]
    fn smoother_glides_toward_target_over_block_time() {
        let sr = 48_000.0;
        let block_secs = 64.0 / sr;
        let mut s = PitchSmoother::new(block_secs, sr);
        let mut tgt = [[0.0; STACK_LANES]; N_PITCH_DESTS];
        for k in 0..STACK_LANES {
            tgt[0][k] = 1.0;
        }
        // Run ~10 blocks worth of samples; should converge well past 99%.
        for _ in 0..(10 * 64) {
            s.tick(&tgt);
        }
        for k in 0..STACK_LANES {
            assert!(
                (s.current()[0][k] - 1.0).abs() < 1e-2,
                "lane {k} got {}",
                s.current()[0][k]
            );
        }
    }

    #[test]
    fn smoother_snap_jumps_immediately() {
        let mut s = PitchSmoother::default();
        let mut tgt = [[0.0; STACK_LANES]; N_PITCH_DESTS];
        for k in 0..STACK_LANES {
            tgt[0][k] = 0.75;
        }
        s.snap_to(&tgt);
        assert_eq!(s.current()[0][0], 0.75);
    }

    #[test]
    fn patch_matrix_dispatches_by_layer() {
        let mut pm = PatchMatrix::default();
        pm.upper.slots[0] = full_slot(SourceId::Lfo1, DestId::GlobalPitch, 0.5, CurveKind::Lin);
        pm.lower.slots[0] = full_slot(SourceId::ModWheel, DestId::GlobalPitch, 1.0, CurveKind::Lin);
        assert_eq!(pm.table(Layer::Upper).slots[0].source, SourceId::Lfo1);
        assert_eq!(pm.table(Layer::Lower).slots[0].source, SourceId::ModWheel);
    }
}
