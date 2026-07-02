//! Worklet audio-host (ticket 0038) — the production render loop.
//!
//! This is the web analogue of `vxn-clap`'s audio-thread half
//! (`VxnAudioProcessor::process`, vxn-clap/src/lib.rs:286-390). Where the CLAP
//! host hands `process()` a sample-accurate event list *on the audio thread*,
//! the browser splits controller (main thread) from renderer (worklet). The
//! transport that bridges them is the 0035 SAB event ring; this host is what
//! consumes it on the render side and turns it into audio with the same
//! sample-accurate slicing the plugin gets for free.
//!
//! # Why a Rust host (vs the 0035 JS loop)
//!
//! The 0035 spike drove the slice loop from JS, calling back into wasm once per
//! slice and once per event (`vxn_process_slice` / `vxn_set_param`). That proved
//! the mechanism but crosses the JS↔wasm boundary O(events + slices) times per
//! quantum. This host does it in **one** `vxn_host_render` call: JS copies the
//! ring's raw wire bytes into a linear-memory scratch, then Rust decodes
//! (`codec`), slices, and renders entirely inside wasm. The engine is unchanged
//! — `Synth::process` still renders contiguous slices; the host owns slicing,
//! exactly the plugin's contract.
//!
//! # Per-quantum loop (mirrors the CLAP batch loop)
//!
//! 1. Apply non-automatable shared state **once, before event ingestion**:
//!    `set_key_mode` / `set_split_point` (passed as args, the `SharedParams`
//!    analogue — ADR 0003 §3; these are not param ids). This matches the plugin
//!    setting them once per block before the event loop.
//! 2. Slice the block at event sample-offsets: apply every event at offset `k`
//!    (decode+apply via [`crate::codec`]), render `[prev..k)`, advance, repeat;
//!    render the tail. Records arrive in offset order (single SPSC producer).
//! 3. Parameters: sample-accurate param automation rides the ring as `EV_PARAM`
//!    records and is applied at its offset by the slice loop. The 0039 param
//!    store (current-value, lock-free) is applied block-start by the worklet via
//!    [`vxn_host_set_param`] before this call — the `LocalParams` fold analogue.

use crate::codec::{self, SLOT_BYTES};
use crate::QUANTUM;
use vxn_app::KeyMode;
use vxn_engine::Synth;

/// Max events decoded per quantum. Matches the 0035 ring capacity
/// (`DEFAULT_CAPACITY = 1024`), so a full ring drains in one render.
pub const MAX_EVENTS: usize = 1024;

/// The worklet audio-host: a `Synth`, its stereo output (read straight out of
/// linear memory by JS), and the event-decode scratch JS copies ring bytes into.
pub struct Host {
    synth: Synth,
    out_l: [f32; QUANTUM],
    out_r: [f32; QUANTUM],
    /// Raw 16-byte wire records for the current quantum. JS writes here (via the
    /// pointer from [`vxn_host_events_ptr`]) then calls [`vxn_host_render`].
    events: [u8; SLOT_BYTES * MAX_EVENTS],
}

impl Host {
    fn new(sample_rate: f32) -> Self {
        Host {
            synth: Synth::new(sample_rate),
            out_l: [0.0; QUANTUM],
            out_r: [0.0; QUANTUM],
            events: [0u8; SLOT_BYTES * MAX_EVENTS],
        }
    }

    /// The render loop proper, factored out so it is unit-testable without the
    /// C-ABI pointer dance. Renders one quantum into `out_l`/`out_r` from the
    /// first `n` records in `events`, slicing at each record's sample offset.
    fn render(&mut self, n: usize, key_mode: u8, split_point: u8) {
        // (1) Non-automatable shared state, once, before any events.
        self.synth.set_key_mode(KeyMode::from_u8(key_mode));
        self.synth.set_split_point(split_point);

        // Disjoint field borrows so decode (reads `events`) and render (writes
        // the output buffers, mutates `synth`) can coexist — the plugin does the
        // same split (vxn-clap/src/lib.rs:330-333).
        let Host {
            synth,
            out_l,
            out_r,
            events,
        } = self;

        let n = n.min(MAX_EVENTS);
        let q = QUANTUM;
        let mut prev = 0usize;
        let mut i = 0usize;

        // (2) The CLAP batch loop, ported (vxn-clap/src/lib.rs:335-369 +
        // event-ring.mjs `renderQuantumSliced`).
        while i < n {
            let off = (events[i * SLOT_BYTES + 1] as usize).min(q);
            // Render everything strictly before this event's offset.
            if off > prev {
                synth.process(&mut out_l[prev..off], &mut out_r[prev..off]);
                prev = off;
            }
            // Apply ALL events at this same offset (one CLAP batch boundary).
            while i < n && (events[i * SLOT_BYTES + 1] as usize).min(q) == off {
                let base = i * SLOT_BYTES;
                codec::decode_and_apply(&events[base..base + SLOT_BYTES], synth);
                i += 1;
            }
        }
        // Render the tail.
        if prev < q {
            synth.process(&mut out_l[prev..q], &mut out_r[prev..q]);
        }
    }
}

