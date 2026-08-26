//! Profiling harness: a deliberately "busy" patch, in two voicings that cost
//! the same on paper (ticket 0264/0266).
//!
//! Ported from VXN1's [`busy_profile`](../../../../vxn-1/crates/vxn-engine/examples/busy_profile.rs),
//! adapted to VXN1b's two-synth structure and stack voicing. Both layers run a
//! hard-working patch — 4× oversample, FX, high resonance, cross-mod, and
//! matrix routes into pitch/PWM/cutoff so the evaluator does work every block.
//!
//! The point of *this* harness is the comparison ADR 0003 leaves open: 32 lanes
//! are 32 lanes whether they are 32 separate notes or one 32-wide stack, so
//! neither voicing should be meaningfully cheaper. A single stack has every
//! lane on the *same* note, which makes the pitch cascade and filter-coefficient
//! work identical per lane — that may help the vectoriser, or may do nothing,
//! but it must not *hurt*.
//!
//!   cargo run --release --example busy_profile -p vxn1b-engine -- poly
//!   cargo run --release --example busy_profile -p vxn1b-engine -- stack
//!
//! Under a sampler, drop the timing and record instead:
//!
//!   cargo build --release --example busy_profile -p vxn1b-engine
//!   samply record ./target/release/examples/busy_profile stack

use std::time::Instant;

use vxn1b_engine::params::{StackWidth, patch_clap_id};
use vxn1b_engine::{Engine, Layer, MAX_VOICES, ParamId};

const SR: f32 = 48_000.0;
const FRAMES: usize = 512;
const ITERS: usize = 4_000;

/// The two voicings under test. Both put every lane of layer 1 to work.
enum Voicing {
    /// Width 1 × `MAX_VOICES` notes — ordinary full polyphony.
    Poly,
    /// Width `MAX_VOICES` × 1 note — a single full-width stack.
    Stack,
}

impl Voicing {
    fn parse(a: Option<String>) -> Voicing {
        match a.as_deref() {
            Some("stack") => Voicing::Stack,
            _ => Voicing::Poly,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Voicing::Poly => "1 x 32 (full poly)",
            Voicing::Stack => "32 x 1 (one full-width stack)",
        }
    }

    /// Width index into [`StackWidth`], and the notes to hold.
    fn setup(&self) -> (f32, Vec<u8>) {
        match self {
            Voicing::Poly => (0.0, (0..MAX_VOICES).map(|i| 36 + i as u8).collect()),
            Voicing::Stack => {
                let widest = (StackWidth::COUNT - 1) as f32;
                (widest, vec![60])
            }
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let voicing = Voicing::parse(args.next());
    let iters: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(ITERS);

    let mut e = Engine::new(SR);
    let (width, notes) = voicing.setup();

    // Globals: 4× oversample plus FX, so the whole output stage is live.
    for (p, v) in [
        (ParamId::Oversample, 2.0),
        (ParamId::ChorusOn, 1.0),
        (ParamId::DelayOn, 1.0),
    ] {
        if let Some(id) = vxn1b_engine::params::global_clap_id(p) {
            e.set_param(id, v);
        }
    }

    // Layer 1 carries the voicing under test. Layer 2 stays off: this measures
    // one synth's lanes, not the dual-layer sum.
    let set = |e: &mut Engine, p: ParamId, v: f32| {
        if let Some(id) = patch_clap_id(Layer::L1, p) {
            e.set_param(id, v);
        }
    };
    for (p, v) in [
        (ParamId::StackWidth, width),
        (ParamId::UnisonDetune, 12.0), // a real fan, so lanes differ in pitch
        (ParamId::Resonance, 0.9),
        (ParamId::Osc1Level, 0.8),
        (ParamId::Osc2Level, 0.8),
        (ParamId::Osc2Coarse, 7.0),
        (ParamId::CrossModType, 1.0), // hard sync
        (ParamId::CrossModAmount, 0.5),
        (ParamId::Spread, 1.0), // per-lane pan, so the Pan dest is live
    ] {
        set(&mut e, p, v);
    }

    for n in notes {
        e.note_on(0, n, 1.0);
    }

    let mut l = vec![0.0; FRAMES];
    let mut r = vec![0.0; FRAMES];
    // Warm past the attack into steady state before timing.
    for _ in 0..40 {
        e.process_block(&mut l, &mut r);
    }

    let t0 = Instant::now();
    let mut acc = 0.0f32;
    for _ in 0..iters {
        e.process_block(&mut l, &mut r);
        acc += l[0]; // defeat dead-code elimination
    }
    let dt = t0.elapsed();
    std::hint::black_box(acc);

    let audio = iters as f64 * FRAMES as f64 / SR as f64;
    println!(
        "{:<32} {:>8.3} s for {:>6.2} s audio  ({:>6.1}x realtime, {:.1}% of one core)",
        voicing.label(),
        dt.as_secs_f64(),
        audio,
        audio / dt.as_secs_f64(),
        100.0 * dt.as_secs_f64() / audio,
    );
}
