//! Integration tests for the 0048 pattern engine: polymeter phasing, per-trig
//! probability, retrig n-over-m (curves + velocity ramp), sample-accuracy across
//! blocks and transport jumps, and an allocation-free callback.
//!
//! Most assertions use a spy `TrackEngine` that records the absolute sample
//! position + velocity of every trig, so scheduling is checked exactly,
//! independent of DSP.

use std::sync::{Arc, Mutex};

use vxn3_engine::engine::Engine;
use vxn3_engine::sequencer::{Retrig, RetrigCurve};
use vxn3_engine::track_engine::{EngineKind, TrackEngine};
use vxn3_engine::transport::Transport;

const SR: f32 = 48_000.0;
const BPM: f64 = 120.0;
/// Samples per 16th note at 120 BPM / 48 kHz.
const STEP: usize = 6_000;

// ── Allocation trap ───────────────────────────────────────────────────────────

mod alloc_trap {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    thread_local! {
        static ARMED: Cell<bool> = const { Cell::new(false) };
        static COUNT: Cell<usize> = const { Cell::new(0) };
    }
    struct A;
    // SAFETY: forwards to System, only adds a TLS counter bump while armed.
    unsafe impl GlobalAlloc for A {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            if ARMED.with(Cell::get) {
                COUNT.with(|c| c.set(c.get() + 1));
            }
            unsafe { System.alloc(l) }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            unsafe { System.dealloc(p, l) }
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
            if ARMED.with(Cell::get) {
                COUNT.with(|c| c.set(c.get() + 1));
            }
            unsafe { System.realloc(p, l, n) }
        }
    }
    #[global_allocator]
    static G: A = A;
    pub fn count_allocs(f: impl FnOnce()) -> usize {
        COUNT.with(|c| c.set(0));
        ARMED.with(|a| a.set(true));
        f();
        ARMED.with(|a| a.set(false));
        COUNT.with(Cell::get)
    }
}

// ── Spy engine ────────────────────────────────────────────────────────────────

type Log = Arc<Mutex<Vec<(usize, f32)>>>; // (sample position, velocity)

struct Spy {
    pos: usize,
    log: Log,
}
impl TrackEngine for Spy {
    fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        self.pos += out.len();
    }
    fn on_trig(&mut self, _note: f32, velocity: f32) {
        self.log.lock().unwrap().push((self.pos, velocity));
    }
    fn reset(&mut self) {
        self.pos = 0;
    }
    fn set_sample_rate(&mut self, _sr: f32) {}
    fn kind(&self) -> EngineKind {
        EngineKind::KickTone
    }
}

fn spy_on(engine: &mut Engine, track: usize) -> Log {
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    engine.track_mut(track).engine = Box::new(Spy {
        pos: 0,
        log: log.clone(),
    });
    log
}

/// Render `total` frames in `block`-sized chunks while playing, feeding the host
/// transport's advancing beat clock each block.
fn run(engine: &mut Engine, total: usize, block: usize) {
    let bps = BPM / 60.0 / SR as f64;
    let mut p = 0usize;
    while p < total {
        let n = block.min(total - p);
        engine.set_transport(Transport {
            playing: true,
            tempo_bpm: BPM,
            song_pos_beats: Some(p as f64 * bps),
        });
        let mut l = vec![0.0_f32; n];
        let mut r = vec![0.0_f32; n];
        engine.process_block(&mut l, &mut r);
        p += n;
    }
}

fn positions(log: &Log) -> Vec<usize> {
    log.lock().unwrap().iter().map(|&(p, _)| p).collect()
}

// ── Polymeter ─────────────────────────────────────────────────────────────────

#[test]
fn tracks_of_different_lengths_phase() {
    let mut engine = Engine::new(SR, 512);
    // Track 0: length 16; track 1: length 12. Both fire step 0 only, same tick.
    engine.pattern_mut(0).len = 16;
    engine.pattern_mut(0).set(0, 36.0, 1.0);
    engine.pattern_mut(1).len = 12;
    engine.pattern_mut(1).set(0, 36.0, 1.0);
    let log0 = spy_on(&mut engine, 0);
    let log1 = spy_on(&mut engine, 1);

    run(&mut engine, 40 * STEP, 512); // 40 steps — long enough to show drift

    // Track 0 fires every 16 steps, track 1 every 12 → different periods, so
    // after coinciding at 0 they drift apart.
    assert_eq!(positions(&log0), vec![0, 16 * STEP, 32 * STEP]);
    assert_eq!(positions(&log1), vec![0, 12 * STEP, 24 * STEP, 36 * STEP]);
}

// ── Probability ───────────────────────────────────────────────────────────────

/// Count trigs over `n_steps` active steps at fire probability `p`.
fn count_at_probability(p: f32, n_steps: usize) -> usize {
    let mut engine = Engine::new(SR, 512);
    engine.pattern_mut(0).len = 16;
    for s in 0..16 {
        engine.pattern_mut(0).set_probability(s, p);
        engine.pattern_mut(0).steps[s].note = 36.0;
    }
    let log = spy_on(&mut engine, 0);
    run(&mut engine, n_steps * STEP, 512);
    log.lock().unwrap().len()
}

#[test]
fn probability_extremes_are_deterministic() {
    assert_eq!(count_at_probability(1.0, 256), 256, "p=1 always fires");
    assert_eq!(count_at_probability(0.0, 256), 0, "p=0 never fires");
}

