//! Audio-domain zipper regression for LFO → OpNLevel / OpNPan matrix routes.
//! The state-convergence tests in `engine.rs` assert the ramp's bookkeeping;
//! this asserts the rendered audio. Detector: mean |second difference| of the
//! output at block-edge sample offsets vs the block interior. A stepped (or too
//! coarsely interpolated) control leaves d² impulses at block edges; a correct
//! per-sample ramp at the 32-sample control rate leaves edge ≈ interior
//! (measured ~1.08; bound 1.5).
//!
//! The render is driven the way the CLAP shell drives the engine: 32-sample
//! control blocks with `apply_block_params` re-applied per process cycle,
//! regardless of host buffer size.

use vxn2_engine::MatrixRowRaw;
use vxn2_engine::engine::Engine;

const SR: f32 = 48_000.0;
const BLK: usize = 32;
const MAX_EDGE_RATIO: f64 = 1.5;

fn edge_interior_ratio_of(buf: &[f32]) -> f64 {
    let mut sum = [0.0_f64; BLK];
    let mut cnt = [0u64; BLK];
    for i in 1..buf.len() - 1 {
        sum[i % BLK] += (buf[i + 1] - 2.0 * buf[i] + buf[i - 1]).abs() as f64;
        cnt[i % BLK] += 1;
    }
    let mean: Vec<f64> = (0..BLK).map(|i| sum[i] / cnt[i].max(1) as f64).collect();
    let edge = (mean[BLK - 1] + mean[0] + mean[1]) / 3.0;
    let interior: f64 = mean[4..BLK - 4].iter().sum::<f64>() / (BLK - 8) as f64;
    edge / interior
}

fn edge_interior_ratio(dest: u8) -> (f64, f64) {
    let mut e = Engine::new(SR, BLK);
    // Dry render: FX tails would only dilute the detector.
    e.params.delay.on = false;
    e.params.delay.mix = 0.0;
    e.params.reverb.on = false;
    e.params.reverb.mix = 0.0;
    e.params.mod_params.lfo1.rate_hz = 5.0;
    e.params.patch.stack.density = 4; // give the pan fold moving lanes
    e.params.matrix_rows[0] = MatrixRowRaw {
        source: 1, // Lfo1
        dest,
        curve: 0,
        active: true,
        depth: 1.0,
        scale_src: 0,
    };
    e.params.mtx_depths[0] = 1.0;
    e.apply_block_params();
    e.note_on(60, 100);

    let mut l = [0.0_f32; BLK];
    let mut r = [0.0_f32; BLK];
    // Warm 0.5 s past the attack so EG motion doesn't pollute the
    // measurement window, then capture 1 s.
    for _ in 0..(SR as usize / 2 / BLK) {
        e.apply_block_params();
        e.process_block(&mut l, &mut r);
    }
    let nblocks = SR as usize / BLK;
    let mut buf_l = Vec::with_capacity(nblocks * BLK);
    let mut buf_r = Vec::with_capacity(nblocks * BLK);
    for _ in 0..nblocks {
        e.apply_block_params();
        e.process_block(&mut l, &mut r);
        buf_l.extend_from_slice(&l);
        buf_r.extend_from_slice(&r);
    }

    (edge_interior_ratio_of(&buf_l), edge_interior_ratio_of(&buf_r))
}

#[test]
fn lfo_level_route_leaves_no_block_edge_zipper() {
    let (l, r) = edge_interior_ratio(2); // Op1Level
    assert!(
        l < MAX_EDGE_RATIO && r < MAX_EDGE_RATIO,
        "level-route block-edge d² ratio L={l:.2} R={r:.2} (want < {MAX_EDGE_RATIO})"
    );
}

#[test]
fn lfo_pan_route_leaves_no_block_edge_zipper() {
    let (l, r) = edge_interior_ratio(3); // Op1Pan
    assert!(
        l < MAX_EDGE_RATIO && r < MAX_EDGE_RATIO,
        "pan-route block-edge d² ratio L={l:.2} R={r:.2} (want < {MAX_EDGE_RATIO})"
    );
}

