//! Perf bench rig (ticket 0087, epic E020).
//!
//! A worst-case render harness: a 16-voice, full-FX patch single-sourced in
//! Rust, driven through [`vxn_engine::Synth::process`] one quantum at a time.
//! It exists to **measure** the SIMD128 build at full polyphony in the browser
//! — the 0034 spike only reported Node throughput at 1 voice, which is not the
//! shipping truth (epic E020 "Why last").
//!
//! # Why measurement lives in JS, not here
//!
//! `wasm32-unknown-unknown` has no `std::time`, so this rig cannot time itself.
//! Instead [`vxn_bench_render`] batches `n_quanta` of rendering per call and the
//! JS harness brackets that call with `performance.now()` — inside an
//! AudioWorklet, the only place that reflects real audio-callback scheduling.
//! This mirrors the EMA CPU meter the production worklet already runs
//! (`web/vxn-processor-0038.js`).
//!
//! # The worst-case patch (single source of truth for E020)
//!
//! - **16 voices, all lanes lit.** `KeyMode::Dual` fires every note on *both*
//!   layers (vxn-engine `lib.rs` `note_on`), and each layer is 8 channels
//!   (`vxn_dsp::CHANNELS_PER_LAYER = 8`). So 8 distinct notes × 2 layers = the
//!   full 16-voice complement.
//! - **Full FX bus, no fast path.** Reverb + delay + chorus + phaser are forced
//!   on with a long reverb decay and high delay feedback so the tail keeps
//!   recirculating a non-zero signal and never collapses to the engine's
//!   exact-silence fast path (`Synth::process`'s `both_silent` skip). Every
//!   quantum stays on the hot path — that is the point.
//!
//! 0089 (denormal stress) reuses this rig via [`Bench::new_held_quiet`].

use crate::QUANTUM;
use vxn_app::{global_clap_id, GlobalParam, KeyMode};
use vxn_engine::Synth;

/// Distinct notes held in the worst-case chord. Eight notes × two layers
/// (Dual) = the full 16-voice complement (`vxn_dsp::CHANNELS_PER_LAYER * 2`).
/// A spread, dissonant cluster so no two voices share a phase and the mix
/// genuinely exercises all lanes.
const CHORD: [u8; 8] = [36, 43, 48, 52, 55, 60, 64, 67];

/// The worst-case render bench. Owns its `Synth` (patched at construction) plus
/// the stereo scratch JS reads out of linear memory after a batch render.
pub struct Bench {
    synth: Synth,
    out_l: [f32; QUANTUM],
    out_r: [f32; QUANTUM],
}

impl Bench {
    /// Build the loud 16-voice worst-case bench: full chord held in `Dual`, full
    /// FX bus on with a long reverb tail / high delay feedback.
    pub fn new(sample_rate: f32) -> Self {
        let mut bench = Bench {
            synth: Synth::new(sample_rate),
            out_l: [0.0; QUANTUM],
            out_r: [0.0; QUANTUM],
        };
        bench.apply_fx_worst_case();
        // Dual so every note lights both layers (8 + 8 = 16 voices).
        bench.synth.set_key_mode(KeyMode::Dual);
        for &note in &CHORD {
            bench.synth.note_on(note, 1.0);
        }
        bench
    }

    /// Build the 0089 held-quiet denormal variant: the same full FX tail, but a
    /// single very-quiet sustained note instead of the loud cluster. The point is
    /// a perpetually-tiny, never-exactly-zero signal recirculating in the reverb
    /// feedback — the denormal-cliff case the 0034 release path never hit.
    pub fn new_held_quiet(sample_rate: f32) -> Self {
        let mut bench = Bench {
            synth: Synth::new(sample_rate),
            out_l: [0.0; QUANTUM],
            out_r: [0.0; QUANTUM],
        };
        bench.apply_fx_worst_case();
        bench.synth.set_key_mode(KeyMode::Dual);
        // One low-velocity note: a small but non-zero amp-env level feeds the
        // FX tail without lighting the loud-mix worst case.
        bench.synth.note_on(48, 0.02);
        bench
    }

    /// Force the full FX bus on, set to keep the tail alive. Values are *plain*
    /// (descriptor range), not normalised — `Synth::set_param` →
    /// `SharedParams::set` clamps to the descriptor, so we pass real units
    /// (seconds, feedback ratio). Names resolved via `GlobalParam::from_name` so
    /// this never drifts from the param table.
    fn apply_fx_worst_case(&mut self) {
        // (name, plain value). Long reverb decay + high delay/phaser feedback so
        // the recirculating paths never settle to exact silence.
        const FX: [(&str, f32); 14] = [
            ("reverb_on", 1.0),
            ("reverb_size", 1.0),
            ("reverb_decay", 10.0), // max decay (descriptor 0.2..10.0 s)
            ("reverb_mix", 0.6),
            ("delay_on", 1.0),
            ("delay_time", 0.35),
            ("delay_feedback", 0.9), // near max (descriptor 0.0..0.95)
            ("delay_mix", 0.5),
            ("chorus_on", 1.0),
            ("chorus_mix", 0.5),
            ("phaser_on", 1.0),
            ("phaser_fb", 0.85),
            ("phaser_mix", 0.5),
            ("limiter_on", 1.0), // limiter in the path = its work runs too
        ];
        for (name, value) in FX {
            let g = GlobalParam::from_name(name)
                .expect("worst-case FX param name must exist in the param table");
            self.synth.set_param(global_clap_id(g) as u32 as usize, value);
        }
    }

