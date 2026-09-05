//! Note sequences to play the engine with.
//!
//! Each one is chosen to put a specific question in front of your ears:
//!
//! - `chord` — does it sound like anything? Held triad, then release.
//! - `scale` — five octaves chromatic. **The mip-transition test.** Band
//!   changes happen mid-run, so any level step or timbre jump at a mip boundary
//!   shows up as a bump in an otherwise even sweep.
//! - `arp` — fast repeated onsets. Tests envelope retrigger and lane reuse.
//! - `steal` — 24 overlapping notes against a 16-voice cap. **The allocator
//!   test.** You should hear notes disappear, but never a click.
//! - `vel` — the same note at rising velocity, for the envelope ceiling.
//! - `high` — sustained notes at the top of the keyboard. **The oversampling
//!   A/B.** Everything else is too low, too short, or both: the 8x-vs-16x
//!   divergence grows steeply with pitch, and judging timbre needs a held note,
//!   not an 81 ms one.

use vxn4_engine::Engine;

#[derive(Clone, Copy, Debug)]
pub enum Ev {
    On(u8, u8),
    Off(u8),
}

pub struct Sequence {
    pub name: &'static str,
    /// Total render length, including the tail after the last event.
    pub secs: f32,
    pub build: fn() -> Vec<(f32, Ev)>,
}

pub static SEQUENCES: &[Sequence] = &[
    Sequence {
        name: "chord",
        secs: 6.0,
        build: chord,
    },
    Sequence {
        name: "scale",
        secs: 8.0,
        build: scale,
    },
    Sequence {
        name: "arp",
        secs: 6.0,
        build: arp,
    },
    Sequence {
        name: "steal",
        secs: 8.0,
        build: steal,
    },
    Sequence {
        name: "vel",
        secs: 8.0,
        build: vel,
    },
    Sequence {
        name: "high",
        secs: 10.0,
        build: high,
    },
];

/// A held minor 9th, then release. The "what does this patch sound like" test.
fn chord() -> Vec<(f32, Ev)> {
    let notes = [48u8, 55, 60, 63, 67, 70];
    let mut e = Vec::new();
    for (i, n) in notes.iter().enumerate() {
        // Slight spread so the onsets are distinguishable.
        e.push((0.05 * i as f32, Ev::On(*n, 100)));
    }
    for n in notes.iter() {
        e.push((3.0, Ev::Off(*n)));
    }
    e
}

/// Chromatic over five octaves. Mip boundaries are crossed several times.
fn scale() -> Vec<(f32, Ev)> {
    let mut e = Vec::new();
    let step = 0.09;
    for (i, note) in (36u8..96).enumerate() {
        let t = i as f32 * step;
        e.push((t, Ev::On(note, 100)));
        e.push((t + step * 0.9, Ev::Off(note)));
    }
    e
}

/// Fast arpeggio — repeated onsets on a small set of pitches, so lanes are
/// reused while their predecessors are still releasing.
fn arp() -> Vec<(f32, Ev)> {
    let pattern = [60u8, 63, 67, 70, 72, 70, 67, 63];
    let mut e = Vec::new();
    let step = 0.075;
    for i in 0..48 {
        let t = i as f32 * step;
        let n = pattern[i % pattern.len()];
        e.push((t, Ev::On(n, 96)));
        e.push((t + step * 0.8, Ev::Off(n)));
    }
    e
}

/// 24 long overlapping notes against a 16-voice cap.
///
/// Everything is held — no note-offs until the end — so the allocator has to
/// steal, repeatedly, with every voice `Held` rather than conveniently
/// releasing. Listen for clicks at the steal points; there should be none, and
/// the notes that vanish should be the quiet ones.
fn steal() -> Vec<(f32, Ev)> {
    let mut e = Vec::new();
    for i in 0..24u8 {
        // Rising velocity, so the quietest-first rule has something to prefer
        // and the early (quiet) notes are the ones that should disappear.
        let vel = 40 + i * 3;
        e.push((0.12 * i as f32, Ev::On(40 + i * 2, vel)));
    }
    for i in 0..24u8 {
        e.push((5.0, Ev::Off(40 + i * 2)));
    }
    e
}

/// One note, rising velocity. The envelope-ceiling test.
fn vel() -> Vec<(f32, Ev)> {
    let mut e = Vec::new();
    for i in 0..8u8 {
        let t = i as f32 * 0.85;
        let v = 8 + i * 17;
        e.push((t, Ev::On(60, v.min(127))));
        e.push((t + 0.6, Ev::Off(60)));
    }
    e
}

