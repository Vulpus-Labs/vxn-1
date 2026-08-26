//! Profiling harness for a **deep-routing** patch — the case where the matrix
//! evaluator has to do dependent work rather than fan out independent scalars.
//!
//! Patch (layer 1 only; layer 2 silent):
//!   - LFO 2 → LFO 1 rate  (the one-block-lagged dest — chained modulator)
//!   - LFO 1 → cross-mod amount
//!   - cross mod = hard sync
//!   - osc 1 = pulse at +18 st (octave +1, coarse +6)
//!   - Env 1 → PWM
//!
//! Modes let the routing cost be isolated from the DSP it drives:
//!
//!   cargo run --release --example route_profile -p vxn1b-engine -- routed
//!   cargo run --release --example route_profile -p vxn1b-engine -- flat   # same panel, no routes
//!   cargo run --release --example route_profile -p vxn1b-engine -- clean  # no sync either
//!   cargo run --release --example route_profile -p vxn1b-engine -- idle   # no notes
//!
//! Under a sampler:
//!   CARGO_PROFILE_RELEASE_DEBUG=line-tables-only \
//!     cargo build --release --example route_profile -p vxn1b-engine
//!   samply record --rate 4000 --save-only -o /tmp/prof.json \
//!     target/release/examples/route_profile routed 20000

use std::time::Instant;

use vxn1b_engine::matrix::{Curve, DestId, MatrixSlot, SourceId};
use vxn1b_engine::params::{global_clap_id, patch_clap_id};
use vxn1b_engine::{Engine, Layer, MAX_VOICES, ParamId};

const SR: f32 = 48_000.0;
const FRAMES: usize = 512;
const ITERS: usize = 4_000;

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| "routed".into());
    let iters: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(ITERS);
    let os: f32 = std::env::var("OS").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let fx: f32 = std::env::var("FX").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    // Osc 1 waveform override, so the pulse's two polyBLEP evaluations per
    // sample can be A/B'd against a saw's one.
    let w1: f32 = std::env::var("W1").ok().and_then(|v| v.parse().ok()).unwrap_or(3.0);

    let (routes, sync, notes) = match mode.as_str() {
        "flat" => (false, true, true),
        "clean" => (false, false, true),
        "idle" => (true, true, false),
        _ => (true, true, true),
    };

    let mut e = Engine::new(SR);

    for (p, v) in [
        (ParamId::Oversample, os),
        (ParamId::ChorusOn, fx),
        (ParamId::DelayOn, fx),
    ] {
        if let Some(id) = global_clap_id(p) {
            e.set_param(id, v);
        }
    }

    let set = |e: &mut Engine, p: ParamId, v: f32| {
        if let Some(id) = patch_clap_id(Layer::L1, p) {
            e.set_param(id, v);
        }
    };

    // Panel: osc 1 pulse at +18 st, osc 2 as the sync slave, cross mod = sync.
    for (p, v) in [
        (ParamId::Osc1Wave, w1),    // 3 = Pulse
        (ParamId::Osc1Octave, 1.0), // +12
        (ParamId::Osc1Coarse, 6.0), // +6  => +18 st
        (ParamId::Osc1Level, 0.8),
        (ParamId::Osc1PulseWidth, 0.5),
        (ParamId::Osc2Level, 0.8),
        (ParamId::Osc2Coarse, 7.0),
        (ParamId::CrossModType, if sync { 1.0 } else { 0.0 }),
        (ParamId::CrossModAmount, 1.0),
        (ParamId::Resonance, 0.6),
        (ParamId::Lfo1Rate, 5.0),
        (ParamId::Lfo2Rate, 3.0),
        (ParamId::Env1Sustain, 0.7), // Env 1 alive at steady state for the PWM route
    ] {
        set(&mut e, p, v);
    }

    if routes {
        // Slots 0..2 are the default patch (Env2→Amp, LFO1→Pitch, Spread→Pan);
        // the chained routing goes in the free slots.
        let table = e.matrix_mut(Layer::L1);
        table.slots[3] = MatrixSlot {
            source: SourceId::Lfo2,
            dest: DestId::Lfo1Rate,
            depth: 1.0,
            curve: Curve::Lin,
            scale_src: SourceId::None,
        };
        table.slots[4] = MatrixSlot {
            source: SourceId::Lfo1,
            dest: DestId::CrossModAmount,
            depth: 0.8,
            curve: Curve::Lin,
            scale_src: SourceId::None,
        };
        table.slots[5] = MatrixSlot {
            source: SourceId::Env1,
            dest: DestId::Pwm,
            depth: 0.7,
            curve: Curve::Lin,
            scale_src: SourceId::None,
        };
        for (slot, depth) in [(3usize, 1.0f32), (4, 0.8), (5, 0.7)] {
            if let Some(p) = ParamId::slot_depth(slot) {
                set(&mut e, p, depth);
            }
        }
    }

    if notes {
        for i in 0..MAX_VOICES {
            e.note_on(0, 36 + i as u8, 1.0);
        }
    }

    let mut l = vec![0.0; FRAMES];
    let mut r = vec![0.0; FRAMES];
    for _ in 0..40 {
        e.process_block(&mut l, &mut r);
    }

    let t0 = Instant::now();
    let mut acc = 0.0f32;
    for _ in 0..iters {
        e.process_block(&mut l, &mut r);
        acc += l[0];
    }
    let dt = t0.elapsed();
    std::hint::black_box(acc);

    let audio = iters as f64 * FRAMES as f64 / SR as f64;
    println!(
        "{:<8} {:>8.3} s for {:>6.2} s audio  ({:>7.1}x realtime, {:>5.2}% of one core)",
        mode,
        dt.as_secs_f64(),
        audio,
        audio / dt.as_secs_f64(),
        100.0 * dt.as_secs_f64() / audio,
    );
}
