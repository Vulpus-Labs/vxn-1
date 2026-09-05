//! Fixtures for the vxn-4 operator-block sizing bench.
//!
//! One place to define what "a realistic 8-operator voice" means, so the
//! criterion benches and the `sweep` binary cannot drift apart and report
//! different numbers for the same configuration.
//!
//! Two routings are provided, and the difference between them is the difference
//! between a worst case and a typical one:
//!
//! - [`dense_routing`] — all 64 modulation routes live. This is what sets the
//!   polyphony floor, and it is the number to quote when sizing.
//! - [`sparse_routing`] — a DX7-shaped algorithm, 8 routes and one self-feedback.
//!   What most patches will actually cost.

use vxn4_dsp::ops::{CompiledRouting, NOPS, OpConfig, Routing, SumBus};
use vxn4_dsp::wavetable::{WaveBank, Waveform};

/// Samples of 1x output per render call.
pub const BLOCK_1X: usize = 64;

/// Host sample rate the polyphony figures are quoted against.
pub const SR: f32 = 48_000.0;

/// Eight operators across all four waveforms, with ratios spread far enough
/// apart that they select different mips and the gathers genuinely scatter.
///
/// A uniform bank of sine operators at ratio 1.0 would put every lookup in one
/// cache line and flatter the result by a wide margin.
pub fn configs() -> [OpConfig; NOPS] {
    let waves = [
        Waveform::Sine,
        Waveform::Saw,
        Waveform::Square,
        Waveform::Triangle,
        Waveform::Sine,
        Waveform::Saw,
        Waveform::Triangle,
        Waveform::Square,
    ];
    // 0.5 to 11.0 spans about 4.5 octaves, so mip selection spreads over
    // several levels within a single voice.
    let ratios = [1.0, 2.0, 3.0, 0.5, 7.0, 1.0, 4.0, 11.0];
    let mut cfg = [OpConfig::default(); NOPS];
    for (d, slot) in cfg.iter_mut().enumerate() {
        *slot = OpConfig {
            wave: waves[d],
            ratio: ratios[d],
            level: 0.7,
            pan: (d as f32 / 3.5) - 1.0,
        };
    }
    cfg
}

/// All 64 routes live. The worst case, and the one that sizes the synth.
///
/// Depths are small (~0.02–0.03 turns per unit) because 64 simultaneous routes
/// at patch-typical depth would drive every operator into broadband noise —
/// which would be a fair stress test of the arithmetic but an unfair one of the
/// gathers, since a fully scrambled phase defeats every locality effect the
/// bench is trying to measure.
pub fn dense_routing() -> Routing {
    let mut r = Routing::default();
    for d in 0..NOPS {
        for s in 0..NOPS {
            r.pm[d][s] = 0.02 + 0.004 * ((d * NOPS + s) as f32 / 64.0);
        }
        r.out[d] = 0.125;
    }
    r
}

/// A DX7-shaped algorithm: two 3-operator stacks and a 2-operator stack, with
/// self-feedback on the topmost modulator. 9 of 64 routes live.
pub fn sparse_routing() -> Routing {
    let mut r = Routing::default();
    // Stack A: 2 -> 1 -> 0 (carrier), feedback on 2.
    r.pm[0][1] = 0.9;
    r.pm[1][2] = 0.7;
    r.pm[2][2] = 0.35;
    // Stack B: 5 -> 4 -> 3 (carrier).
    r.pm[3][4] = 0.8;
    r.pm[4][5] = 0.5;
    // Stack C: 7 -> 6 (carrier).
    r.pm[6][7] = 1.1;
    // A little cross-coupling, as the vxn-4 matrix allows and a DX7 does not.
    r.pm[3][2] = 0.2;
    r.pm[6][1] = 0.15;
    r.pm[0][7] = 0.1;

    for d in [0usize, 3, 6] {
        r.out[d] = 0.33;
    }
    r
}

/// Keys spread over five octaves, so voices land on different mips.
pub fn keys<const V: usize>() -> [u8; V] {
    let mut k = [0u8; V];
    for (v, slot) in k.iter_mut().enumerate() {
        *slot = 36 + (v as u8).wrapping_mul(7) % 60;
    }
    k
}

/// Everything a render call needs, built once.
pub struct Fixture {
    pub bank: WaveBank,
    pub cfg: [OpConfig; NOPS],
    pub routing: Routing,
    pub compiled: CompiledRouting,
    pub bus: SumBus,
    pub base_len: usize,
    pub os: usize,
}

impl Fixture {
    pub fn new(base_len: usize, os: usize, routing: Routing) -> Self {
        let cfg = configs();
        let bus = SumBus::new(&cfg, &routing);
        let compiled = CompiledRouting::compile(&routing);
        Self {
            bank: WaveBank::new(base_len),
            cfg,
            routing,
            compiled,
            bus,
            base_len,
            os,
        }
    }

    /// Oversampled rate the operator block runs at.
    pub fn sr_os(&self) -> f32 {
        SR * self.os as f32
    }

    /// Tap bytes one voice touches, assuming each operator holds one mip.
    ///
    /// This is the number that decides whether the gathers stay in L1. Apple
    /// M-series cores have 128 KiB of L1d; most x86 cores have 32–48 KiB.
    pub fn voice_working_set(&self) -> usize {
        // Operators sit on different mips; mip 1 is a fair midpoint for the
        // ratio spread in `configs`.
        NOPS * self.bank.mip_tap_bytes(1)
    }
}
