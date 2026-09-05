//! Band-limited mip-mapped wavetables for the vxn-4 operator block.
//!
//! ## Why mips, given the oversampling
//!
//! Oversampling does not band-limit the operator's *own* waveform — a naive
//! saw read from a table aliases at any rate. It only moves the fold point for
//! the PM sidebands generated downstream of the read. So the two mechanisms are
//! complementary and neither substitutes for the other: mips keep the source
//! spectrum clean, oversampling keeps the modulation products clean.
//!
//! ## Layout
//!
//! One flat allocation per waveform holds every mip back to back, with a small
//! [`MipMeta`] array carrying the per-mip offset and index arithmetic. One base
//! pointer, one L1-resident metadata array, and no pointer chase per lookup.
//!
//! Mip `k` has length `max(base_len >> k, MIN_LEN)` and carries `len / 2`
//! harmonics. Halving the length with the harmonic count keeps interpolation
//! error roughly constant across the set — a shorter table is only ever
//! selected for a waveform that has correspondingly fewer harmonics in it, so
//! the curve between adjacent entries is no sharper.
//!
//! ## The value+slope layout
//!
//! [`Tap`] stores `(v[i], v[i+1] - v[i])` rather than a plain `f32` series. A
//! linear read is then one 8-byte load and one FMA instead of two loads and a
//! subtract, and because a `Tap` is 8-byte aligned the pair can never straddle
//! a cache line — a plain `f32` table straddles on 1 index in 16. It costs 2x
//! the memory, which at the table lengths in play is nothing.
//!
//! [`Plain`] keeps the two-load form so the bench can price the difference
//! rather than assume it.

use std::f32::consts::TAU;

/// Number of mip levels per waveform.
///
/// 12 levels from a 2048-entry mip-0 bottoms out at [`MIN_LEN`] by level 7, so
/// the tail levels are duplicates. They are kept so that [`WaveTable::mip_for`]
/// can saturate on the index without a separate clamp, and because dropping to
/// a shorter mip-0 (the whole point of the bench) pushes the useful levels
/// further down the array.
pub const N_MIPS: usize = 12;

/// Shortest mip. 16 entries carry 8 harmonics, which is already past the top
/// octave of anything that will be used as a carrier.
pub const MIN_LEN: usize = 16;

/// The four waveforms the brief names. Assignable per operator.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Waveform {
    Sine,
    Triangle,
    Saw,
    Square,
}

impl Waveform {
    /// Every waveform, in table-index order.
    pub const ALL: [Waveform; 4] = [
        Waveform::Sine,
        Waveform::Triangle,
        Waveform::Saw,
        Waveform::Square,
    ];

    /// Index into [`WaveBank`]'s table array.
    #[inline]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// Amplitude of the `k`th harmonic (`k >= 1`), before normalisation.
    ///
    /// Standard series. Sign conventions are the textbook ones; for saw the
    /// alternating sign is dropped because a sawtooth and its mirror are the
    /// same waveform under a phase offset, and nothing here is phase-sensitive
    /// against an external reference.
    fn harmonic_amp(self, k: usize) -> f32 {
        let kf = k as f32;
        match self {
            Waveform::Sine => {
                if k == 1 {
                    1.0
                } else {
                    0.0
                }
            }
            Waveform::Saw => 1.0 / kf,
            Waveform::Square => {
                if k & 1 == 1 {
                    1.0 / kf
                } else {
                    0.0
                }
            }
            Waveform::Triangle => {
                if k & 1 == 1 {
                    let sign = if ((k - 1) / 2) & 1 == 0 { 1.0 } else { -1.0 };
                    sign / (kf * kf)
                } else {
                    0.0
                }
            }
        }
    }
}

/// One table entry: value and the slope to the next entry.
///
/// `align(8)` is load-bearing, not decoration — it is what guarantees the pair
/// never straddles a cache line.
#[repr(C, align(8))]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Tap {
    pub v: f32,
    pub d: f32,
}

/// Per-mip index arithmetic, hoisted out of the tables themselves.
#[derive(Clone, Copy, Debug, Default)]
pub struct MipMeta {
    /// First [`Tap`] of this mip within the flat tap array.
    pub off: u32,
    /// First `f32` of this mip within the flat plain array (stride `len + 1`).
    pub plain_off: u32,
    /// Entry count. Always a power of two.
    pub len: u32,
    /// `32 - log2(len)`. A Q32 phase shifted right by this yields the index.
    pub shift: u32,
    /// `(1 << shift) - 1` — the sub-entry fraction bits of a Q32 phase.
    pub frac_mask: u32,
    /// `1.0 / (1 << shift)`, so the fraction normalises with a multiply.
    pub frac_scale: f32,
}

