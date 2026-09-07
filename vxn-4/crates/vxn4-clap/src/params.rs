//! VXN4's host parameter table: eleven params, and deliberately no more.
//!
//! ## What the host does not see
//!
//! The synth has 72 modulatable destinations, each taking two sources. Exposing
//! that would be several hundred automation lanes, and would bake the patch's
//! routing topology into every saved project — rewire a patch and every lane
//! that named a route is pointing at something else.
//!
//! So modulation reaches the host as **eight macro knobs and nothing else**.
//! The knobs are matrix *sources*; which routes each one drives, and how far,
//! is patch state ([`vxn4_engine::matrix`] carries the argument in full). A
//! host automates intent, and a lane that says "macro 3" survives the patch
//! behind it being rewired.
//!
//! That leaves: which patch, how much oversampling, how loud, and the eight
//! knobs.
//!
//! ## Ids
//!
//! `clap_id == param_index`, dense and contiguous, so the host can enumerate by
//! index and look up by id through the one positional [`decode`]. Ids are
//! computed rather than accreted, which keeps them stable across sessions
//! without an append-only rule — the same scheme vxn-3 uses.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU32, Ordering};

use vxn4_engine::{MAX_MASTER_GAIN, N_MACROS, N_PATCHES, Quality, patch_names};

/// Params before the macro block: patch, quality, master gain.
pub const N_FIXED: usize = 3;

/// Total host params.
pub const TOTAL_PARAMS: usize = N_FIXED + N_MACROS;

/// The decoded meaning of a `clap_id`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Slot {
    /// Which of the hardwired patches. Stepped; changing it kills all sound.
    Patch,
    /// Operator-block oversampling: 8x or 16x. Stepped.
    Quality,
    /// Output trim, applied upstream of the limiter.
    MasterGain,
    /// Macro knob `0..N_MACROS`, a modulation matrix source.
    Macro(u8),
}

/// Decode a `clap_id`, or `None` if out of range.
pub fn decode(id: usize) -> Option<Slot> {
    Some(match id {
        0 => Slot::Patch,
        1 => Slot::Quality,
        2 => Slot::MasterGain,
        _ if id < TOTAL_PARAMS => Slot::Macro((id - N_FIXED) as u8),
        _ => return None,
    })
}

/// A slot's `(min, max, default, stepped)` for `param_info`.
///
/// Stepped params carry their **inclusive** max as a count minus one, which is
/// what CLAP means by a stepped range — `0..=5` is six patches, not five.
pub fn range(slot: Slot) -> (f32, f32, f32, bool) {
    match slot {
        Slot::Patch => (0.0, (N_PATCHES - 1) as f32, 0.0, true),
        Slot::Quality => (0.0, 1.0, 0.0, true),
        Slot::MasterGain => (0.0, MAX_MASTER_GAIN, 1.0, false),
        // Zero, not centre: every macro at zero is the patch exactly as its
        // table writes it, and a default of 0.5 would mean no patch ever
        // sounded as authored without the player pulling eight knobs down.
        Slot::Macro(_) => (0.0, 1.0, 0.0, false),
    }
}

/// The default value of a param id, used to seed the cache so `get_value`
/// matches a fresh engine.
pub fn default_value(id: usize) -> f32 {
    decode(id).map(|s| range(s).2).unwrap_or(0.0)
}

/// Clamp a host write into the slot's declared range.
///
/// Hosts are not required to respect `min_value`/`max_value`, and a rogue write
/// reaching the engine as a patch index would be an out-of-bounds read. The
/// engine clamps too; this makes the *cache* — which is what state save writes
/// and `get_value` reports — honest as well.
pub fn clamp(slot: Slot, value: f32) -> f32 {
    let (min, max, ..) = range(slot);
    if value.is_nan() {
        min
    } else {
        value.clamp(min, max)
    }
}

/// Quality from its parameter value.
pub fn quality_from(value: f32) -> Quality {
    if value >= 0.5 {
        Quality::X16
    } else {
        Quality::X8
    }
}

/// Patch index from its parameter value.
pub fn patch_from(value: f32) -> usize {
    (value.round().max(0.0) as usize).min(N_PATCHES - 1)
}

pub fn write_name(slot: Slot, out: &mut String) {
    let _ = match slot {
        Slot::Patch => out.write_str("Patch"),
        Slot::Quality => out.write_str("Quality"),
        Slot::MasterGain => out.write_str("Master Gain"),
        Slot::Macro(m) => write!(out, "Macro {}", m + 1),
    };
}

