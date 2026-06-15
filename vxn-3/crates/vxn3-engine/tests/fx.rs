//! Integration tests for the 0051 FX cut: the p-lockable delay send (dub throw),
//! self-oscillation, tempo-synced delay time, the master limiter's hard ceiling +
//! reported latency, and allocation-free processing.

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

/// Render `total` frames, returning the left channel.
fn render(engine: &mut Engine, total: usize, block: usize) -> Vec<f32> {
    let bps = BPM / 60.0 / SR as f64;
    let mut out = Vec::with_capacity(total);
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
        out.extend_from_slice(&l);
        p += n;
    }
    out
}

fn rms(b: &[f32]) -> f32 {
    (b.iter().map(|&x| x * x).sum::<f32>() / b.len().max(1) as f32).sqrt()
}

#[test]
fn send_plock_throws_a_hit_into_the_delay() {
    // One kick on step 0. Send base is 0 → the dry path alone leaves the tail
    // clean. A revert send-lock on step 0 throws *that* hit into the delay.
    let after = 40_000..90_000; // well past the ~0.35 s kick decay

    let mut dry = Engine::new(SR, 512);
    dry.pattern_mut(0).set(0, 28.0, 1.0);
    let dry_buf = render(&mut dry, 96_000, 512);
    let dry_tail = rms(&dry_buf[after.clone()]);

    let mut thrown = Engine::new(SR, 512);
    thrown.pattern_mut(0).set(0, 28.0, 1.0);
    thrown.pattern_mut(0).set_lock(
        0,
        LockParam::Send,
        Lock { value: 1.0, termination: Termination::Revert { n: 1 } },
    );
    let thrown_buf = render(&mut thrown, 96_000, 512);
    let thrown_tail = rms(&thrown_buf[after]);

    assert!(
        thrown_tail > dry_tail * 8.0,
        "send p-lock should throw the hit into the delay tail: dry={dry_tail}, thrown={thrown_tail}"
    );
}

#[test]
fn delay_self_oscillates_past_unity_and_stays_finite() {
    let mut engine = Engine::new(SR, 512);
    engine.pattern_mut(0).set(0, 28.0, 1.0); // single seed hit
    let io = engine.io();
    io.edits.push(EngineCommand::SetSend { track: 0, amount: 1.0 });
    io.edits.push(EngineCommand::SetDelayFeedback { value: 1.15 }); // past unity
    let buf = render(&mut engine, 240_000, 512); // 5 s

    let tail = &buf[200_000..];
    assert!(rms(tail) > 0.005, "still self-oscillating, tail rms={}", rms(tail));
    assert!(
        buf.iter().all(|x| x.is_finite() && x.abs() <= 0.96),
        "bounded by the saturator + limiter"
    );
}

#[test]
fn delay_time_tracks_host_tempo() {
    // A single thrown kick; with feedback 0 there's one echo at the synced time.
    // Find the echo peak position at two tempos — it should scale with tempo.
    fn echo_pos(bpm: f64) -> usize {
        let mut e = Engine::new(SR, 512);
        e.pattern_mut(0).set(0, 28.0, 1.0);
        let io = e.io();
        io.edits.push(EngineCommand::SetSend { track: 0, amount: 1.0 });
        io.edits.push(EngineCommand::SetDelayFeedback { value: 0.0 });
        io.edits.push(EngineCommand::SetDelayReturn { value: 1.0 });
        let bps = bpm / 60.0 / SR as f64;
        let mut out = Vec::new();
        let mut p = 0usize;
        while p < 96_000 {
            e.set_transport(Transport { playing: true, tempo_bpm: bpm, song_pos_beats: Some(p as f64 * bps) });
            let mut l = vec![0.0_f32; 512];
            let mut r = vec![0.0_f32; 512];
            e.process_block(&mut l, &mut r);
            out.extend_from_slice(&l);
            p += 512;
        }
        // Peak after the dry kick (skip the first 5000 samples).
        out[5_000..]
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().partial_cmp(&b.1.abs()).unwrap())
            .map(|(i, _)| i + 5_000)
            .unwrap()
    }
    let at_120 = echo_pos(120.0);
    let at_60 = echo_pos(60.0);
    // 0.75 beat: 18000 samples @120, 36000 @60.
    assert!((at_120 as i64 - 18_000).abs() < 2_000, "120 BPM echo ~18000, got {at_120}");
    assert!((at_60 as i64 - 36_000).abs() < 2_000, "60 BPM echo ~36000, got {at_60}");
}

#[test]
fn master_limiter_prevents_clipping() {
    // Drive all 8 tracks hot.
    let mut engine = Engine::new(SR, 512);
    let io = engine.io();
    for t in 0..vxn3_engine::N_TRACKS as u8 {
        for s in 0..16 {
            engine.pattern_mut(t as usize).set(s, 30.0 + t as f32, 1.0);
        }
        io.edits.push(EngineCommand::SetGain { track: t, gain: 1.5 });
    }
    let buf = render(&mut engine, 96_000, 512);
    let peak = buf.iter().fold(0.0_f32, |m, &x| m.max(x.abs()));
    assert!(peak <= 0.95 + 1e-3, "limiter holds the ceiling, peak={peak}");
    assert!(peak > 0.5, "and the mix is actually loud, peak={peak}");
}

#[test]
fn reports_limiter_latency() {
    let engine = Engine::new(SR, 512);
    assert_eq!(engine.latency_samples(), vxn3_engine::LIMITER_LOOKAHEAD);
    assert_eq!(engine.latency_samples(), 64);
}

#[test]
fn fx_path_is_allocation_free() {
    let mut engine = Engine::new(SR, 512);
    for t in 0..vxn3_engine::N_TRACKS as u8 {
        engine.pattern_mut(t as usize).set(0, 30.0 + t as f32, 1.0);
    }
    let io = engine.io();
    io.edits.push(EngineCommand::SetSend { track: 0, amount: 0.6 });
    io.edits.push(EngineCommand::SetDelayFeedback { value: 0.8 });
    let bps = BPM / 60.0 / SR as f64;
    let mut l = vec![0.0_f32; 512];
    let mut r = vec![0.0_f32; 512];
    engine.set_transport(Transport { playing: true, tempo_bpm: BPM, song_pos_beats: Some(0.0) });
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
    assert_eq!(allocs, 0, "delay + limiter + send allocated on the audio path");
}