// ── C ABI (raw `WebAssembly.instantiate`, no wasm-bindgen — 0034 pattern) ────

/// Create a host at `sample_rate`. Returns an opaque handle (pointer) every
/// other call passes back. Leaks the box; [`vxn_host_destroy`] reclaims it.
#[unsafe(no_mangle)]
pub extern "C" fn vxn_host_new(sample_rate: f32) -> *mut Host {
    Box::into_raw(Box::new(Host::new(sample_rate)))
}

/// # Safety
/// `ptr` must be a handle from [`vxn_host_new`], not yet destroyed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_host_destroy(ptr: *mut Host) {
    if !ptr.is_null() {
        drop(unsafe { Box::from_raw(ptr) });
    }
}

/// Pointer to the event-decode scratch in linear memory. JS copies drained ring
/// records here (`n * 16` bytes, offset-ordered) before [`vxn_host_render`].
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn_host_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_host_events_ptr(ptr: *mut Host) -> *mut u8 {
    match unsafe { ptr.as_mut() } {
        Some(h) => h.events.as_mut_ptr(),
        None => core::ptr::null_mut(),
    }
}

/// Capacity of the event scratch in records (so JS never overruns it).
#[unsafe(no_mangle)]
pub extern "C" fn vxn_host_max_events() -> u32 {
    MAX_EVENTS as u32
}

/// Set a param by CLAP id on the host's synth. The worklet calls this block-
/// start for each param the 0039 store reports changed (the `LocalParams` fold),
/// before [`vxn_host_render`]. Sample-accurate param automation does NOT use
/// this — it rides the ring as `EV_PARAM` and is applied at its offset.
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn_host_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_host_set_param(ptr: *mut Host, index: u32, value: f32) {
    if let Some(h) = unsafe { ptr.as_mut() } {
        h.synth.set_param(index as usize, value);
    }
}

/// Read a param's current PLAIN value by CLAP id. The main-thread coordinator
/// (ticket 0042) snapshots all 165 of these off a throwaway host to SEED the
/// 0039 param store with the engine's defaults before the worklet starts —
/// otherwise the store's zero-initialised slots would fold zeros over every
/// param on the first quantum (silencing the voice). The real controller wasm
/// (0044) owns defaults via vxn-app's descriptors; this getter is the headless/
/// pre-controller seam that keeps the store honest.
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn_host_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_host_get_param(ptr: *mut Host, index: u32) -> f32 {
    match unsafe { ptr.as_ref() } {
        Some(h) => h.synth.params().get_by_clap_id(index as usize),
        None => 0.0,
    }
}

/// Rebuild the engine at a new sample rate (context sample-rate change). The
/// AudioWorklet `sampleRate` is fixed per context, so in practice this is hit by
/// a context teardown/rebuild or offline render; wired for completeness (0040).
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn_host_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_host_set_sample_rate(ptr: *mut Host, sample_rate: f32) {
    if let Some(h) = unsafe { ptr.as_mut() } {
        h.synth.set_sample_rate(sample_rate);
    }
}

/// All-notes-off / clear voice state without freeing the host or touching the
/// ring/store. The worklet calls this on resume-after-suspend to avoid stuck
/// notes (0040), and on re-init recovery.
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn_host_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_host_reset(ptr: *mut Host) {
    if let Some(h) = unsafe { ptr.as_mut() } {
        h.synth.reset();
    }
}

/// **Test-only**: force a wasm trap (Rust panic → `unreachable` on wasm →
/// `WebAssembly.RuntimeError` in JS). Exists so the 0040 trap-safety harness can
/// prove the worklet boundary catches a render-thread trap and recovers. Never
/// called in production.
#[unsafe(no_mangle)]
pub extern "C" fn vxn_host_force_trap() {
    panic!("forced trap (0040 trap-safety test)");
}

