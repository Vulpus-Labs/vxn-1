//! Integration tests for the 0052 main↔audio I/O: UI edit commands mutate the
//! engine, engine selection swaps via the shared mailbox, the playhead reflects
//! each lane's position, and draining stays allocation-free.

use vxn3_engine::engine::Engine;
use vxn3_engine::engines::make;
use vxn3_engine::flavour::colour_override;
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

/// The marker gestures end to end (0354), over the queue and into a *running*
/// engine: a drag carries the hits in the slots either side, an insert leaves every
/// one of them at the time it was firing at, and a swing sweep keeps the welded ones
/// on their markers.
#[test]
fn marker_gestures_reshape_a_running_lane() {
    let mut engine = Engine::new(SR, 512);
    let io = engine.io();

    // Welded to beat marker 1, and half way through the third 16th after it.
    for (beat, sub, f) in [(1_u16, 0_u8, 0.0_f32), (1, 2, 0.5)] {
        assert!(io.edits.push(EngineCommand::AddHit {
            track: 0,
            beat,
            sub,
            f,
            nudge: 0,
            y: 0.5,
            note: 36.0,
            velocity: 1.0,
        }));
    }
    let _ = play_block(&mut engine, 0.0, 64);
    assert_eq!(engine.track_mut(0).pattern.fire_beat(0), 1.0);
    assert_eq!(engine.track_mut(0).pattern.fire_beat(1), 1.5 + 0.5 * 0.25);

    // Drag marker 1 late. Both hits hang off slots the drag reshaped, so both move —
    // the two-sidedness the strip highlights — and the welded one lands exactly on
    // the marker, because a drag writes no hit record at all.
    assert!(io.edits.push(EngineCommand::DragBeatMarker { track: 0, marker: 1, pos: 1.5 }));
    let _ = play_block(&mut engine, 0.0, 64);
    {
        let p = &engine.track_mut(0).pattern;
        assert_eq!(p.grid().beat_marker(1), 1.5);
        assert_eq!(p.fire_beat(0), 1.5, "welded stays welded");
        assert!(p.fire_beat(1) > 1.5 + 0.5 * 0.25, "the hit in the next slot moved too");
    }

    // A drag past a neighbour clamps rather than crossing: the editor asks, the grid
    // decides, and no gesture can produce a slot narrower than MIN_SLOT.
    assert!(io.edits.push(EngineCommand::DragBeatMarker { track: 0, marker: 1, pos: 99.0 }));
    let _ = play_block(&mut engine, 0.0, 64);
    assert_eq!(
        engine.track_mut(0).pattern.grid().beat_marker(1),
        2.0 - vxn3_engine::MIN_SLOT
    );
    assert!(io.edits.push(EngineCommand::DragBeatMarker { track: 0, marker: 1, pos: 1.5 }));

    // Insert and delete are the opposite rule: the fire times survive the split and
    // the merge, which is what "the hits visibly stay put" means.
    let _ = play_block(&mut engine, 0.0, 64);
    let before = [
        engine.track_mut(0).pattern.fire_beat(0),
        engine.track_mut(0).pattern.fire_beat(1),
    ];
    assert!(io.edits.push(EngineCommand::InsertBeatMarker { track: 0, marker: 3, pos: 2.5, subs: 0 }));
    let _ = play_block(&mut engine, 0.0, 64);
    {
        let p = &engine.track_mut(0).pattern;
        assert_eq!(p.grid().n_beats(), 5);
        assert_eq!([p.fire_beat(0), p.fire_beat(1)], before, "an insert moves nothing");
    }
    assert!(io.edits.push(EngineCommand::DeleteBeatMarker { track: 0, marker: 3 }));
    let _ = play_block(&mut engine, 0.0, 64);
    {
        let p = &engine.track_mut(0).pattern;
        assert_eq!(p.grid().n_beats(), 4);
        assert_eq!([p.fire_beat(0), p.fire_beat(1)], before, "a delete moves nothing");
    }

    // Swing, and the tuplet override beside it. The welded hit rides its marker
    // through the whole sweep — the demo the storage model exists for.
    for amount in [0.25_f64, 0.6, 1.0, -0.5, 0.0] {
        assert!(io.edits.push(EngineCommand::SetSwing {
            track: 0,
            swing: vxn3_engine::Swing::mpc(amount),
        }));
        let _ = play_block(&mut engine, 0.0, 64);
        let p = &engine.track_mut(0).pattern;
        let h = p.hits()[0];
        assert_eq!(
            p.fire_beat(0),
            p.grid().sub_pos(h.beat as usize, h.sub as u32),
            "the welded hit came off its marker at swing {amount}"
        );
    }
    assert!(io.edits.push(EngineCommand::SetBeatSubs { track: 0, beat: 2, subs: 3 }));
    let _ = play_block(&mut engine, 0.0, 64);
    assert_eq!(engine.track_mut(0).pattern.grid().subs(2), 3);
    assert_eq!(engine.track_mut(0).pattern.grid().subs(0), 4, "only the beat named");
}