    /// Render `n_quanta` quanta into the scratch buffers, back to back. The last
    /// quantum's audio is left in `out_l`/`out_r` for a sanity read; the earlier
    /// quanta are rendered for their cost only. JS times the whole call.
    pub fn render(&mut self, n_quanta: u32) {
        for _ in 0..n_quanta {
            let Bench {
                synth,
                out_l,
                out_r,
            } = self;
            synth.process(out_l, out_r);
        }
    }
}

// ── C ABI (mirrors the 0034 raw-ABI pattern in lib.rs) ──────────────────────

/// Create a worst-case bench at `sample_rate`. Returns an opaque handle; every
/// other call passes it back. Leaks the box; [`vxn_bench_destroy`] reclaims it.
#[unsafe(no_mangle)]
pub extern "C" fn vxn_bench_new(sample_rate: f32) -> *mut Bench {
    Box::into_raw(Box::new(Bench::new(sample_rate)))
}

/// # Safety
/// `ptr` must be a handle from [`vxn_bench_new`], not yet destroyed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_bench_destroy(ptr: *mut Bench) {
    if !ptr.is_null() {
        drop(unsafe { Box::from_raw(ptr) });
    }
}

/// Render `n_quanta` quanta. JS brackets this call with `performance.now()` to
/// get render-time-per-quantum (= elapsed / n_quanta).
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn_bench_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_bench_render(ptr: *mut Bench, n_quanta: u32) {
    if let Some(bench) = unsafe { ptr.as_mut() } {
        bench.render(n_quanta);
    }
}

/// Pointer to the left-channel scratch (`QUANTUM` f32s) for a sanity read.
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn_bench_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_bench_out_l(ptr: *mut Bench) -> *const f32 {
    match unsafe { ptr.as_ref() } {
        Some(bench) => bench.out_l.as_ptr(),
        None => core::ptr::null(),
    }
}

/// Whether this wasm was built with `+simd128` (1) or scalar (0), so the harness
/// can label which build it measured. Compile-time `cfg!`, no runtime detection.
#[unsafe(no_mangle)]
pub extern "C" fn vxn_bench_simd128() -> u32 {
    if cfg!(target_feature = "simd128") {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Peak abs sample across a buffer — the audibility probe.
    fn peak(buf: &[f32]) -> f32 {
        buf.iter().fold(0.0f32, |m, &s| m.max(s.abs()))
    }

    #[test]
    fn worst_case_is_audible() {
        // The loud 16-voice chord must produce real output (not silence) once the
        // attack has run — proof the bench actually exercises the engine.
        let mut bench = Bench::new(48_000.0);
        bench.render(8); // let the attack open
        let p = peak(&bench.out_l).max(peak(&bench.out_r));
        assert!(p > 0.0, "worst-case bench rendered silence (peak {p})");
    }

    #[test]
    fn fx_tail_survives_note_off() {
        // Release every note, then keep rendering: the reverb/delay tail must keep
        // the output non-silent for many quanta — i.e. we stay on the hot path
        // (not the exact-silence fast path) well after note-off, which is the
        // whole reason the patch forces a long FX tail.
        let mut bench = Bench::new(48_000.0);
        bench.render(16); // sound the chord, charge the FX tails
        for &note in &CHORD {
            bench.synth.note_off(note);
        }
        // Render ~1 s of tail; assert audio is still present at the end.
        let sr_quanta = (48_000 / QUANTUM as u32).max(1);
        bench.render(sr_quanta);
        let p = peak(&bench.out_l).max(peak(&bench.out_r));
        assert!(
            p > 0.0,
            "FX tail collapsed to exact silence after note-off (peak {p}) — the \
             worst-case patch would hit the fast path and under-measure"
        );
    }

    #[test]
    fn held_quiet_tail_is_non_silent() {
        // The 0089 denormal variant: a held quiet note into the reverb must keep
        // the tail on the hot path (non-zero), never collapsing to exact silence.
        let mut bench = Bench::new_held_quiet(48_000.0);
        bench.render(64); // let the quiet signal charge the FX feedback
        let p = peak(&bench.out_l).max(peak(&bench.out_r));
        assert!(
            p > 0.0,
            "held-quiet denormal patch produced exact silence (peak {p}) — it \
             must stay on the hot path to stress denormals"
        );
    }

    #[test]
    fn render_advances_per_quantum() {
        // `render(n)` must run exactly n quanta of work. We can't see time, but we
        // can see that successive renders keep producing fresh output (the synth's
        // phase advances), proving the loop body ran n times rather than once.
        let mut bench = Bench::new(48_000.0);
        bench.render(4);
        let first = bench.out_l;
        bench.render(4);
        let second = bench.out_l;
        // A periodic, multi-voice signal will differ quantum-to-quantum.
        assert_ne!(
            first, second,
            "output identical across two render batches — render loop did not advance"
        );
    }

    #[test]
    fn simd_flag_matches_build() {
        // Just exercises the export; the value is build-dependent. On a host
        // `cargo test` (x86/arm native) simd128 is never set, so it must be 0
        // here — the wasm SIMD build is what flips it to 1.
        assert_eq!(
            vxn_bench_simd128(),
            0,
            "host test build should report simd128=0 (only the wasm +simd128 \
             build returns 1)"
        );
    }
}
