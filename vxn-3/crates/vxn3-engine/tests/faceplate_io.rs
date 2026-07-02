//! Integration tests for the 0052 main↔audio I/O: UI edit commands mutate the
//! engine, engine selection swaps via the shared mailbox, the playhead reflects
//! each lane's position, and draining stays allocation-free.

use vxn3_engine::engine::Engine;
use vxn3_engine::engines::make;
use vxn3_engine::io::{EngineCommand, PlayheadState};
use vxn3_engine::track_engine::EngineKind;
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

fn play_block(engine: &mut Engine, beat0: f64, frames: usize) -> (Vec<f32>, Vec<f32>) {
    engine.set_transport(Transport {
        playing: true,
        tempo_bpm: BPM,
        song_pos_beats: Some(beat0),
    });
    let mut l = vec![0.0_f32; frames];
    let mut r = vec![0.0_f32; frames];
    engine.process_block(&mut l, &mut r);
    (l, r)
}

fn rms(b: &[f32]) -> f32 {
    (b.iter().map(|&x| x * x).sum::<f32>() / b.len().max(1) as f32).sqrt()
}

#[test]
fn edit_command_programs_a_trig() {
    let mut engine = Engine::new(SR, 512);
    let io = engine.io();

    // Empty pattern → silence at beat 0.
    let (l, _) = play_block(&mut engine, 0.0, 512);
    assert!(rms(&l) < 1e-6, "empty pattern silent");

    // Program step 0 on track 0 from the "UI"; it fires at the next step-0
    // boundary (beat 4.0).
    assert!(io.edits.push(EngineCommand::SetStep {
        track: 0,
        step: 0,
        note: 28.0,
        velocity: 1.0,
    }));
    let (l, _) = play_block(&mut engine, 4.0, 512);
    assert!(rms(&l) > 0.01, "programmed trig audible, rms={}", rms(&l));
}

#[test]
fn playhead_reflects_each_lanes_position() {
    let mut engine = Engine::new(SR, 512);
    let io = engine.io();
    // Track 1 runs a 12-step lane; at beat 1.25 it's on a different step than a
    // 16-step lane would be.
    assert!(io.edits.push(EngineCommand::SetLength { track: 1, len: 12 }));

    let _ = play_block(&mut engine, 3.5, 64); // 3.5 beats = 14 sixteenths
    assert_eq!(io.playhead.step(0), 14, "track0 (len16): 14 % 16 = 14");
    assert_eq!(io.playhead.step(1), 2, "track1 (len12): 14 % 12 = 2 (phased)");
    assert!(io.playhead.playing());

    // Stopped → playhead parks.
    engine.set_transport(Transport {
        playing: false,
        tempo_bpm: BPM,
        song_pos_beats: Some(3.5),
    });
    let mut l = vec![0.0; 64];
    let mut r = vec![0.0; 64];
    engine.process_block(&mut l, &mut r);
    assert_eq!(io.playhead.step(0), PlayheadState::STOPPED);
    assert!(!io.playhead.playing());
}

#[test]
fn engine_selection_swaps_via_shared_mailbox() {
    let mut engine = Engine::new(SR, 512);
    let io = engine.io();
    assert_eq!(engine.track_mut(2).engine.kind(), EngineKind::KickTone);

    // "UI" picks Noise for track 2: build on main, hand over via the swap.
    io.swaps[2].send(make(EngineKind::Noise, SR)).map_err(|_| ()).unwrap();
    let _ = play_block(&mut engine, 0.0, 512); // installs the swap
    assert_eq!(engine.track_mut(2).engine.kind(), EngineKind::Noise);
}

#[test]
fn command_drain_is_allocation_free() {
    let mut engine = Engine::new(SR, 512);
    let io = engine.io();
    for t in 0..vxn3_engine::N_TRACKS as u8 {
        io.edits.push(EngineCommand::SetStep { track: t, step: 0, note: 36.0, velocity: 1.0 });
    }
    let bps = BPM / 60.0 / SR as f64;
    let mut l = vec![0.0_f32; 512];
    let mut r = vec![0.0_f32; 512];
    engine.set_transport(Transport { playing: true, tempo_bpm: BPM, song_pos_beats: Some(0.0) });
    engine.process_block(&mut l, &mut r); // prime

    let allocs = alloc_trap::count_allocs(|| {
        for b in 1..200 {
            // A fresh edit every block, drained on the audio thread.
            io.edits.push(EngineCommand::SetMacro {
                track: (b % 8) as u8,
                slot: 0,
                value: 0.5,
            });
            engine.set_transport(Transport {
                playing: true,
                tempo_bpm: BPM,
                song_pos_beats: Some((b * 512) as f64 * bps),
            });
            engine.process_block(&mut l, &mut r);
        }
    });
    assert_eq!(allocs, 0, "command drain / playhead publish allocated");
}