/// The marker verbs cross to the audio thread like every other delta, so applying one
/// must not allocate — the insert and delete paths re-derive every hit's position,
/// which is exactly the kind of code that reaches for a `Vec` if it is allowed to.
#[test]
fn marker_gesture_drain_is_allocation_free() {
    let mut engine = Engine::new(SR, 512);
    let io = engine.io();
    for i in 0..8 {
        io.edits.push(EngineCommand::AddHit {
            track: 0,
            beat: i % 4,
            sub: (i % 4) as u8,
            f: 0.25,
            nudge: 7,
            y: 0.5,
            note: 36.0,
            velocity: 1.0,
        });
    }
    let bps = BPM / 60.0 / SR as f64;
    let mut l = vec![0.0_f32; 512];
    let mut r = vec![0.0_f32; 512];
    engine.set_transport(Transport { playing: true, tempo_bpm: BPM, song_pos_beats: Some(0.0) });
    engine.process_block(&mut l, &mut r); // prime

    let allocs = alloc_trap::count_allocs(|| {
        for b in 1..200 {
            let pos = 1.0 + 0.5 * ((b % 3) as f64);
            io.edits.push(EngineCommand::DragBeatMarker { track: 0, marker: 1, pos });
            io.edits.push(EngineCommand::InsertBeatMarker { track: 0, marker: 4, pos: 3.5, subs: 0 });
            io.edits.push(EngineCommand::DeleteBeatMarker { track: 0, marker: 4 });
            io.edits.push(EngineCommand::SetSwing {
                track: 0,
                swing: vxn3_engine::Swing::mpc((b % 5) as f64 / 5.0),
            });
            io.edits.push(EngineCommand::SetBeatSubs { track: 0, beat: 2, subs: (b % 6) as u8 });
            engine.set_transport(Transport {
                playing: true,
                tempo_bpm: BPM,
                song_pos_beats: Some((b * 512) as f64 * bps),
            });
            engine.process_block(&mut l, &mut r);
        }
    });
    assert_eq!(allocs, 0, "a marker gesture allocated on the audio thread");
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

/// AC (0355): the palette's edits cross to the audio thread like any other lane
/// edit, and the channels that arrive are the ones the arcs were dragged to — the
/// editor's luminance floor is a display rule and has no representation here.
///
/// Two hits in one slot again, because the palette is hit-keyed: the arcs edit the
/// diamond that was shift-clicked, not whatever else shares its subdivision.
#[test]
fn the_colour_verbs_cross_to_the_audio_thread_unaltered() {
    let mut engine = Engine::new(SR, 512);
    let io = engine.io();
    for f in [0.0_f32, 0.5] {
        assert!(io.edits.push(EngineCommand::AddHit {
            track: 0, beat: 0, sub: 0, f, nudge: 0, y: 0.5, note: 36.0, velocity: 1.0,
        }));
    }
    // Black on one, a tuned triple on the other. Black is the interesting one: it is
    // an invisible diamond the editor has to floor to draw, and a macro vector that
    // sends zero to all three slots.
    assert!(io.edits.push(EngineCommand::SetHitColour {
        track: 0,
        hit: 0,
        rgb: [0.0, 0.0, 0.0],
    }));
    assert!(io.edits.push(EngineCommand::SetHitColour {
        track: 0,
        hit: 1,
        rgb: [1.0, 0.0, 0.5],
    }));
    let _ = play_block(&mut engine, 0.0, 64);
    {
        let p = &engine.track_mut(0).pattern;
        assert_eq!(p.hits()[0].rgb, [0.0, 0.0, 0.0]);
        assert_eq!(colour_override(p.hits()[0].rgb), Some([0.0; 3]), "black sends zero");
        assert_eq!(colour_override(p.hits()[1].rgb), Some([1.0, 0.0, 0.5]));
    }

    // Clearing is its own verb, and lands somewhere else entirely: the slots fall
    // back to the p-lock/base rather than being driven to zero.
    assert!(io.edits.push(EngineCommand::ClearHitColour { track: 0, hit: 0 }));
    let _ = play_block(&mut engine, 0.0, 64);
    let p = &engine.track_mut(0).pattern;
    assert_eq!(colour_override(p.hits()[0].rgb), None);
    assert_eq!(colour_override(p.hits()[1].rgb), Some([1.0, 0.0, 0.5]));
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

/// AC (0366): a lane edit reaches the model and the engine's copy, and the two
/// hold the same list — asserted over a lane whose hits are **not** in insertion
/// order, since the index every later edit uses is a fire-order position.
///
/// This is the defect in miniature. The editor is a view over the model; before
/// 0366 the model did not exist, the engine's copy was the only one, and a page
/// rebuilt over it started from an empty list — so its "hit 1" named nothing and
/// the first drag moved someone else's diamond.
#[test]
fn the_model_and_the_engines_copy_hold_the_same_lane() {
    let mut engine = Engine::new(SR, 512);
    let io = engine.io();

    // Placed back to front, so insertion order and fire order disagree.
    for beat in [3_u16, 0, 2, 1] {
        let cmd = EngineCommand::AddHit {
            track: 0,
            beat,
            sub: 0,
            f: 0.0,
            nudge: 0,
            y: 0.5,
            note: 36.0 + beat as f32,
            velocity: 1.0,
        };
        // What the controller does: queue it, then advance the model.
        assert!(io.edits.push(cmd));
        assert!(io.patterns.apply(cmd));
    }
    let _ = play_block(&mut engine, 0.0, 64);

    let model = io.patterns.get(0);
    assert_eq!(model.hits(), engine.track_mut(0).pattern.hits(), "one lane, two copies");
    let beats: Vec<u16> = model.hits().iter().map(|h| h.beat).collect();
    assert_eq!(beats, vec![0, 1, 2, 3], "fire order, not insertion order");

    // The editor is built from the model, so it points at index 1 for the hit at
    // beat 1 — and the engine moves that hit and no other.
    let drag = EngineCommand::SetHitY { track: 0, hit: 1, y: 0.9 };
    assert!(io.edits.push(drag));
    assert!(io.patterns.apply(drag));
    let _ = play_block(&mut engine, 0.0, 64);

    let p = &engine.track_mut(0).pattern;
    assert_eq!(p.hits()[1].beat, 1);
    assert_eq!(p.hits()[1].y, 0.9);
    for i in [0_usize, 2, 3] {
        assert_eq!(p.hits()[i].y, 0.5, "hit {i} must not have moved");
    }
    assert_eq!(io.patterns.get(0).hits(), p.hits(), "still in step after the drag");
}

/// AC (0366): nothing clears a lane to reach agreement. A freshly built engine
/// **seeds** its copy from the model, so a plugin reactivated over a live model
/// starts holding it rather than starting empty and discarding it.
#[test]
fn a_new_engine_seeds_its_lanes_from_the_model() {
    let io = vxn3_engine::io::EngineIo::new();
    let mut p = vxn3_engine::Pattern::default();
    p.insert(vxn3_engine::Hit::at(2, 1));
    p.insert(vxn3_engine::Hit::at(0, 0));
    p.set_grid_beats(3);
    io.patterns.set(4, p);

    let mut engine = Engine::with_io(SR, 512, io.clone());
    assert_eq!(engine.track_mut(4).pattern.hits(), io.patterns.get(4).hits());
    assert_eq!(engine.track_mut(4).pattern.grid(), io.patterns.get(4).grid());
    assert_eq!(engine.track_mut(4).pattern.len(), 2);
    // …and the model is untouched by having been read.
    assert_eq!(io.patterns.get(4).len(), 2);
}

/// AC (0366): the **full flush** — the whole model down to a *running* engine, for
/// when the model was replaced rather than edited. The marker rides the edit queue
/// so the replacement lands at its own place among the deltas around it.
#[test]
fn a_full_flush_replaces_a_running_engines_lane() {
    let mut engine = Engine::new(SR, 512);
    let io = engine.io();

    // The engine is running with a lane the user built.
    let add = EngineCommand::AddHit {
        track: 1, beat: 0, sub: 0, f: 0.0, nudge: 0, y: 0.5, note: 36.0, velocity: 1.0,
    };
    assert!(io.edits.push(add));
    assert!(io.patterns.apply(add));
    let _ = play_block(&mut engine, 0.0, 64);
    assert_eq!(engine.track_mut(1).pattern.len(), 1);

    // A restore replaces the model wholesale and flushes it down.
    let mut restored = vxn3_engine::Pattern::default();
    restored.set_grid_beats(2);
    for (b, s) in [(1_u16, 2_u8), (0, 1), (1, 0)] {
        restored.insert(vxn3_engine::Hit::at(b, s));
    }
    io.patterns.set(1, restored);
    assert!(io.flush_lane(1));

    let _ = play_block(&mut engine, 0.0, 64);
    let live = &engine.track_mut(1).pattern;
    assert_eq!(live.len(), 3, "the replacement landed whole");
    assert_eq!(live.grid().n_beats(), 2, "geometry travels with it");
    assert_eq!(live.hits(), io.patterns.get(1).hits(), "and matches the model");
}

/// A flush and the deltas around it have **one** order. The marker travels in the
/// queue precisely so an edit sent after a flush is applied after it, rather than
/// the two racing on separate channels.
#[test]
fn a_flush_orders_against_the_edits_around_it() {
    let mut engine = Engine::new(SR, 512);
    let io = engine.io();

    let mut restored = vxn3_engine::Pattern::default();
    restored.insert(vxn3_engine::Hit::at(0, 0));
    io.patterns.set(2, restored);
    assert!(io.flush_lane(2));

    // …then an edit, which must land on top of the flushed lane, not under it.
    let after = EngineCommand::AddHit {
        track: 2, beat: 2, sub: 0, f: 0.0, nudge: 0, y: 0.5, note: 36.0, velocity: 1.0,
    };
    assert!(io.edits.push(after));
    assert!(io.patterns.apply(after));

    let _ = play_block(&mut engine, 0.0, 64);
    let live = &engine.track_mut(2).pattern;
    assert_eq!(live.len(), 2, "flush then edit, in that order");
    assert_eq!(live.hits(), io.patterns.get(2).hits());
}

/// AC (0366): the readback costs the audio thread nothing, because there is no
/// readback on the audio thread — the model is main-side. What the audio thread
/// does carry is the flush, and applying one must not allocate either.
#[test]
fn flushing_every_lane_is_allocation_free() {
    let mut engine = Engine::new(SR, 512);
    let io = engine.io();
    for t in 0..vxn3_engine::N_TRACKS {
        let mut p = vxn3_engine::Pattern::default();
        p.insert(vxn3_engine::Hit::at(0, 1));
        io.patterns.set(t, p);
    }
    let bps = BPM / 60.0 / SR as f64;
    let mut l = vec![0.0_f32; 512];
    let mut r = vec![0.0_f32; 512];
    engine.set_transport(Transport { playing: true, tempo_bpm: BPM, song_pos_beats: Some(0.0) });
    engine.process_block(&mut l, &mut r); // prime

    let allocs = alloc_trap::count_allocs(|| {
        for b in 1..200 {
            // The ring holds FLUSH_CAP - 1 in flight; whatever gets through is
            // drained by the block below, so the next iteration has room again.
            io.flush_all();
            engine.set_transport(Transport {
                playing: true,
                tempo_bpm: BPM,
                song_pos_beats: Some((b * 512) as f64 * bps),
            });
            engine.process_block(&mut l, &mut r);
        }
    });
    assert_eq!(allocs, 0, "flush install allocated on the audio thread");
    // Every lane the flush reached holds the model's pattern.
    assert_eq!(engine.track_mut(0).pattern.hits(), io.patterns.get(0).hits());
}