#[test]
fn probability_half_thins_statistically() {
    let fired = count_at_probability(0.5, 1024);
    let frac = fired as f64 / 1024.0;
    assert!(
        (0.40..=0.60).contains(&frac),
        "p=0.5 should fire ~half, got {fired}/1024 = {frac:.3}"
    );
}

// ── Retrig ────────────────────────────────────────────────────────────────────

#[test]
fn retrig_even_is_sample_accurate_across_blocks() {
    let mut engine = Engine::new(SR, 512);
    engine.pattern_mut(0).len = 16;
    engine.pattern_mut(0).set_retrig(
        0,
        Retrig {
            n: 4,
            m: 2,
            curve: RetrigCurve::Even,
            vel_end: 1.0,
        },
    );
    let log = spy_on(&mut engine, 0);

    // Render exactly the 2-step window (12000 frames) — only the first retrig.
    run(&mut engine, 2 * STEP, 512);

    // 4 hits evenly over 2 steps (span 12000) → 0, 3000, 6000, 9000.
    assert_eq!(positions(&log), vec![0, 3_000, 6_000, 9_000]);
}

#[test]
fn retrig_velocity_ramps_linearly() {
    let mut engine = Engine::new(SR, 512);
    engine.pattern_mut(0).len = 16;
    engine.pattern_mut(0).steps[0].velocity = 1.0;
    engine.pattern_mut(0).set_retrig(
        0,
        Retrig {
            n: 4,
            m: 2,
            curve: RetrigCurve::Even,
            vel_end: 0.25,
        },
    );
    let log = spy_on(&mut engine, 0);
    run(&mut engine, 2 * STEP, 512);

    let vels: Vec<f32> = log.lock().unwrap().iter().map(|&(_, v)| v).collect();
    assert_eq!(vels.len(), 4);
    for (got, want) in vels.iter().zip([1.0, 0.75, 0.5, 0.25]) {
        assert!((got - want).abs() < 1e-4, "vel ramp: got {got}, want {want}");
    }
}

#[test]
fn retrig_accel_gaps_shrink() {
    let mut engine = Engine::new(SR, 512);
    engine.pattern_mut(0).len = 16;
    engine.pattern_mut(0).set_retrig(
        0,
        Retrig {
            n: 4,
            m: 2,
            curve: RetrigCurve::Accel,
            vel_end: 1.0,
        },
    );
    let log = spy_on(&mut engine, 0);
    run(&mut engine, 2 * STEP, 512);

    let p = positions(&log);
    assert_eq!(p.len(), 4);
    assert_eq!(p[0], 0, "first hit at window start");
    let gaps: Vec<i64> = p.windows(2).map(|w| w[1] as i64 - w[0] as i64).collect();
    assert!(
        gaps.windows(2).all(|g| g[1] < g[0]),
        "accel → strictly shrinking gaps, got {gaps:?}"
    );
}

// ── Transport jump ────────────────────────────────────────────────────────────

#[test]
fn transport_jump_resyncs_the_lane() {
    let mut engine = Engine::new(SR, 512);
    engine.pattern_mut(0).len = 16;
    engine.pattern_mut(0).set(0, 36.0, 1.0); // plain trig on step 0
    let log = spy_on(&mut engine, 0);

    let mut l = vec![0.0_f32; 512];
    let mut r = vec![0.0_f32; 512];

    // Block at beat 0 → step 0 fires.
    engine.set_transport(Transport {
        playing: true,
        tempo_bpm: BPM,
        song_pos_beats: Some(0.0),
    });
    engine.process_block(&mut l, &mut r);

    // Host jumps to beat 4.0 (next bar boundary) → lane resyncs and step 0
    // (index 16) fires at the jumped block's start.
    engine.set_transport(Transport {
        playing: true,
        tempo_bpm: BPM,
        song_pos_beats: Some(4.0),
    });
    engine.process_block(&mut l, &mut r);

    let p = positions(&log);
    assert_eq!(p.len(), 2, "jump re-fires the step");
    assert_eq!(p[1], 512, "second fire at the jumped block's start");
}

// ── Allocation-free with the full pattern engine ──────────────────────────────

#[test]
fn process_block_alloc_free_with_probability_and_retrig() {
    let mut engine = Engine::new(SR, 512);
    for t in 0..vxn3_engine::N_TRACKS {
        engine.pattern_mut(t).len = 8 + t; // polymeter (≤ MAX_STEPS)
        engine.pattern_mut(t).set_probability(0, 0.7);
        engine.pattern_mut(t).set_retrig(
            4,
            Retrig {
                n: 6,
                m: 3,
                curve: RetrigCurve::Decel,
                vel_end: 0.3,
            },
        );
    }
    let bps = BPM / 60.0 / SR as f64;
    let mut l = vec![0.0_f32; 512];
    let mut r = vec![0.0_f32; 512];
    engine.set_transport(Transport {
        playing: true,
        tempo_bpm: BPM,
        song_pos_beats: Some(0.0),
    });
    engine.process_block(&mut l, &mut r); // prime

    let allocs = alloc_trap::count_allocs(|| {
        for b in 1..300 {
            engine.set_transport(Transport {
                playing: true,
                tempo_bpm: BPM,
                song_pos_beats: Some((b * 512) as f64 * bps),
            });
            engine.process_block(&mut l, &mut r);
        }
    });
    assert_eq!(allocs, 0, "pattern scheduling allocated on the audio path");
}