/// Every mip of one waveform, in two layouts.
pub struct WaveTable {
    /// Value+slope layout, `len` entries per mip.
    pub taps: Box<[Tap]>,
    /// Plain layout, `len + 1` entries per mip; the guard entry duplicates
    /// index 0 so a wrapping read needs no branch.
    pub plain: Box<[f32]>,
    pub meta: [MipMeta; N_MIPS],
    /// Length of mip 0.
    pub base_len: usize,
    pub waveform: Waveform,
}

/// Length of mip `k`.
#[inline]
pub fn mip_len(base_len: usize, k: usize) -> usize {
    (base_len >> k.min(31)).max(MIN_LEN)
}

impl WaveTable {
    /// Build every mip of `w` by additive synthesis.
    ///
    /// Not real-time. Called once per `(waveform, base_len)` at init.
    ///
    /// The inner loop reads a single unit sine table by an integer stride
    /// rather than calling `sin` per sample per harmonic — `sin(k·2πi/L)` is
    /// `unit[(k·i) mod L]` exactly, which turns ~11M transcendental calls
    /// across a full bank into an add and a compare.
    pub fn generate(w: Waveform, base_len: usize) -> Self {
        assert!(
            base_len.is_power_of_two() && base_len >= MIN_LEN,
            "base_len must be a power of two >= {MIN_LEN}, got {base_len}"
        );

        let mut raw: Vec<Vec<f32>> = Vec::with_capacity(N_MIPS);
        for k in 0..N_MIPS {
            let len = mip_len(base_len, k);
            let harmonics = len / 2;

            let unit: Vec<f32> = (0..len)
                .map(|i| (TAU * i as f32 / len as f32).sin())
                .collect();

            let mut acc = vec![0.0f32; len];
            for h in 1..=harmonics {
                let amp = w.harmonic_amp(h);
                if amp == 0.0 {
                    continue;
                }
                // `h < len`, so the wrap is a single conditional subtract.
                let mut j = 0usize;
                for slot in acc.iter_mut() {
                    *slot += amp * unit[j];
                    j += h;
                    if j >= len {
                        j -= len;
                    }
                }
            }
            raw.push(acc);
        }

        // One scale for every mip, taken from the loudest of them. Normalising
        // each mip to its own peak would step the level as an operator crosses
        // a mip boundary under pitch modulation; normalising all of them
        // against mip 0 alone leaves the others free to exceed 1.0, because
        // Gibbs overshoot lands between mip 0's sample points and squarely on a
        // narrower mip's. Square at 256 overshoots by 5e-5 that way — inaudible,
        // but it makes the sum-bus headroom a claim rather than a fact.
        let peak = raw
            .iter()
            .flatten()
            .fold(0.0f32, |m, s| m.max(s.abs()));
        let scale = if peak > 0.0 { 1.0 / peak } else { 1.0 };

        let mut taps: Vec<Tap> = Vec::new();
        let mut plain: Vec<f32> = Vec::new();
        let mut meta = [MipMeta::default(); N_MIPS];

        for (k, mip) in raw.iter().enumerate() {
            let len = mip.len();
            let shift = 32 - len.trailing_zeros();

            meta[k] = MipMeta {
                off: taps.len() as u32,
                plain_off: plain.len() as u32,
                len: len as u32,
                shift,
                frac_mask: (1u32 << shift) - 1,
                frac_scale: 1.0 / (1u64 << shift) as f32,
            };

            for i in 0..len {
                let v = mip[i] * scale;
                let next = mip[(i + 1) % len] * scale;
                taps.push(Tap { v, d: next - v });
                plain.push(v);
            }
            plain.push(mip[0] * scale); // wrap guard
        }

        Self {
            taps: taps.into_boxed_slice(),
            plain: plain.into_boxed_slice(),
            meta,
            base_len,
            waveform: w,
        }
    }