/// Master volume is a block-rate scalar: a slider drag or automation lane
/// writes a fresh `volume_db` once per control block. Applying it as a raw
/// block-constant multiply steps the gain at every block edge → audible
/// zipper. The per-sample smoother (`MasterState`) must spread that step so
/// the block-edge d² stays ≈ interior. Same detector as the LFO routes.
#[test]
fn master_volume_sweep_leaves_no_block_edge_zipper() {
    let mut e = Engine::new(SR, BLK);
    // Dry — FX tails only dilute the detector.
    e.params.delay.on = false;
    e.params.delay.mix = 0.0;
    e.params.reverb.on = false;
    e.params.reverb.mix = 0.0;
    e.apply_block_params();
    e.note_on(60, 100);

    let mut l = [0.0_f32; BLK];
    let mut r = [0.0_f32; BLK];
    // Warm past the attack so EG motion doesn't pollute the window.
    for _ in 0..(SR as usize / 2 / BLK) {
        e.process_block(&mut l, &mut r);
    }

    let nblocks = SR as usize / BLK;
    let mut buf = Vec::with_capacity(nblocks * BLK);
    for b in 0..nblocks {
        // Continuously sweep the master volume every control block — the
        // gesture a fader move / automation lane produces.
        e.params.master.volume_db = -12.0 + 9.0 * (b as f32 * 0.05).sin();
        e.apply_block_params();
        e.process_block(&mut l, &mut r);
        buf.extend_from_slice(&l);
    }

    let ratio = edge_interior_ratio_of(&buf);
    assert!(
        ratio < MAX_EDGE_RATIO,
        "master-volume block-edge d² ratio {ratio:.2} (want < {MAX_EDGE_RATIO})"
    );
}

/// The dynamics comp/sat params (threshold, ratio, makeup, drive) each scale
/// the signal per sample. Without per-sample smoothing, a swept slider writes
/// a fresh value once per control block → a gain step at every block edge →
/// audible crackle. Sweep makeup (the most direct multiply) every block and
/// assert the block-edge d² stays ≈ interior. Same detector as the LFO routes
/// and master volume.
#[test]
fn dynamics_makeup_sweep_leaves_no_block_edge_zipper() {
    let mut e = Engine::new(SR, BLK);
    // Dry tails off — only the dynamics block under test should color the
    // signal.
    e.params.delay.on = false;
    e.params.delay.mix = 0.0;
    e.params.reverb.on = false;
    e.params.reverb.mix = 0.0;
    e.params.dynamics.on = true;
    e.params.dynamics.threshold_db = -24.0;
    e.params.dynamics.ratio = 4.0;
    e.params.dynamics.drive_db = 12.0;
    e.params.dynamics.mix = 1.0;
    e.apply_block_params();
    e.note_on(60, 100);

    let mut l = [0.0_f32; BLK];
    let mut r = [0.0_f32; BLK];
    // Warm past the attack so EG motion doesn't pollute the window.
    for _ in 0..(SR as usize / 2 / BLK) {
        e.apply_block_params();
        e.process_block(&mut l, &mut r);
    }

    let nblocks = SR as usize / BLK;
    let mut buf = Vec::with_capacity(nblocks * BLK);
    for b in 0..nblocks {
        // Sweep makeup across its full 0..24 dB range every control block —
        // the gesture a fast slider drag produces.
        e.params.dynamics.makeup_db = 12.0 + 12.0 * (b as f32 * 0.05).sin();
        e.apply_block_params();
        e.process_block(&mut l, &mut r);
        buf.extend_from_slice(&l);
    }

    let ratio = edge_interior_ratio_of(&buf);
    assert!(
        ratio < MAX_EDGE_RATIO,
        "dynamics-makeup block-edge d² ratio {ratio:.2} (want < {MAX_EDGE_RATIO})"
    );
}