/// The group path a slot belongs to.
pub fn write_module(slot: Slot, out: &mut String) {
    let _ = match slot {
        Slot::Patch | Slot::Quality | Slot::MasterGain => out.write_str("Global"),
        Slot::Macro(_) => out.write_str("Macros"),
    };
}

pub fn write_value_text(slot: Slot, value: f32, out: &mut String) {
    let _ = match slot {
        Slot::Patch => out.write_str(patch_names()[patch_from(value)]),
        Slot::Quality => write!(out, "{}x", quality_from(value).factor()),
        Slot::MasterGain => write_db(value, out),
        Slot::Macro(_) => write!(out, "{:.0}%", value * 100.0),
    };
}

/// Invert [`write_value_text`] for host text edits.
///
/// `value → text → value → text` has to be stable, which `clap-validator`
/// checks. For the stepped params that means parsing the *name* back, since
/// that is what the text says — a patch reads "bell", not "2".
pub fn parse_value(slot: Slot, text: &str) -> Option<f32> {
    let t = text.trim();
    match slot {
        Slot::Patch => patch_names()
            .iter()
            .position(|n| n.eq_ignore_ascii_case(t))
            .map(|i| i as f32)
            .or_else(|| leading_number(t)),
        Slot::Quality => match leading_number(t)? as i32 {
            16 => Some(1.0),
            8 => Some(0.0),
            // A bare 0 or 1 is the raw parameter value rather than a factor,
            // which is what a host round-tripping `get_value` will hand back.
            n => Some(if n >= 1 { 1.0 } else { 0.0 }),
        },
        Slot::MasterGain => {
            if t.starts_with("-inf") {
                return Some(0.0);
            }
            Some(10f32.powf(leading_number(t)? / 20.0))
        }
        Slot::Macro(_) => Some(leading_number(t)? / 100.0),
    }
}

/// The leading numeric token of `s`, ignoring any unit suffix.
fn leading_number(s: &str) -> Option<f32> {
    let num: String = s
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    num.parse::<f32>().ok()
}

fn write_db(lin: f32, out: &mut String) -> std::fmt::Result {
    if lin <= 1e-4 {
        out.write_str("-inf dB")
    } else {
        write!(out, "{:.1} dB", 20.0 * lin.log10())
    }
}

/// Thread-safe cache of the current host-facing param values.
///
/// The audio thread writes it as automation lands; the main thread reads it for
/// `get_value` and for state save, and writes it on an inactive flush. Seeded
/// to each param's default so a fresh instance reports the engine's real
/// starting state rather than zeroes.
pub struct ParamCache {
    vals: [AtomicU32; TOTAL_PARAMS],
}

impl ParamCache {
    pub fn new() -> Self {
        Self {
            vals: std::array::from_fn(|i| AtomicU32::new(default_value(i).to_bits())),
        }
    }

    #[inline]
    pub fn get(&self, id: usize) -> f32 {
        f32::from_bits(self.vals[id].load(Ordering::Relaxed))
    }

    /// Store `value`, clamped to the slot's range. Out-of-range ids are
    /// dropped.
    #[inline]
    pub fn set(&self, id: usize, value: f32) {
        let Some(slot) = decode(id) else { return };
        self.vals[id].store(clamp(slot, value).to_bits(), Ordering::Relaxed);
    }
}