    /// Pick the mip whose harmonic content stays under Nyquist for a Q32 phase
    /// increment of `inc` per oversampled tick.
    ///
    /// Normalised increment is `f = inc / 2^32`, so the top harmonic that fits
    /// below Nyquist is `0.5 / f = 2^31 / inc`. A mip of length `L` carries
    /// `L / 2` harmonics, so the constraint is `L <= 2^32 / inc`.
    ///
    /// Block rate, never per tick — and callers should apply hysteresis on top,
    /// because pitch modulation that dithers across a boundary will otherwise
    /// switch mips every block.
    pub fn mip_for(&self, inc: u32) -> usize {
        if inc == 0 {
            return 0;
        }
        let max_len = (u32::MAX / inc) as usize;
        let mut k = 0;
        while k + 1 < N_MIPS && mip_len(self.base_len, k) > max_len {
            k += 1;
        }
        k
    }

    /// Value+slope read: one 8-byte load, one FMA.
    ///
    /// `phase >> shift` is bounded by `2^(32 - shift) == len` by construction,
    /// so the bounds check here is provably never taken — it is left in because
    /// the DSP crates in this workspace carry no `unsafe`, and
    /// [`ValueSlopeUnchecked`] exists to price what keeping it costs.
    #[inline]
    pub fn lookup(&self, mip: usize, phase: u32) -> f32 {
        let m = self.meta[mip];
        let idx = (phase >> m.shift) as usize;
        let frac = (phase & m.frac_mask) as f32 * m.frac_scale;
        let t = self.taps[m.off as usize + idx];
        t.v + frac * t.d
    }

    /// Two-load lerp over the plain layout, reading the wrap guard at the top.
    #[inline]
    pub fn lookup_plain(&self, mip: usize, phase: u32) -> f32 {
        let m = self.meta[mip];
        let base = m.plain_off as usize + (phase >> m.shift) as usize;
        let frac = (phase & m.frac_mask) as f32 * m.frac_scale;
        let a = self.plain[base];
        let b = self.plain[base + 1];
        a + frac * (b - a)
    }

    /// [`Self::lookup`] without the bounds check.
    ///
    /// Added to measure the check rather than assume it. The check does not
    /// cost a branch — it costs **vector width**, which is the failure mode ADR
    /// 0002 §4 exists to catch. Disassembled from the linked `sweep` binary on
    /// aarch64, M1, rustc 1.95.0:
    ///
    /// | reader | bounds checks | widest NEON op | M lookups/s |
    /// |---|---|---|---|
    /// | this method | 0 | `fmul.4s` | 2693 |
    /// | [`Self::lookup_plain_unchecked`] | 0 | `fmul.4s` | 2131 |
    /// | [`Self::lookup`] | 9 | `fmul.2s` | 1750 |
    /// | [`Self::lookup_plain`] | 18 | none, scalar | 1347 |
    ///
    /// The gather cannot vectorise either way — NEON has no gather — but the
    /// index and interpolation arithmetic around it can, and each check halves
    /// the width it survives at.
    ///
    /// **This is nonetheless not the reader to ship.** In isolation the check
    /// costs 54%; inside the full operator block, where the lookup is one cost
    /// among many, it costs 4.6% (54.9 → 57.4 voices). That is not enough to
    /// buy `unsafe` into a DSP crate that currently has none. Keep
    /// [`Self::lookup`]; this method's job is to have made that a measurement
    /// rather than a preference.
    #[inline]
    pub fn lookup_unchecked(&self, mip: usize, phase: u32) -> f32 {
        let m = self.meta[mip];
        let idx = (phase >> m.shift) as usize;
        let frac = (phase & m.frac_mask) as f32 * m.frac_scale;
        debug_assert!(idx < m.len as usize);
        debug_assert!(m.off as usize + idx < self.taps.len());
        // SAFETY: `m.shift == 32 - log2(m.len)`, so `phase >> m.shift` is at
        // most `2^(32 - shift) - 1 == m.len - 1` for every `u32` phase. `m.off`
        // is the mip's start within `taps` and `generate` pushed exactly
        // `m.len` entries from there, so `m.off + idx` is in bounds. `mip` is
        // indexed into a fixed-size array above, which is checked.
        let t = unsafe { *self.taps.get_unchecked(m.off as usize + idx) };
        t.v + frac * t.d
    }