/// Render one quantum: apply `key_mode`/`split_point` once, then slice the block
/// at the offsets of the first `n_events` records in the scratch and render each
/// slice. Output lands in the buffers exposed by [`vxn_host_out_l`] /
/// [`vxn_host_out_r`].
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn_host_new`]; the scratch must hold at
/// least `n_events` valid 16-byte records (written via [`vxn_host_events_ptr`]).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_host_render(
    ptr: *mut Host,
    n_events: u32,
    key_mode: u8,
    split_point: u8,
) {
    if let Some(h) = unsafe { ptr.as_mut() } {
        h.render(n_events as usize, key_mode, split_point);
    }
}

/// Pointer to the left output buffer (`QUANTUM` f32s). JS copies from here after
/// [`vxn_host_render`].
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn_host_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_host_out_l(ptr: *mut Host) -> *const f32 {
    match unsafe { ptr.as_ref() } {
        Some(h) => h.out_l.as_ptr(),
        None => core::ptr::null(),
    }
}

/// Pointer to the right output buffer (`QUANTUM` f32s).
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn_host_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_host_out_r(ptr: *mut Host) -> *const f32 {
    match unsafe { ptr.as_ref() } {
        Some(h) => h.out_r.as_ptr(),
        None => core::ptr::null(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{encode, Event};

    /// First strictly-non-zero output frame (the engine's silent-skip drives
    /// un-noted output to exact 0.0, so onset detection is threshold-free —
    /// same probe the 0035 harness uses).
    fn onset(buf: &[f32]) -> Option<usize> {
        buf.iter().position(|&s| s != 0.0)
    }

    /// Write `events` into a host's scratch as raw wire bytes, mirroring what JS
    /// copies out of the ring.
    fn load(host: &mut Host, events: &[Event]) {
        for (i, ev) in events.iter().enumerate() {
            host.events[i * SLOT_BYTES..(i + 1) * SLOT_BYTES].copy_from_slice(&encode(ev));
        }
    }

    #[test]
    fn note_on_takes_effect_at_its_sub_block_offset() {
        // A note-on written for offset N must produce its first non-zero sample
        // at N (+1 frame of engine attack latency, as the 0035 harness measured)
        // — NOT at block start. This is the timing-parity-with-CLAP criterion.
        //
        // Render TWO quanta into a 256-frame buffer: at offset 127 the +1 attack
        // latency pushes the onset to frame 128, i.e. the start of the next
        // quantum — so a single 128-frame buffer can't see it. The 0035 harness
        // reports the same (offset 127 -> onset 128).
        for off in [0u8, 1, 7, 31, 63, 100, 127] {
            let mut host = Host::new(48_000.0);
            load(
                &mut host,
                &[Event::NoteOn {
                    offset: off,
                    note: 64,
                    velocity: 1.0,
                }],
            );
            let mut combined = [0.0f32; QUANTUM * 2];
            host.render(1, 0, 60);
            combined[..QUANTUM].copy_from_slice(&host.out_l);
            host.render(0, 0, 60); // tail quantum, no new events
            combined[QUANTUM..].copy_from_slice(&host.out_l);

            let got = onset(&combined).expect("expected audible output");
            assert_eq!(
                got,
                off as usize + 1,
                "note-on at offset {off} produced onset at {got}, want {}",
                off as usize + 1
            );
        }
    }

    #[test]
    fn sliced_host_matches_per_event_reference() {
        // The host's one-call render must produce byte-identical audio to the
        // reference: apply each event then process the matching sub-slice by
        // hand (the literal CLAP loop). Proves the in-wasm loop didn't drift.
        let stream = [
            Event::NoteOn {
                offset: 0,
                note: 60,
                velocity: 0.8,
            },
            Event::SetParam {
                offset: 16,
                id: 2,
                plain: 0.7,
            },
            Event::NoteOn {
                offset: 64,
                note: 67,
                velocity: 0.6,
            },
            Event::NoteOff {
                offset: 100,
                note: 60,
            },
        ];

        let mut host = Host::new(48_000.0);
        load(&mut host, &stream);
        host.render(stream.len() as u32 as usize, 0, 60);

        // Reference: same engine, manual apply-then-slice.
        let mut synth = Synth::new(48_000.0);
        synth.set_key_mode(KeyMode::from_u8(0));
        synth.set_split_point(60);
        let (mut l, mut r) = ([0.0f32; QUANTUM], [0.0f32; QUANTUM]);
        let mut prev = 0usize;
        for ev in &stream {
            let k = (ev.offset() as usize).min(QUANTUM);
            if k > prev {
                synth.process(&mut l[prev..k], &mut r[prev..k]);
                prev = k;
            }
            codec::apply(ev, &mut synth);
        }
        if prev < QUANTUM {
            synth.process(&mut l[prev..QUANTUM], &mut r[prev..QUANTUM]);
        }

        assert_eq!(host.out_l, l, "left output diverges from per-event reference");
        assert_eq!(host.out_r, r, "right output diverges from per-event reference");
    }

    #[test]
    fn param_step_lands_at_offset() {
        // A param change at offset N affects only the audio from N onward:
        // samples [0..N) must be identical to a reference render without the
        // param change (the host slices at the event offset and renders the
        // pre-event region before applying the event).
        //
        // Reference: note-on only, no param change.
        let note_ev = [Event::NoteOn {
            offset: 0,
            note: 60,
            velocity: 1.0,
        }];
        let mut ref_host = Host::new(48_000.0);
        load(&mut ref_host, &note_ev);
        ref_host.render(1, 0, 60);
        let ref_pre: Vec<f32> = ref_host.out_l[..64].to_vec();

        // Sliced render: same note-on plus a param step at offset 64.
        let mut host = Host::new(48_000.0);
        load(
            &mut host,
            &[
                Event::NoteOn {
                    offset: 0,
                    note: 60,
                    velocity: 1.0,
                },
                Event::SetParam {
                    offset: 64,
                    id: 2, // PatchParam::Osc1Fine (Upper layer)
                    plain: 1.0,
                },
            ],
        );
        host.render(2, 0, 60);

        // Pre-offset: the param hasn't been applied yet — identical to reference.
        assert_eq!(
            &host.out_l[..64],
            ref_pre.as_slice(),
            "samples before offset 64 were affected by a param change at 64"
        );
        // Post-offset: rendering continued past offset 64 (tail not dropped).
        assert!(
            onset(&host.out_l).is_some(),
            "no audio rendered at all"
        );
        assert!(
            host.out_l[120] != 0.0 || host.out_r[120] != 0.0,
            "tail after the param-step offset was not rendered"
        );
    }

    #[test]
    fn empty_quantum_renders_full_silence_tail() {
        // No events: the host must still render the whole quantum (the tail path
        // with prev==0), producing exact silence for an idle synth.
        let mut host = Host::new(48_000.0);
        host.render(0, 0, 60);
        assert!(host.out_l.iter().all(|&s| s == 0.0));
        assert!(host.out_r.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn reset_clears_sounding_voices() {
        // A held note then reset must silence the engine — proves the lifecycle
        // reset (resume-after-suspend / re-init) clears voice state.
        let mut host = Host::new(48_000.0);
        load(
            &mut host,
            &[Event::NoteOn {
                offset: 0,
                note: 60,
                velocity: 1.0,
            }],
        );
        host.render(1, 0, 60);
        assert!(onset(&host.out_l).is_some(), "note should sound before reset");
        host.synth.reset();
        host.render(0, 0, 60);
        assert!(
            host.out_l.iter().all(|&s| s == 0.0),
            "reset did not clear the sounding voice"
        );
    }

    #[test]
    fn set_sample_rate_keeps_rendering() {
        // Changing sample rate must leave the host renderable (no panic, audio
        // still produced) — the engine rebuilds its rate-dependent state.
        let mut host = Host::new(48_000.0);
        host.synth.set_sample_rate(44_100.0);
        load(
            &mut host,
            &[Event::NoteOn {
                offset: 0,
                note: 60,
                velocity: 1.0,
            }],
        );
        host.render(1, 0, 60);
        assert!(onset(&host.out_l).is_some(), "no audio after sample-rate change");
    }

    #[test]
    fn key_mode_applied_before_events() {
        // key_mode/split arrive as args (shared state) and are applied before the
        // event loop — render must not panic and must honour the passed mode.
        // Split mode (2) routes notes by split point; just assert a clean render.
        //
        // NOTE (E031/0161, deferred to 0169): this is only a crash guard, not a
        // behavioural assertion. A stronger "Split vs Whole route differently"
        // check is infeasible here — a single Poly-mode note routes identically
        // under both, and the level params that would differ use Glide::Block
        // (don't settle within one 128-sample quantum). Left as-is deliberately.
        let mut host = Host::new(48_000.0);
        load(
            &mut host,
            &[Event::NoteOn {
                offset: 0,
                note: 72,
                velocity: 1.0,
            }],
        );
        host.render(1, 2, 60); // Split at note 60; note 72 is upper
        assert!(onset(&host.out_l).is_some(), "split-mode note produced no audio");
    }
}