impl Default for ParamCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_eleven_params() {
        assert_eq!(TOTAL_PARAMS, 11);
        assert_eq!(N_MACROS, 8);
    }

    #[test]
    fn decode_covers_the_table_and_nothing_past_it() {
        for id in 0..TOTAL_PARAMS {
            assert!(decode(id).is_some(), "id {id} should decode");
        }
        assert_eq!(decode(TOTAL_PARAMS), None);
        assert_eq!(decode(usize::MAX), None);
    }

    #[test]
    fn the_positional_scheme_is_stable() {
        assert_eq!(decode(0), Some(Slot::Patch));
        assert_eq!(decode(1), Some(Slot::Quality));
        assert_eq!(decode(2), Some(Slot::MasterGain));
        assert_eq!(decode(N_FIXED), Some(Slot::Macro(0)));
        assert_eq!(decode(TOTAL_PARAMS - 1), Some(Slot::Macro(7)));
    }

    /// Every macro defaults to zero, which is what makes "the patch as
    /// authored" the state a fresh instance is in.
    #[test]
    fn macros_default_to_zero() {
        for m in 0..N_MACROS {
            assert_eq!(default_value(N_FIXED + m), 0.0);
        }
    }

    /// A stepped param's max is the top *index*, not the count. Off by one here
    /// and the last patch is unreachable from a host's generic UI.
    #[test]
    fn stepped_ranges_are_inclusive_indices() {
        let (min, max, _, stepped) = range(Slot::Patch);
        assert!(stepped);
        assert_eq!(min, 0.0);
        assert_eq!(max, (N_PATCHES - 1) as f32);
        assert_eq!(patch_from(max), N_PATCHES - 1);
        assert_eq!(range(Slot::Quality).1, 1.0);
    }

    #[test]
    fn a_hostile_write_cannot_leave_the_declared_range() {
        assert_eq!(clamp(Slot::Patch, 999.0), (N_PATCHES - 1) as f32);
        assert_eq!(clamp(Slot::Patch, -5.0), 0.0);
        assert_eq!(clamp(Slot::Macro(0), 4.0), 1.0);
        assert_eq!(clamp(Slot::MasterGain, 100.0), MAX_MASTER_GAIN);
        assert_eq!(clamp(Slot::Macro(0), f32::NAN), 0.0);
        // And through the cache, which is what state save writes out.
        let c = ParamCache::new();
        c.set(0, 999.0);
        assert_eq!(c.get(0), (N_PATCHES - 1) as f32);
    }

    #[test]
    fn patch_and_quality_decode_from_their_values() {
        for p in 0..N_PATCHES {
            assert_eq!(patch_from(p as f32), p);
        }
        // Hosts send a stepped param as a float; rounding, not truncation, is
        // what makes 1.9999 the patch the user picked.
        assert_eq!(patch_from(1.9999), 2);
        assert_eq!(quality_from(0.0), Quality::X8);
        assert_eq!(quality_from(1.0), Quality::X16);
    }

    /// `value → text → value → text` must be stable, which is what
    /// `clap-validator`'s param-conversions check asserts.
    #[test]
    fn value_text_round_trips() {
        let mut cases = vec![
            (Slot::Quality, 0.0),
            (Slot::Quality, 1.0),
            (Slot::MasterGain, 1.0),
            (Slot::MasterGain, 0.0),
            (Slot::MasterGain, 0.5),
            (Slot::Macro(0), 0.0),
            (Slot::Macro(3), 0.42),
            (Slot::Macro(7), 1.0),
        ];
        for p in 0..N_PATCHES {
            cases.push((Slot::Patch, p as f32));
        }
        for (slot, v) in cases {
            let mut s1 = String::new();
            write_value_text(slot, v, &mut s1);
            let v2 = parse_value(slot, &s1).unwrap_or_else(|| panic!("parse {slot:?} {s1:?}"));
            let mut s2 = String::new();
            write_value_text(slot, v2, &mut s2);
            assert_eq!(s1, s2, "{slot:?} v={v} unstable: {s1:?} vs {s2:?}");
        }
    }

    /// A patch reads by name, so the round trip has to go through the name
    /// table — and every patch name must be distinct for that to work at all.
    #[test]
    fn a_patch_renders_and_parses_by_name() {
        let mut s = String::new();
        write_value_text(Slot::Patch, 2.0, &mut s);
        assert_eq!(s, patch_names()[2]);
        assert_eq!(parse_value(Slot::Patch, &s), Some(2.0));

        let mut names = patch_names().to_vec();
        names.sort_unstable();
        let n = names.len();
        names.dedup();
        assert_eq!(
            names.len(),
            n,
            "duplicate patch name — text edits are ambiguous"
        );
    }

    #[test]
    fn quality_renders_as_a_factor() {
        let mut s = String::new();
        write_value_text(Slot::Quality, 1.0, &mut s);
        assert_eq!(s, "16x");
        s.clear();
        write_value_text(Slot::Quality, 0.0, &mut s);
        assert_eq!(s, "8x");
    }

    #[test]
    fn the_cache_seeds_defaults_and_reads_back() {
        let c = ParamCache::new();
        assert_eq!(c.get(0), 0.0);
        assert_eq!(c.get(2), 1.0); // unity master gain
        c.set(2, 0.25);
        assert_eq!(c.get(2), 0.25);
        // Out of range is dropped, not panicked on.
        c.set(TOTAL_PARAMS, 1.0);
    }
}
