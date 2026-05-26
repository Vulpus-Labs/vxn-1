//! Steady-state render benchmark. Measures the cost of rendering a full
//! 16-voice load so we can judge real-time headroom before deciding whether
//! SoA/SIMD vectorisation is worth the complexity.
//!
//! Throughput is reported in samples/sec. Divide by the sample rate (48 000)
//! to get the real-time factor at full 16-voice polyphony: e.g. 4.8 M
//! samples/sec ÷ 48 000 = 100× real-time → ~1% of one core for one instance.

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::time::Duration;
use vxn_engine::{GlobalParam, Layer, PatchParam, Synth, global_clap_id, patch_clap_id};

fn pp(p: PatchParam) -> usize {
    patch_clap_id(Layer::Upper, p)
}
fn gp(g: GlobalParam) -> usize {
    global_clap_id(g)
}

const SR: f32 = 48_000.0;
const FRAMES: usize = 512;

/// Build a synth with 16 voices held at sustain. `fx` toggles chorus + delay;
/// `res` sets filter resonance; `os` is the oversampling factor (1/2/4). `xmod`
/// selects the cross-mod kernel: 0 = off (vectorised fast osc path), 1 = hard
/// sync, 2 = through-zero phase mod — so the coupled kernels are benchmarked.
fn setup(fx: bool, res: f32, os: f32, xmod: f32) -> Synth {
    let mut s = Synth::new(SR);
    s.set_param(gp(GlobalParam::ChorusOn), if fx { 1.0 } else { 0.0 });
    s.set_param(gp(GlobalParam::DelayOn), if fx { 1.0 } else { 0.0 });
    s.set_param(gp(GlobalParam::Oversample), os);
    s.set_param(pp(PatchParam::Resonance), res);
    // Route Env 1 -> cutoff and LFO 1 -> pitch so the fixed routes do real work.
    s.set_param(pp(PatchParam::CutoffEnvDepth), 24.0); // Env 1 -> cutoff (fixed source)
    s.set_param(pp(PatchParam::PitchLfoSrc), 1.0); // LFO 1
    s.set_param(pp(PatchParam::PitchLfoDepth), 3.0);
    // CrossModType: 1=Sync, 2=PM. Detune osc2 so the coupled path does real work.
    s.set_param(pp(PatchParam::CrossModType), xmod);
    if xmod != 0.0 {
        s.set_param(pp(PatchParam::Osc2Coarse), 7.0);
    }
    if xmod == 2.0 {
        s.set_param(pp(PatchParam::CrossModAmount), 0.5);
    }
    for n in 48..64u8 {
        s.note_on(n, 1.0);
    }
    // Warm past the attack so we measure the sustained steady state.
    let mut l = vec![0.0; FRAMES];
    let mut r = vec![0.0; FRAMES];
    for _ in 0..40 {
        s.process(&mut l, &mut r);
    }
    s
}

fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("render_16_voices");
    group.throughput(Throughput::Elements(FRAMES as u64));
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(60);

    // os param value: 0.0=Off(1x), 1.0=2x, 2.0=4x. xmod: 0=off, 1=sync, 2=PM.
    for (name, fx, res, os, xmod) in [
        ("dry_1x", false, 0.2, 0.0, 0.0),
        ("dry_2x", false, 0.2, 1.0, 0.0),
        ("dry_4x", false, 0.2, 2.0, 0.0),
        ("selfosc_4x", false, 1.0, 2.0, 0.0),
        ("with_fx_2x", true, 0.2, 1.0, 0.0),
        ("sync_4x", false, 0.2, 2.0, 1.0),
        ("pm_4x", false, 0.2, 2.0, 2.0),
    ] {
        let mut s = setup(fx, res, os, xmod);
        let mut l = vec![0.0; FRAMES];
        let mut r = vec![0.0; FRAMES];
        group.bench_function(name, |b| {
            b.iter(|| s.process(black_box(&mut l), black_box(&mut r)));
        });
    }
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
