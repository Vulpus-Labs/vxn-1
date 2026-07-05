//! Integration test for the three-engine minimal-techno kit (0049): `Kick/Tone`,
//! `Metal`, and `Noise` all running through the **unchanged** 0047
//! track/dispatch/SoA framework. Proves the `TrackEngine` trait generalises
//! across the poly and resonator voicing models with no poly-specific
//! assumptions, and that the full mix stays allocation-free.

use vxn3_engine::engine::Engine;
use vxn3_engine::engines::make;
use vxn3_engine::track_engine::EngineKind;
use vxn3_engine::transport::Transport;

const SR: f32 = 48_000.0;
const BPM: f64 = 126.0;

mod alloc_trap {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    thread_local! {
        static ARMED: Cell<bool> = const { Cell::new(false) };
        static COUNT: Cell<usize> = const { Cell::new(0) };
    }
    struct A;
    // SAFETY: forwards to System; only a TLS counter bump while armed.
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

/// Build the kit: kick (poly), hats (Metal resonator), snare (Noise poly).
fn build_kit() -> Engine {
    let mut engine = Engine::new(SR, 512);

    // Track 0 — Kick/Tone (default engine): 4-on-the-floor, low note.
    for s in [0, 4, 8, 12] {
        engine.pattern_mut(0).set(s, 28.0, 1.0);
    }

    // Track 1 — Metal: closed hats on the off-8ths; one open hat that the next
    // closed hat chokes. (note 42 = closed, 46 = open — GM map.)
    engine.track_mut(1).engine = make(EngineKind::Metal, SR);
    for s in [2, 6, 10] {
        engine.pattern_mut(1).set(s, 42.0, 0.7);
    }
    engine.pattern_mut(1).set(14, 46.0, 0.8); // open hat (rings into the bar)

    // Track 2 — Noise: snare backbeat.
    engine.track_mut(2).engine = make(EngineKind::Noise, SR);
    for s in [4, 12] {
        engine.pattern_mut(2).set(s, 60.0, 0.9);
    }

    engine
}

fn run(engine: &mut Engine, total: usize, block: usize) -> Vec<f32> {
    let bps = BPM / 60.0 / SR as f64;
    let mut out = Vec::with_capacity(total);
    let mut p = 0;
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

#[test]
fn three_engine_kit_plays_a_loop() {
    let mut engine = build_kit();
    // ~1 bar at 126 BPM ≈ 1.9 s.
    let buf = run(&mut engine, 96_000, 512);
    let rms = (buf.iter().map(|&x| x * x).sum::<f32>() / buf.len() as f32).sqrt();
    assert!(rms > 0.03, "kit loop audible, rms={rms}");
    assert!(buf.iter().all(|x| x.is_finite()), "finite output");
    // Confirm engine kinds: poly + resonator + poly through one framework.
    assert_eq!(engine.track_mut(0).engine.kind(), EngineKind::KickTone);
    assert_eq!(engine.track_mut(1).engine.kind(), EngineKind::Metal);
    assert_eq!(engine.track_mut(2).engine.kind(), EngineKind::Noise);
}

#[test]
fn three_engine_kit_is_allocation_free() {
    let mut engine = build_kit();
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
    assert_eq!(allocs, 0, "three-engine mix allocated on the audio path");
}

/// The flavour runtime (0180/0181/0182) is allocation-free on the audio path: the
/// Driven track's per-trig `resolve` + re-cook, and the Noise clap's multi-tap gate +
/// SVF bandpass + snap, must not allocate — both trig repeatedly across the loop.
#[test]
fn driven_flavour_trig_is_allocation_free() {
    use vxn3_engine::flavour::{Binding, Curve, Flavour};

    let mut engine = build_kit();
    // Install a Driven flavour with all three macros bound and drive + click on, so
    // every trig re-resolves and the saturation/click paths are exercised (0181).
    let flav = Flavour {
        base: vec![0.001, 0.35, 24.0, 0.05, 0.4, 0.5], // 6 params incl. drive + click
        bindings: vec![
            Binding { slot: 0, param: 1, curve: Curve::Linear, depth: 0.6 },
            Binding { slot: 1, param: 3, curve: Curve::Exp, depth: 0.1 },
            Binding { slot: 2, param: 2, curve: Curve::Linear, depth: 12.0 },
        ],
        macro_defaults: [0.5; 3],
    };
    engine.track_mut(0).engine.apply_flavour(flav);
    // Track 1 is Metal — a cymbal exercises the XOR oscillators + shimmer LFO + HP (0183).
    engine
        .track_mut(1)
        .engine
        .apply_flavour(vxn3_engine::engines::metal::flavour_cymbal());
    // Track 2 is Noise — give it a 4-tap clap so the burst gate + SVF + snap run (0182).
    engine
        .track_mut(2)
        .engine
        .apply_flavour(vxn3_engine::engines::noise::flavour_clap());

    let bps = BPM / 60.0 / SR as f64;
    let mut l = vec![0.0_f32; 512];
    let mut r = vec![0.0_f32; 512];
    engine.set_transport(Transport { playing: true, tempo_bpm: BPM, song_pos_beats: Some(0.0) });
    engine.process_block(&mut l, &mut r); // prime (installs the resolve)

    let allocs = alloc_trap::count_allocs(|| {
        for b in 1..300 {
            // Nudge a macro every few blocks → forces a re-resolve at the next trig.
            if b % 8 == 0 {
                engine.track_mut(0).engine.set_macro(0, (b as f32 * 0.01) % 1.0);
            }
            engine.set_transport(Transport {
                playing: true,
                tempo_bpm: BPM,
                song_pos_beats: Some((b * 512) as f64 * bps),
            });
            engine.process_block(&mut l, &mut r);
        }
    });
    assert_eq!(allocs, 0, "flavour resolve allocated on the audio path");
}