/// Sustained notes up the top two octaves, for the 8x-vs-16x comparison.
///
/// One note at a time, held long enough to judge, with a gap between so each is
/// heard on its own. The pitches are the ones the `alias` analysis flags: the
/// divergence is ~-32 dB at note 48 and ~-10 dB at note 102 on `grind`, so this
/// deliberately sits where the difference is largest rather than where the
/// music usually is.
fn high() -> Vec<(f32, Ev)> {
    let mut e = Vec::new();
    for (i, note) in [84u8, 96, 102, 108, 96, 102].iter().enumerate() {
        let t = i as f32 * 1.6;
        e.push((t, Ev::On(*note, 100)));
        e.push((t + 1.35, Ev::Off(*note)));
    }
    e
}

/// Play a sequence through the engine, returning stereo at the host rate.
///
/// Events are applied at sample-accurate positions by cutting the render at
/// each event time, rather than quantising them to a block boundary — the
/// allocator's behaviour under a fast burst is part of what is being listened
/// to, and rounding onsets to a block would blur exactly that.
pub fn render_sequence(engine: &mut Engine, seq: &Sequence, sr: f32) -> (Vec<f32>, Vec<f32>) {
    render_sequence_for(engine, seq, sr, seq.secs)
}

/// [`render_sequence`], truncated to `secs`.
///
/// Only the tests use the truncation, and they need it: `cargo test` runs in
/// debug, where this DSP is slow enough that rendering every sequence against
/// every patch in full took over five minutes. The interesting behaviour of
/// each sequence is in its opening seconds.
pub fn render_sequence_for(
    engine: &mut Engine,
    seq: &Sequence,
    sr: f32,
    secs: f32,
) -> (Vec<f32>, Vec<f32>) {
    let mut events = (seq.build)();
    events.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let total = (secs * sr) as usize;
    let mut left = vec![0.0f32; total];
    let mut right = vec![0.0f32; total];

    let mut cursor = 0usize;
    for (t, ev) in events {
        let at = ((t * sr) as usize).min(total);
        if at > cursor {
            engine.process(&mut left[cursor..at], &mut right[cursor..at]);
            cursor = at;
        }
        match ev {
            Ev::On(n, v) => engine.note_on(n, v),
            Ev::Off(n) => engine.note_off(n),
        }
    }
    if cursor < total {
        engine.process(&mut left[cursor..], &mut right[cursor..]);
    }

    (left, right)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;
    /// Enough to cover each sequence's opening behaviour without paying for the
    /// full render in a debug build.
    const PROBE: f32 = 2.0;

    #[test]
    fn every_sequence_produces_sound_on_every_patch() {
        for s in SEQUENCES.iter() {
            for p in 0..vxn4_engine::N_PATCHES {
                let mut e = Engine::new(SR);
                e.set_patch(p);
                let (l, r) = render_sequence_for(&mut e, s, SR, PROBE);
                assert_eq!(l.len(), (PROBE * SR) as usize);
                let peak = l.iter().chain(r.iter()).fold(0.0f32, |m, x| m.max(x.abs()));
                assert!(peak > 0.01, "{} on patch {p} was silent", s.name);
                assert!(peak <= 1.0, "{} on patch {p} clipped at {peak}", s.name);
                assert!(l.iter().chain(r.iter()).all(|x| x.is_finite()));
            }
        }
    }

    #[test]
    fn a_full_length_render_is_the_right_size() {
        let s = &SEQUENCES[0];
        let mut e = Engine::new(SR);
        let (l, r) = render_sequence(&mut e, s, SR);
        assert_eq!(l.len(), (s.secs * SR) as usize);
        assert_eq!(r.len(), l.len());
    }

    /// The steal sequence must actually drive the allocator past its cap —
    /// otherwise it is not testing what it claims to.
    #[test]
    fn the_steal_sequence_exceeds_the_voice_cap() {
        let ons = (SEQUENCES.iter().find(|s| s.name == "steal").unwrap().build)()
            .iter()
            .filter(|(_, e)| matches!(e, Ev::On(..)))
            .count();
        assert!(
            ons > vxn4_engine::N_ACTIVE,
            "steal issues only {ons} notes against a cap of {}",
            vxn4_engine::N_ACTIVE
        );
    }

    /// A steal must not click. A click is a discontinuity, so bound the
    /// sample-to-sample delta well under what legal signal can produce.
    ///
    /// The first steal lands at note 17, ~2.0 s in, so this has to run past
    /// `PROBE` to reach the behaviour it is testing.
    #[test]
    fn stealing_does_not_click() {
        let s = SEQUENCES.iter().find(|s| s.name == "steal").unwrap();
        for p in 0..vxn4_engine::N_PATCHES {
            let mut e = Engine::new(SR);
            e.set_patch(p);
            let (l, _) = render_sequence_for(&mut e, s, SR, 3.5);
            let max_step = l.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
            // A 20 kHz full-scale sine steps by ~0.9 between samples at 48 k, so
            // anything at or below that is indistinguishable from legal signal.
            assert!(
                max_step < 0.9,
                "patch {p} stepped {max_step} between adjacent samples"
            );
        }
    }
}
