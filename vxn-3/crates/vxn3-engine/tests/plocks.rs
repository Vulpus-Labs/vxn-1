//! End-to-end p-lock tests (0050): a lock resolved through the engine reaches
//! the audio (gain latch silences the mix), and lock resolution stays
//! allocation-free on the audio thread. Per-step revert/latch/preemption
//! semantics are covered precisely by the lane resolver unit tests.

use vxn3_engine::engine::Engine;
use vxn3_engine::io::EngineCommand;
use vxn3_engine::sequencer::{Lock, LockParam, Termination};
use vxn3_engine::transport::Transport;

const SR: f32 = 48_000.0;
const BPM: f64 = 120.0;

mod alloc_trap {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    thread_local! {
        static ARMED: Cell<bool> = const { Cell::new(false) };
        static COUNT: Cell<usize> = const { Cell::new(0) };
    }
    struct A;
    // SAFETY: forwards to System; TLS counter bump only.
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

fn kick_every_step(engine: &mut Engine) {
    for s in 0..16 {
        engine.pattern_mut(0).set(s, 28.0, 1.0);
    }
}

fn run(engine: &mut Engine, total: usize, block: usize) -> f32 {
    let bps = BPM / 60.0 / SR as f64;
    let mut sum_sq = 0.0_f64;
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
        for &x in &l {
            sum_sq += (x as f64) * (x as f64);
        }
        p += n;
    }
    (sum_sq / total as f64).sqrt() as f32
}

#[test]
fn gain_latch_silences_the_mix() {
    // Baseline: kick on every step is audible.
    let mut base = Engine::new(SR, 512);
    kick_every_step(&mut base);
    let baseline = run(&mut base, 96_000, 512);
    assert!(baseline > 0.02, "baseline audible, rms={baseline}");

    // A latched gain=0 lock on step 0 holds the track silent for the whole loop.
    let mut locked = Engine::new(SR, 512);
    kick_every_step(&mut locked);
    locked.pattern_mut(0).set_lock(
        0,
        LockParam::Gain,
        Lock {
            value: 0.0,
            termination: Termination::Latch,
        },
    );
    let silenced = run(&mut locked, 96_000, 512);
    assert!(silenced < 1e-4, "gain latch 0 silences, rms={silenced}");
}

#[test]
fn lock_resolution_is_allocation_free() {
    let mut engine = Engine::new(SR, 512);
    kick_every_step(&mut engine);
    // A revert spike on every other step + a latched pan.
    for s in (0..16).step_by(2) {
        engine.pattern_mut(0).set_lock(
            s,
            LockParam::Decay,
            Lock { value: 0.9, termination: Termination::Revert { n: 1 } },
        );
    }
    let io = engine.io(); // clone the handle once (outside the counted region)
    let bps = BPM / 60.0 / SR as f64;
    let mut l = vec![0.0_f32; 512];
    let mut r = vec![0.0_f32; 512];
    engine.set_transport(Transport { playing: true, tempo_bpm: BPM, song_pos_beats: Some(0.0) });
    engine.process_block(&mut l, &mut r); // prime (first knob cook)

    let allocs = alloc_trap::count_allocs(|| {
        for b in 1..300 {
            // Also push live lock edits through the queue.
            io.edits.push(EngineCommand::SetLock {
                track: 0,
                step: (b % 16) as u8,
                param: LockParam::Tone,
                lock: Lock { value: 0.5, termination: Termination::Latch },
            });
            engine.set_transport(Transport {
                playing: true,
                tempo_bpm: BPM,
                song_pos_beats: Some((b * 512) as f64 * bps),
            });
            engine.process_block(&mut l, &mut r);
        }
    });
    assert_eq!(allocs, 0, "p-lock resolution allocated on the audio path");
}
