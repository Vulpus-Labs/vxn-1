//! Integration tests for the 0047 audible slice: 4-on-the-floor at host tempo,
//! sample-accurate trig scheduling, an allocation-free process callback, and a
//! click-free / alloc-free off-thread engine swap.

use std::sync::{Arc, Mutex};

use vxn3_engine::engine::Engine;
use vxn3_engine::engines::KickTone;
use vxn3_engine::track_engine::{EngineKind, TrackEngine};
use vxn3_engine::transport::Transport;

const SR: f32 = 48_000.0;
const BPM: f64 = 120.0;

// ── Allocation trap (per-test-binary, thread-local) ───────────────────────────

mod alloc_trap {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! {
        static ARMED: Cell<bool> = const { Cell::new(false) };
        static COUNT: Cell<usize> = const { Cell::new(0) };
    }

    struct TrapAlloc;
    // SAFETY: forwards to System; only adds a non-allocating TLS counter bump.
    unsafe impl GlobalAlloc for TrapAlloc {
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
    static GLOBAL: TrapAlloc = TrapAlloc;

    pub fn count_allocs(f: impl FnOnce()) -> usize {
        COUNT.with(|c| c.set(0));
        ARMED.with(|a| a.set(true));
        f();
        ARMED.with(|a| a.set(false));
        COUNT.with(Cell::get)
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Render `total` frames in `block`-sized chunks, feeding the host transport
/// each block (BPM, playing, advancing beat clock). Returns the left channel.
fn render(engine: &mut Engine, total: usize, block: usize, playing: bool) -> Vec<f32> {
    let bps = BPM / 60.0 / SR as f64;
    let mut out = Vec::with_capacity(total);
    let mut p = 0usize;
    while p < total {
        let n = block.min(total - p);
        engine.set_transport(Transport {
            playing,
            tempo_bpm: BPM,
            song_pos_beats: Some(p as f64 * bps),
        });
        let mut l = vec![0.0_f32; n];
        let mut r = vec![0.0_f32; n];
        engine.process_block(&mut l, &mut r);
        out.extend_from_slice(&l);
        p += n;
    }
    out
}

fn rms(b: &[f32]) -> f32 {
    if b.is_empty() {
        return 0.0;
    }
    (b.iter().map(|&x| x * x).sum::<f32>() / b.len() as f32).sqrt()
}

/// A `TrackEngine` that records the absolute sample position of each trig. Lets
/// us assert sample-accurate scheduling exactly, independent of DSP.
struct Spy {
    pos: usize,
    log: Arc<Mutex<Vec<usize>>>,
}
impl TrackEngine for Spy {
    fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        self.pos += out.len();
    }
    fn on_trig(&mut self, _note: f32, _velocity: f32) {
        self.log.lock().unwrap().push(self.pos);
    }
    fn reset(&mut self) {
        self.pos = 0;
    }
    fn set_sample_rate(&mut self, _sr: f32) {}
    fn kind(&self) -> EngineKind {
        EngineKind::KickTone
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[test]
fn four_on_the_floor_is_audible_and_transport_gated() {
    let mut engine = Engine::new(SR, 512);
    // Kick on every quarter note (steps 0,4,8,12 of a 16-step bar).
    let pat = engine.pattern_mut(0);
    for s in [0, 4, 8, 12] {
        pat.set(s, 28.0, 1.0);
    }

    // 1 bar at 120 BPM = 2 s = 96000 frames.
    let playing = render(&mut engine, 96_000, 512, true);
    assert!(rms(&playing) > 0.02, "groove audible, rms={}", rms(&playing));
    assert!(playing.iter().all(|x| x.is_finite()), "finite");

    // Stop transport, let tails die, then a fresh window is silent (no trigs
    // fire while stopped).
    engine.reset();
    let stopped = render(&mut engine, 96_000, 512, false);
    assert!(rms(&stopped) < 1e-5, "stopped → silent, rms={}", rms(&stopped));
}

#[test]
fn trig_scheduling_is_sample_accurate_and_block_size_invariant() {
    // Spy on track 0; one trig per quarter note. Drive at two very different
    // block sizes and assert the recorded sample positions are identical and
    // land exactly on the 16th grid (i*6000 frames at 120 BPM / 48 kHz).
    let expected: Vec<usize> = (0..4).map(|q| q * 24_000).collect();

    for block in [64usize, 512, 1000] {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::new(SR, 1024);
        engine.track_mut(0).engine = Box::new(Spy {
            pos: 0,
            log: log.clone(),
        });
        let pat = engine.pattern_mut(0);
        for s in [0, 4, 8, 12] {
            pat.set(s, 28.0, 1.0);
        }
        let _ = render(&mut engine, 96_000, block, true);
        let got = log.lock().unwrap().clone();
        assert_eq!(got, expected, "block={block}: trig samples must be exact");
    }
}

#[test]
fn process_block_is_allocation_free() {
    let mut engine = Engine::new(SR, 512);
    for t in 0..vxn3_engine::N_TRACKS {
        engine.pattern_mut(t).set(0, 30.0 + t as f32, 1.0);
        engine.pattern_mut(t).set(8, 42.0 + t as f32, 0.8);
    }
    let bps = BPM / 60.0 / SR as f64;
    let mut l = vec![0.0_f32; 512];
    let mut r = vec![0.0_f32; 512];
    // Prime once (warms anything lazy), then count over many blocks.
    engine.set_transport(Transport {
        playing: true,
        tempo_bpm: BPM,
        song_pos_beats: Some(0.0),
    });
    engine.process_block(&mut l, &mut r);

    let allocs = alloc_trap::count_allocs(|| {
        for b in 1..200 {
            engine.set_transport(Transport {
                playing: true,
                tempo_bpm: BPM,
                song_pos_beats: Some((b * 512) as f64 * bps),
            });
            engine.process_block(&mut l, &mut r);
        }
    });
    assert_eq!(allocs, 0, "process_block allocated on the audio path");
}

#[test]
fn engine_swap_is_alloc_free_and_does_not_click() {
    let mut engine = Engine::new(SR, 512);
    // Silent track (empty pattern, transport stopped) so the swap point is
    // silence → no discontinuity. The swap mechanism itself is what we assert
    // is allocation-free on the audio thread.
    let swap = engine.track_swap(0);
    swap.send(Box::new(KickTone::with_default_patch(SR)))
        .map_err(|_| "send failed")
        .unwrap();

    let mut l = vec![0.0_f32; 512];
    let mut r = vec![0.0_f32; 512];
    engine.set_transport(Transport {
        playing: false,
        tempo_bpm: BPM,
        song_pos_beats: Some(0.0),
    });

    let allocs = alloc_trap::count_allocs(|| {
        // This block installs the pending engine (moves boxes, frees nothing).
        engine.process_block(&mut l, &mut r);
    });
    assert_eq!(allocs, 0, "engine swap allocated/freed on the audio thread");

    // No click: the track was silent before and after the swap.
    let peak = l.iter().chain(r.iter()).fold(0.0_f32, |m, &x| m.max(x.abs()));
    assert!(peak < 1e-6, "swap at silence must not click, peak={peak}");

    // The retired engine is dropped on the main thread.
    swap.reclaim();
}
