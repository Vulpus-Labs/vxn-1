//! Standalone driver for profiling — runs the Whistle Down The Wind patch at
//! 8 notes (16 voices, saturating both layers in Dual mode) for a fixed
//! number of seconds. Designed to be wrapped by `samply record`.

use std::path::Path;
use std::time::{Duration, Instant};
use vxn_engine::{Synth, load_preset_file};

const SR: f32 = 48_000.0;
const FRAMES: usize = 512;
const PRESET_PATH: &str =
    "/Users/dominicfox/Library/Audio/Presets/Vulpus Labs/VXN1/Whistle Down The Wind.toml";

fn main() {
    let (perf, _) = load_preset_file(Path::new(PRESET_PATH)).expect("load whistle preset");
    let mut s = Synth::new(SR);
    *s.params_mut() = perf.state.params;
    s.set_key_mode(perf.state.key_mode);
    s.set_split_point(perf.state.split_point);

    for i in 0..8u8 {
        s.note_on(60 + i * 3, 1.0);
    }

    let mut l = vec![0.0; FRAMES];
    let mut r = vec![0.0; FRAMES];

    for _ in 0..120 {
        s.process(&mut l, &mut r);
    }

    let secs: f64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(5.0);
    let deadline = Instant::now() + Duration::from_secs_f64(secs);
    let mut blocks: u64 = 0;
    while Instant::now() < deadline {
        for _ in 0..200 {
            s.process(&mut l, &mut r);
            blocks += 1;
        }
    }
    eprintln!("blocks={} ({:.0} µs/block avg)", blocks, secs * 1_000_000.0 / blocks as f64);
    std::hint::black_box(&l);
}
