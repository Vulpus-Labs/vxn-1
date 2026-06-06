//! Host-tempo subdivision table.
//!
//! When an LFO's (or a delay's) sync is on, its rate/time control no longer
//! means free-running Hz / seconds — it selects a **musical subdivision**
//! locked to the host tempo. The control's normalised position picks a
//! subdivision from [`SUBDIVISIONS`] (coarse → fine), and
//! [`subdivision_hz`] / [`subdivision_seconds`] resolve that to an actual
//! frequency or period from the current BPM.
//!
//! Synth-specific concerns (which CLAP id pairs with which sync flag, how
//! the rate fader maps its normalised position) live with each synth's
//! parameter model.

/// Fallback tempo when the host provides none (no `HAS_TEMPO`). A sane
/// musical default so a synced LFO never stalls or NaNs absent transport.
pub const DEFAULT_TEMPO_BPM: f32 = 120.0;

/// One tempo-sync subdivision: its label and its length in **beats per
/// cycle** (quarter note = 1 beat). Straight = base, dotted = ×1.5,
/// triplet = ×2/3.
#[derive(Clone, Copy, Debug)]
pub struct Subdivision {
    pub label: &'static str,
    pub beats: f32,
}

const fn s(label: &'static str, beats: f32) -> Subdivision {
    Subdivision { label, beats }
}

const T: f32 = 2.0 / 3.0;

/// Subdivisions coarse → fine, each as straight / dotted / triplet,
/// 1/1 … 1/32. Labels match vxn-1's preset JSON — do not "improve" them.
pub static SUBDIVISIONS: [Subdivision; 18] = [
    s("1/1", 4.0),
    s("1/1.", 4.0 * 1.5),
    s("1/1T", 4.0 * T),
    s("1/2", 2.0),
    s("1/2.", 2.0 * 1.5),
    s("1/2T", 2.0 * T),
    s("1/4", 1.0),
    s("1/4.", 1.0 * 1.5),
    s("1/4T", 1.0 * T),
    s("1/8", 0.5),
    s("1/8.", 0.5 * 1.5),
    s("1/8T", 0.5 * T),
    s("1/16", 0.25),
    s("1/16.", 0.25 * 1.5),
    s("1/16T", 0.25 * T),
    s("1/32", 0.125),
    s("1/32.", 0.125 * 1.5),
    s("1/32T", 0.125 * T),
];

/// Map a normalised position `[0, 1]` to a subdivision index.
#[inline]
pub fn index_from_norm(norm: f32) -> usize {
    let last = SUBDIVISIONS.len() - 1;
    (norm.clamp(0.0, 1.0) * last as f32).round() as usize
}

/// Resolve a subdivision (by index) at `tempo_bpm` to a frequency in Hz.
/// Caller clamps to whatever Hz range it cares about.
#[inline]
pub fn subdivision_hz(tempo_bpm: f32, index: usize) -> f32 {
    let beats = SUBDIVISIONS[index.min(SUBDIVISIONS.len() - 1)].beats;
    // beats/sec ÷ beats/cycle = cycles/sec (Hz).
    (tempo_bpm / 60.0) / beats
}

/// Resolve a subdivision (by index) at `tempo_bpm` to a **duration in
/// seconds**. Used by tempo-synced delay: the period, not the rate.
#[inline]
pub fn subdivision_seconds(tempo_bpm: f32, index: usize) -> f32 {
    let beats = SUBDIVISIONS[index.min(SUBDIVISIONS.len() - 1)].beats;
    // beats/cycle ÷ beats/sec = sec/cycle.
    beats / (tempo_bpm / 60.0)
}

/// Label for a subdivision index. Clamps out-of-range to the last entry.
#[inline]
pub fn subdivision_label(index: usize) -> &'static str {
    SUBDIVISIONS[index.min(SUBDIVISIONS.len() - 1)].label
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarter_note_matches_beat_math() {
        // 1/4 cycles once per beat: at 120 BPM that's 2 Hz; at 90, 1.5 Hz.
        let q = SUBDIVISIONS.iter().position(|s| s.label == "1/4").unwrap();
        assert!((subdivision_hz(120.0, q) - 2.0).abs() < 1e-5);
        assert!((subdivision_hz(90.0, q) - 1.5).abs() < 1e-5);
        // 1/8 is twice as fast.
        let e = SUBDIVISIONS.iter().position(|s| s.label == "1/8").unwrap();
        assert!((subdivision_hz(90.0, e) - 3.0).abs() < 1e-5);
    }

    #[test]
    fn dotted_and_triplet_scale_the_straight_rate() {
        let q = SUBDIVISIONS.iter().position(|s| s.label == "1/4").unwrap();
        let qd = SUBDIVISIONS.iter().position(|s| s.label == "1/4.").unwrap();
        let qt = SUBDIVISIONS.iter().position(|s| s.label == "1/4T").unwrap();
        for bpm in [90.0_f32, 140.0] {
            let straight = subdivision_hz(bpm, q);
            assert!(
                (subdivision_hz(bpm, qd) - straight / 1.5).abs() < 1e-4,
                "dotted {bpm}"
            );
            assert!(
                (subdivision_hz(bpm, qt) - straight * 1.5).abs() < 1e-4,
                "triplet {bpm}"
            );
        }
    }

    #[test]
    fn seconds_is_period_of_hz() {
        let q = SUBDIVISIONS.iter().position(|s| s.label == "1/4").unwrap();
        assert!((subdivision_seconds(120.0, q) - 0.5).abs() < 1e-6);
        assert!((subdivision_seconds(60.0, q) - 1.0).abs() < 1e-6);
        for bpm in [60.0_f32, 128.0, 174.0] {
            for idx in 0..SUBDIVISIONS.len() {
                assert!(
                    (subdivision_seconds(bpm, idx) - 1.0 / subdivision_hz(bpm, idx)).abs() < 1e-4
                );
            }
        }
    }

    #[test]
    fn norm_maps_across_the_whole_table() {
        assert_eq!(index_from_norm(0.0), 0);
        assert_eq!(index_from_norm(1.0), SUBDIVISIONS.len() - 1);
        assert_eq!(index_from_norm(-1.0), 0);
        assert_eq!(index_from_norm(2.0), SUBDIVISIONS.len() - 1);
    }

    #[test]
    fn subdivision_label_round_trips() {
        for (i, s) in SUBDIVISIONS.iter().enumerate() {
            assert_eq!(subdivision_label(i), s.label);
        }
    }
}