    /// [`Self::lookup_plain`] without its two bounds checks.
    ///
    /// Exists to keep the layout question separate from the check question.
    /// Plain measures slowest of the three above, but it also carries twice as
    /// many checks as value+slope, so on that evidence alone "value+slope wins"
    /// and "fewer bounds checks win" are the same claim. This variant tells
    /// them apart.
    #[inline]
    pub fn lookup_plain_unchecked(&self, mip: usize, phase: u32) -> f32 {
        let m = self.meta[mip];
        let base = m.plain_off as usize + (phase >> m.shift) as usize;
        let frac = (phase & m.frac_mask) as f32 * m.frac_scale;
        debug_assert!(base + 1 < self.plain.len());
        // SAFETY: as `lookup_unchecked`, plus the guard entry. `generate`
        // pushed `m.len + 1` floats from `m.plain_off`, and the index is at
        // most `m.len - 1`, so both `base` and `base + 1` are in bounds.
        let (a, b) = unsafe {
            (
                *self.plain.get_unchecked(base),
                *self.plain.get_unchecked(base + 1),
            )
        };
        a + frac * (b - a)
    }
}

/// Lookup strategy, resolved at monomorphisation.
///
/// ADR 0002 §4 forbids an enum-match inside a lane loop; this is the marker-type
/// form it prescribes instead. A `Lookup` value never exists at runtime.
pub trait Lookup: Copy {
    fn read(table: &WaveTable, mip: usize, phase: u32) -> f32;
}

/// Value+slope, bounds-checked. The candidate for shipping.
#[derive(Clone, Copy, Debug)]
pub struct ValueSlope;

/// Plain `f32` table, two loads and a subtract.
#[derive(Clone, Copy, Debug)]
pub struct Plain;

/// Value+slope with the (provably never-taken) bounds check removed.
#[derive(Clone, Copy, Debug)]
pub struct ValueSlopeUnchecked;

/// Plain two-load with its bounds checks removed. Isolates layout from checks.
#[derive(Clone, Copy, Debug)]
pub struct PlainUnchecked;

impl Lookup for ValueSlope {
    #[inline]
    fn read(table: &WaveTable, mip: usize, phase: u32) -> f32 {
        table.lookup(mip, phase)
    }
}

impl Lookup for Plain {
    #[inline]
    fn read(table: &WaveTable, mip: usize, phase: u32) -> f32 {
        table.lookup_plain(mip, phase)
    }
}

impl Lookup for ValueSlopeUnchecked {
    #[inline]
    fn read(table: &WaveTable, mip: usize, phase: u32) -> f32 {
        table.lookup_unchecked(mip, phase)
    }
}

impl Lookup for PlainUnchecked {
    #[inline]
    fn read(table: &WaveTable, mip: usize, phase: u32) -> f32 {
        table.lookup_plain_unchecked(mip, phase)
    }
}

/// The four waveforms at one mip-0 length.
pub struct WaveBank {
    tables: [WaveTable; 4],
    pub base_len: usize,
}

impl WaveBank {
    /// Generate every waveform. Not real-time; ~0.2s for `base_len == 2048`.
    pub fn new(base_len: usize) -> Self {
        Self {
            tables: [
                WaveTable::generate(Waveform::Sine, base_len),
                WaveTable::generate(Waveform::Triangle, base_len),
                WaveTable::generate(Waveform::Saw, base_len),
                WaveTable::generate(Waveform::Square, base_len),
            ],
            base_len,
        }
    }

    #[inline]
    pub fn table(&self, w: Waveform) -> &WaveTable {
        &self.tables[w.index()]
    }

    /// Bytes of tap data across the whole bank — the number that decides
    /// whether a voice's working set stays in L1.
    pub fn tap_bytes(&self) -> usize {
        self.tables
            .iter()
            .map(|t| t.taps.len() * size_of::<Tap>())
            .sum()
    }

