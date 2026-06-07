//! Throwaway bench: load the user patch "Whistle Down The Wind" and measure
//! steady-state render cost across varying poly note counts (1/2/4/8). The
//! patch is Dual key-mode, so each note allocates one voice on Upper and one
//! on Lower — 8 notes saturates the 16-voice budget. Also runs 8x OS, all FX
//! on, unison detune, drive, BP filter — a deliberately hot preset.

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::time::Duration;
use std::path::Path;
use vxn_engine::{Synth, load_preset_file};

const SR: f32 = 48_000.0;
const FRAMES: usize = 512;
const PRESET_PATH: &str =
    "/Users/dominicfox/Library/Audio/Presets/Vulpus Labs/VXN1/Whistle Down The Wind.toml";

fn build_synth(notes: u8) -> Synth {
    let (perf, _warnings) =
        load_preset_file(Path::new(PRESET_PATH)).expect("load whistle preset");

    let mut s = Synth::new(SR);
    *s.params_mut() = perf.state.params;
    s.set_key_mode(perf.state.key_mode);
    s.set_split_point(perf.state.split_point);

    // Spread notes around middle C so the split point (60) gets straddled when
    // we eventually grow past one note — but this preset is Dual, so each note
    // lights both layers anyway.
    let base: u8 = 60;
    for i in 0..notes {
        let n = base.saturating_add(i * 3); // root, +min3, +tritone-ish, ...
        s.note_on(n, 1.0);
    }

    // Warm past the attack so we measure the sustained steady state.
    let mut l = vec![0.0; FRAMES];
    let mut r = vec![0.0; FRAMES];
    for _ in 0..120 {
        s.process(&mut l, &mut r);
    }
    s
}

fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("whistle");
    group.throughput(Throughput::Elements(FRAMES as u64));
    group.measurement_time(Duration::from_secs(4));
    group.sample_size(60);

    for notes in [1u8, 2, 4, 8] {
        let mut s = build_synth(notes);
        let mut l = vec![0.0; FRAMES];
        let mut r = vec![0.0; FRAMES];
        let name = format!("{}_notes", notes);
        group.bench_function(&name, |b| {
            b.iter(|| s.process(black_box(&mut l), black_box(&mut r)));
        });
    }

    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
