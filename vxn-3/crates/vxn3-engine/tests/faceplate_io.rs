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

    // Program slot 0 on track 0 from the "UI"; it fires at the next pass of the
    // lane (beat 4.0).
    assert!(io.edits.push(EngineCommand::SetHit {
        track: 0,
        slot: 0,
        note: 28.0,
        velocity: 1.0,
    }));
    let (l, _) = play_block(&mut engine, 4.0, 512);
    assert!(rms(&l) > 0.01, "programmed trig audible, rms={}", rms(&l));
}

/// The lane strip's vocabulary end to end (0353): place a hit off the grid, drag
/// it across a beat marker, quantise it back onto a marker, then delete it — all
/// through the edit queue, all keyed by fire-order index.
#[test]
fn free_position_commands_place_move_and_quantise_a_hit() {
    let mut engine = Engine::new(SR, 512);
    let io = engine.io();

    // Half way through the second 16th of beat 0 → 0.25 + 0.5 · 0.25 = 0.375.
    assert!(io.edits.push(EngineCommand::AddHit {
        track: 0,
        beat: 0,
        sub: 1,
        f: 0.5,
        nudge: 0,
        y: 0.75,
        note: 36.0,
        velocity: 1.0,
    }));
    let _ = play_block(&mut engine, 0.0, 64);
    {
        let p = &engine.track_mut(0).pattern;
        assert_eq!(p.len(), 1);
        assert_eq!(p.fire_beat(0), 0.375);
        assert_eq!(p.hits()[0].y, 0.75);
    }

    // Drag it into beat 2 — the stored (beat, sub) follows, and the resolved time
    // is what the drag asked for rather than where the old slot was.
    assert!(io.edits.push(EngineCommand::SetHitPosition {
        track: 0,
        hit: 0,
        beat: 2,
        sub: 2,
        f: 0.25,
        nudge: 0,
    }));
    assert!(io.edits.push(EngineCommand::SetHitY { track: 0, hit: 0, y: 0.2 }));
    let _ = play_block(&mut engine, 0.0, 64);
    {
        let p = &engine.track_mut(0).pattern;
        assert_eq!((p.hits()[0].beat, p.hits()[0].sub), (2, 2));
        assert_eq!(p.fire_beat(0), 2.5 + 0.25 * 0.25);
        assert_eq!(p.hits()[0].y, 0.2);
    }

    // Quantise-X welds it to its marker; quantise-Y pulls it to the centre curve.
    assert!(io.edits.push(EngineCommand::QuantiseHitX { track: 0, hit: 0, amount: 1.0 }));
    assert!(io.edits.push(EngineCommand::QuantiseHitY { track: 0, hit: 0, amount: 1.0 }));
    let _ = play_block(&mut engine, 0.0, 64);
    {
        let p = &engine.track_mut(0).pattern;
        assert_eq!(p.hits()[0].f, 0.0);
        assert_eq!(p.fire_beat(0), 2.5);
        assert_eq!(p.hits()[0].y, 0.5);
    }

    assert!(io.edits.push(EngineCommand::RemoveHit { track: 0, hit: 0 }));
    let _ = play_block(&mut engine, 0.0, 64);
    assert!(engine.track_mut(0).pattern.is_empty());
}

/// The hit-keyed attribute verbs reach a hit a slot-keyed one cannot: two hits in
/// one subdivision slot, only the second edited.
#[test]
fn hit_keyed_attributes_address_one_of_several_hits_in_a_slot() {
    let mut engine = Engine::new(SR, 512);
    let io = engine.io();
    for f in [0.0_f32, 0.5] {
        assert!(io.edits.push(EngineCommand::AddHit {
            track: 1,
            beat: 0,
            sub: 0,
            f,
            nudge: 0,
            y: 0.5,
            note: 36.0,
            velocity: 1.0,
        }));
    }
    assert!(io.edits.push(EngineCommand::SetHitProbability {
        track: 1,
        hit: 1,
        probability: 0.25,
    }));
    assert!(io.edits.push(EngineCommand::SetHitNote {
        track: 1,
        hit: 1,
        note: 50.0,
        velocity: 0.4,
    }));
    let _ = play_block(&mut engine, 0.0, 64);
    let p = &engine.track_mut(1).pattern;
    assert_eq!(p.len(), 2);
    assert_eq!((p.hits()[0].probability, p.hits()[0].note), (1.0, 36.0));
    assert_eq!((p.hits()[1].probability, p.hits()[1].note), (0.25, 50.0));
    assert_eq!(p.hits()[1].velocity, 0.4);
}

/// An over-capacity add drops rather than allocating — the audio thread's half of
/// the MAX_HITS ceiling the editor shows the user.
#[test]
fn adding_past_the_hit_ceiling_drops_rather_than_growing() {
    let mut engine = Engine::new(SR, 512);
    for i in 0..(vxn3_engine::MAX_HITS + 8) {
        engine.apply_command(EngineCommand::AddHit {
            track: 0,
            beat: 0,
            sub: 0,
            f: (i as f32 / 128.0).min(0.99),
            nudge: 0,
            y: 0.5,
            note: 36.0,
            velocity: 1.0,
        });
    }
    assert_eq!(engine.track_mut(0).pattern.len(), vxn3_engine::MAX_HITS);
}

#[test]
fn playhead_reflects_each_lanes_position() {
    let mut engine = Engine::new(SR, 512);
    let io = engine.io();
    // Track 1 runs a three-beat lane (12 slots); at beat 3.5 it is on a different
    // slot than the default four-beat lane.
    assert!(io.edits.push(EngineCommand::SetGridBeats { track: 1, beats: 3 }));

    let _ = play_block(&mut engine, 3.5, 64); // 3.5 beats = 14 sixteenths
    assert_eq!(io.playhead.step(0), 14, "track0 (16 slots): 14 % 16 = 14");
    assert_eq!(io.playhead.step(1), 2, "track1 (12 slots): 14 % 12 = 2 (phased)");
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
        io.edits.push(EngineCommand::SetHit { track: t, slot: 0, note: 36.0, velocity: 1.0 });
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
            // The 0353 hit verbs re-place a hit by removing and re-inserting it —
            // in the fixed-capacity array, so this must not allocate either.
            io.edits.push(EngineCommand::SetHitPosition {
                track: (b % 8) as u8,
                hit: 0,
                beat: (b % 4) as u16,
                sub: (b % 4) as u8,
                f: 0.5,
                nudge: 0,
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