    /// Bytes of tap data touched by one operator holding one mip.
    pub fn mip_tap_bytes(&self, mip: usize) -> usize {
        mip_len(self.base_len, mip) * size_of::<Tap>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both layouts must read the same curve, or the layout bench is comparing
    /// two different synths.
    #[test]
    fn plain_and_value_slope_agree() {
        let bank = WaveBank::new(256);
        for w in Waveform::ALL {
            let t = bank.table(w);
            for mip in [0usize, 1, 4] {
                for i in 0..4096u32 {
                    let phase = i.wrapping_mul(0x0010_0001);
                    let a = t.lookup(mip, phase);
                    let b = t.lookup_plain(mip, phase);
                    assert!(
                        (a - b).abs() < 1e-6,
                        "{w:?} mip {mip} phase {phase}: {a} vs {b}"
                    );
                }
            }
        }
    }

    #[test]
    fn unchecked_matches_checked_bit_exactly() {
        let bank = WaveBank::new(512);
        for w in Waveform::ALL {
            let t = bank.table(w);
            for mip in 0..N_MIPS {
                for i in 0..2048u32 {
                    let phase = i.wrapping_mul(0x1234_5679);
                    assert_eq!(t.lookup(mip, phase), t.lookup_unchecked(mip, phase));
                    assert_eq!(
                        t.lookup_plain(mip, phase),
                        t.lookup_plain_unchecked(mip, phase)
                    );
                }
            }
        }
    }

    /// The `unsafe` in the unchecked readers rests on `idx < len` holding for
    /// every `u32` phase. Check the claim directly across every mip of every
    /// bank size, rather than only where a render happens to land.
    #[test]
    fn every_phase_indexes_within_its_mip() {
        for base_len in [256usize, 512, 2048] {
            let bank = WaveBank::new(base_len);
            for w in Waveform::ALL {
                let t = bank.table(w);
                for (k, m) in t.meta.iter().enumerate() {
                    assert_eq!(m.len, mip_len(base_len, k) as u32);
                    assert_eq!(m.shift, 32 - m.len.trailing_zeros());
                    // The extremes bracket every intermediate value, since
                    // `phase >> shift` is monotonic in `phase`.
                    for phase in [0u32, 1, u32::MAX / 2, u32::MAX - 1, u32::MAX] {
                        let idx = (phase >> m.shift) as usize;
                        assert!(idx < m.len as usize, "{w:?} mip {k} phase {phase}");
                        assert!(m.off as usize + idx < t.taps.len());
                        assert!(m.plain_off as usize + idx + 1 < t.plain.len());
                    }
                }
            }
        }
    }

    #[test]
    fn sine_mip_is_a_sine() {
        let bank = WaveBank::new(2048);
        let t = bank.table(Waveform::Sine);
        for i in 0..2048usize {
            let phase = ((i as u64 * (1u64 << 32)) / 2048) as u32;
            let want = (TAU * i as f32 / 2048.0).sin();
            assert!(
                (t.lookup(0, phase) - want).abs() < 1e-4,
                "i={i}: {} vs {want}",
                t.lookup(0, phase)
            );
        }
    }

    #[test]
    fn every_mip_is_normalised_and_bounded() {
        for base_len in [256usize, 512, 2048] {
            let bank = WaveBank::new(base_len);
            for w in Waveform::ALL {
                let t = bank.table(w);
                for m in t.meta.iter() {
                    let s = m.off as usize;
                    let e = s + m.len as usize;
                    let peak = t.taps[s..e].iter().fold(0.0f32, |a, x| a.max(x.v.abs()));
                    // Exactly one mip touches 1.0 and the rest sit under it.
                    // Nothing may exceed it, or the sum-bus headroom is a lie.
                    assert!(peak <= 1.0, "{w:?} {base_len} peak {peak}");
                    assert!(peak > 0.4, "{w:?} {base_len} peak {peak} collapsed");
                }
            }
        }
    }

    #[test]
    fn mip_selection_keeps_harmonics_under_nyquist() {
        let bank = WaveBank::new(2048);
        let t = bank.table(Waveform::Saw);
        // Sweep phase increments from very slow to a quarter of Nyquist.
        for e in 0..24 {
            let inc = 1u32 << e;
            let k = t.mip_for(inc);
            let len = mip_len(2048, k);
            let harmonics = len / 2;
            let top = harmonics as f64 * (inc as f64 / 4_294_967_296.0);
            assert!(
                top <= 0.5 + 1e-9,
                "inc 2^{e} chose mip {k} (len {len}), top harmonic at {top} of fs"
            );
        }
    }

    #[test]
    fn mip_selection_saturates_at_the_shortest_mip() {
        let bank = WaveBank::new(2048);
        let t = bank.table(Waveform::Square);
        assert_eq!(t.mip_for(0), 0);
        assert_eq!(t.mip_for(1), 0);
        assert_eq!(t.mip_for(u32::MAX), N_MIPS - 1);
    }
}
