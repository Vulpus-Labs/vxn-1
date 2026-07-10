//! Worklet audio-host (ticket 0153) — the production render loop.
//!
//! This is the web analogue of `vxn2-clap`'s audio-thread half
//! (`VxnAudioProcessor::process`). Where the CLAP host hands `process()` a
//! sample-accurate event list *on the audio thread*, the browser splits
//! controller (main thread) from renderer (worklet). The transport that bridges
//! them is the SAB event ring (ticket 0155); this host consumes it on the render
//! side and turns it into audio with the same event-sliced + control-block
//! chunked shape the plugin renders.
//!
//! # Why a Rust host (vs a JS slice loop)
//!
//! JS copies the ring's raw wire bytes into a linear-memory scratch, then Rust
//! decodes ([`crate::codec`]), slices, folds params and renders entirely inside
//! wasm in **one** [`vxn_host_render`] call — the JS↔wasm boundary is crossed
//! once per quantum, not once per event.
//!
//! # Per-quantum loop (mirrors the `vxn2-clap` process loop)
//!
//! 1. **Fold params once, at block start.** vxn-2 has no per-id engine setter;
//!    param edits live in the atomic [`SharedParams`] store (written by
//!    [`vxn_host_set_param`] block-start and by `EV_PARAM` records), and
//!    [`Engine::snapshot_params`] folds the whole store into the engine +
//!    refreshes FX / matrix. This is the `local.write_to → apply_block_params`
//!    step of the plugin loop. A mid-quantum `EV_PARAM` therefore lands next
//!    quantum — the plugin has the same one-block param latency.
//! 2. **Slice at event sample-offsets.** Apply every event at offset `k`
//!    (notes / MIDI act on the engine immediately), render `[prev..k)`, advance,
//!    repeat; render the tail. Records arrive offset-ordered (single SPSC
//!    producer).
//! 3. **Sub-chunk each region to `CONTROL_BLOCK`.** `Engine::process_block`
//!    samples block-rate state (LFOs, matrix) once per call and requires
//!    `len <= CONTROL_BLOCK`, so each sliced region is rendered in
//!    ≤`CONTROL_BLOCK`-frame pieces — the plugin's `control_chunks`.

use crate::codec::{self, SLOT_BYTES};
use crate::{CONTROL_BLOCK, QUANTUM};
use vxn2_engine::engine::Engine;
use vxn2_engine::shared::SharedParams;

/// Max events decoded per quantum. Matches the ring capacity (ticket 0155), so a
/// full ring drains in one render.
pub const MAX_EVENTS: usize = 1024;

/// The worklet audio-host: an [`Engine`], the [`SharedParams`] store it folds
/// each block, its stereo output (read straight out of linear memory by JS), and
/// the event-decode scratch JS copies ring bytes into.
pub struct Host {
    engine: Engine,
    /// Block-start param fold source. `vxn_host_set_param` and `EV_PARAM` write
    /// here; [`Engine::snapshot_params`] reads it at the top of each render.
    /// Owned (not `Arc`) — the worklet render thread is the sole accessor, so
    /// there is no cross-thread sharing to arbitrate (unlike the plugin).
    shared: SharedParams,
    sample_rate: f32,
    out_l: [f32; QUANTUM],
    out_r: [f32; QUANTUM],
    /// Raw 16-byte wire records for the current quantum. JS writes here (via the
    /// pointer from [`vxn_host_events_ptr`]) then calls [`vxn_host_render`].
    events: [u8; SLOT_BYTES * MAX_EVENTS],
}

/// Render `l`/`r` in ≤`CONTROL_BLOCK`-frame pieces — the engine samples its
/// block-rate state once per `process_block` and asserts `len <= CONTROL_BLOCK`.
/// Chunk boundaries are relative to the slice start, matching the plugin's
/// per-batch `control_chunks`.
#[inline]
fn render_chunked(engine: &mut Engine, l: &mut [f32], r: &mut [f32]) {
    let len = l.len();
    let mut a = 0usize;
    while a < len {
        let b = (a + CONTROL_BLOCK).min(len);
        engine.process_block(&mut l[a..b], &mut r[a..b]);
        a = b;
    }
}

impl Host {
    fn new(sample_rate: f32) -> Self {
        Host {
            engine: Engine::new(sample_rate, CONTROL_BLOCK),
            shared: SharedParams::new(),
            sample_rate,
            out_l: [0.0; QUANTUM],
            out_r: [0.0; QUANTUM],
            events: [0u8; SLOT_BYTES * MAX_EVENTS],
        }
    }

    /// The render loop proper, factored out so it is unit-testable without the
    /// C-ABI pointer dance. Renders one quantum into `out_l`/`out_r` from the
    /// first `n` records in `events`, slicing at each record's sample offset.
    fn render(&mut self, n: usize) {
        // (1) Fold the whole param store into the engine, once, before events.
        self.engine.snapshot_params(&self.shared);

        // Disjoint field borrows so decode (reads `events`, writes `shared`) and
        // render (writes the output buffers, mutates `engine`) can coexist.
        let Host {
            engine,
            shared,
            out_l,
            out_r,
            events,
            ..
        } = self;

        let n = n.min(MAX_EVENTS);
        let q = QUANTUM;
        let mut prev = 0usize;
        let mut i = 0usize;

        // (2) The plugin batch loop, ported.
        while i < n {
            let off = (events[i * SLOT_BYTES + 1] as usize).min(q);
            // Render everything strictly before this event's offset.
            if off > prev {
                render_chunked(engine, &mut out_l[prev..off], &mut out_r[prev..off]);
                prev = off;
            }
            // Apply ALL events at this same offset (one batch boundary).
            while i < n && (events[i * SLOT_BYTES + 1] as usize).min(q) == off {
                let base = i * SLOT_BYTES;
                codec::decode_and_apply(&events[base..base + SLOT_BYTES], engine, shared);
                i += 1;
            }
        }
        // Render the tail.
        if prev < q {
            render_chunked(engine, &mut out_l[prev..q], &mut out_r[prev..q]);
        }
    }
}

// ── C ABI (raw `WebAssembly.instantiate`, no wasm-bindgen) ───────────────────

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

/// Frames per Web Audio render quantum, so JS sizes its scratch buffers to match
/// the engine instead of hard-coding the constant.
#[unsafe(no_mangle)]
pub extern "C" fn vxn_quantum() -> u32 {
    QUANTUM as u32
}

/// Set a param by CLAP id (plain value) into the store. The worklet calls this
/// block-start for each param the store reports changed (the block-fold path),
/// before [`vxn_host_render`], which folds the store into the engine. Sample-
/// accurate automation instead rides the ring as `EV_PARAM`.
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn_host_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_host_set_param(ptr: *mut Host, index: u32, value: f32) {
    if let Some(h) = unsafe { ptr.as_mut() } {
        h.shared.set(index as usize, value);
    }
}

/// Set a param by CLAP id from a normalised `[0, 1]` value (taper-aware).
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn_host_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_host_set_param_norm(ptr: *mut Host, index: u32, norm: f32) {
    if let Some(h) = unsafe { ptr.as_mut() } {
        h.shared.set_normalised(index as usize, norm);
    }
}

/// Read a param's current PLAIN value by CLAP id from the store. `SharedParams`
/// is seeded with the default patch at construction, so this returns real
/// defaults immediately — the main-thread coordinator can snapshot all ids off a
/// throwaway host to seed its param store (ticket 0156) without a render.
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn_host_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_host_get_param(ptr: *mut Host, index: u32) -> f32 {
    match unsafe { ptr.as_ref() } {
        Some(h) => h.shared.get(index as usize),
        None => 0.0,
    }
}

/// Rebuild the engine at a new sample rate (context sample-rate change). The
/// AudioWorklet `sampleRate` is fixed per context, so in practice this is hit by
/// a context teardown/rebuild or offline render; wired for completeness. The
/// engine has no in-place `set_sample_rate`, so it is rebuilt and re-folded from
/// the (unchanged) param store.
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn_host_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_host_set_sample_rate(ptr: *mut Host, sample_rate: f32) {
    if let Some(h) = unsafe { ptr.as_mut() } {
        h.sample_rate = sample_rate;
        h.engine = Engine::new(sample_rate, CONTROL_BLOCK);
        h.engine.snapshot_params(&h.shared);
    }
}

/// All-notes-off / clear voice + FX-tail state without freeing the host or
/// touching the store. The worklet calls this on resume-after-suspend to avoid
/// stuck notes, and on re-init recovery.
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn_host_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_host_reset(ptr: *mut Host) {
    if let Some(h) = unsafe { ptr.as_mut() } {
        h.engine.reset();
    }
}

/// **Test-only**: force a wasm trap (Rust panic → `unreachable` on wasm →
/// `WebAssembly.RuntimeError` in JS). Exists so the trap-safety harness (ticket
/// 0156) can prove the worklet boundary catches a render-thread trap and
/// recovers. Never called in production.
#[unsafe(no_mangle)]
pub extern "C" fn vxn_host_force_trap() {
    panic!("forced trap (0156 trap-safety test)");
}

/// Render one quantum: fold params, then slice the block at the offsets of the
/// first `n_events` records in the scratch and render each slice. Output lands
/// in the buffers exposed by [`vxn_host_out_l`] / [`vxn_host_out_r`].
///
/// Unlike vxn-1's host this takes no key-mode/split args — the vxn-2 FM engine
/// has no dual/split layer.
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn_host_new`]; the scratch must hold at
/// least `n_events` valid 16-byte records (written via [`vxn_host_events_ptr`]).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_host_render(ptr: *mut Host, n_events: u32) {
    if let Some(h) = unsafe { ptr.as_mut() } {
        h.render(n_events as usize);
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

    /// First frame whose magnitude clears a small threshold on either channel.
    /// vxn-2's FX bus means "silence" isn't guaranteed to be exact 0.0, so this
    /// is threshold-based rather than the exact-zero probe vxn-1 could use.
    fn onset(l: &[f32], r: &[f32]) -> Option<usize> {
        l.iter()
            .zip(r)
            .position(|(&a, &b)| a.abs() > 1e-6 || b.abs() > 1e-6)
    }

    /// Write `events` into a host's scratch as raw wire bytes, mirroring what JS
    /// copies out of the ring.
    fn load(host: &mut Host, events: &[Event]) {
        for (i, ev) in events.iter().enumerate() {
            host.events[i * SLOT_BYTES..(i + 1) * SLOT_BYTES].copy_from_slice(&encode(ev));
        }
    }

    /// Reference render: the literal fold-then-slice-then-chunk loop, applying
    /// typed events (not bytes). The host's one-call render must match this
    /// byte-for-byte — proving the in-wasm loop didn't drift.
    fn reference(sr: f32, events: &[Event]) -> ([f32; QUANTUM], [f32; QUANTUM]) {
        let mut engine = Engine::new(sr, CONTROL_BLOCK);
        let shared = SharedParams::new();
        engine.snapshot_params(&shared);
        let (mut l, mut r) = ([0.0f32; QUANTUM], [0.0f32; QUANTUM]);
        let mut prev = 0usize;
        let mut i = 0usize;
        let n = events.len();
        while i < n {
            let off = (events[i].offset() as usize).min(QUANTUM);
            if off > prev {
                render_chunked(&mut engine, &mut l[prev..off], &mut r[prev..off]);
                prev = off;
            }
            while i < n && (events[i].offset() as usize).min(QUANTUM) == off {
                codec::apply(&events[i], &mut engine, &shared);
                i += 1;
            }
        }
        if prev < QUANTUM {
            render_chunked(&mut engine, &mut l[prev..QUANTUM], &mut r[prev..QUANTUM]);
        }
        (l, r)
    }

    #[test]
    fn note_on_produces_audio() {
        // A note-on must produce audible output within a couple of quanta (the
        // FM attack + any lookahead settles well inside that).
        let mut host = Host::new(48_000.0);
        load(
            &mut host,
            &[Event::NoteOn { offset: 0, note: 64, velocity: 1.0 }],
        );
        let mut any = false;
        for q in 0..4 {
            host.render(if q == 0 { 1 } else { 0 });
            if onset(&host.out_l, &host.out_r).is_some() {
                any = true;
                break;
            }
        }
        assert!(any, "note-on produced no audible output");
    }

    #[test]
    fn sliced_host_matches_per_event_reference() {
        // The host's one-call render must produce byte-identical audio to the
        // fold-then-slice-then-chunk reference over a mixed event stream.
        let stream = [
            Event::NoteOn { offset: 0, note: 60, velocity: 0.8 },
            Event::SetParam { offset: 16, id: 2, plain: 0.7 },
            Event::NoteOn { offset: 64, note: 67, velocity: 0.6 },
            Event::NoteOff { offset: 100, note: 60 },
        ];
        let mut host = Host::new(48_000.0);
        load(&mut host, &stream);
        host.render(stream.len());

        let (l, r) = reference(48_000.0, &stream);
        assert_eq!(host.out_l, l, "left output diverges from per-event reference");
        assert_eq!(host.out_r, r, "right output diverges from per-event reference");
    }

    #[test]
    fn note_offset_delays_onset() {
        // A note-on at a later offset must not sound before that offset: the
        // region before it is byte-identical to an all-silent (no-event) render.
        let mut host = Host::new(48_000.0);
        load(
            &mut host,
            &[Event::NoteOn { offset: 96, note: 60, velocity: 1.0 }],
        );
        host.render(1);
        let (silent_l, silent_r) = reference(48_000.0, &[]);
        assert_eq!(
            &host.out_l[..96],
            &silent_l[..96],
            "audio appeared before the note's offset"
        );
        assert_eq!(&host.out_r[..96], &silent_r[..96]);
    }

    #[test]
    fn empty_quantum_renders_finite() {
        // No events: the host still renders the whole quantum (tail path with
        // prev==0), producing finite output for an idle engine.
        let mut host = Host::new(48_000.0);
        host.render(0);
        assert!(host.out_l.iter().all(|s| s.is_finite()));
        assert!(host.out_r.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn reset_clears_sounding_voices() {
        // A held note then reset must silence the engine's voices.
        let mut host = Host::new(48_000.0);
        load(
            &mut host,
            &[Event::NoteOn { offset: 0, note: 60, velocity: 1.0 }],
        );
        for _ in 0..3 {
            host.render(1);
        }
        assert!(
            onset(&host.out_l, &host.out_r).is_some(),
            "note should sound before reset"
        );
        host.engine.reset();
        // A couple of quanta for any FX tail to drain to silence.
        host.render(0);
        host.render(0);
        assert!(
            onset(&host.out_l, &host.out_r).is_none(),
            "reset did not clear the sounding voice"
        );
    }

    #[test]
    fn set_sample_rate_keeps_rendering() {
        let mut host = Host::new(48_000.0);
        unsafe { vxn_host_set_sample_rate(&mut host as *mut Host, 44_100.0) };
        assert_eq!(host.sample_rate, 44_100.0);
        load(
            &mut host,
            &[Event::NoteOn { offset: 0, note: 60, velocity: 1.0 }],
        );
        let mut any = false;
        for q in 0..4 {
            host.render(if q == 0 { 1 } else { 0 });
            if onset(&host.out_l, &host.out_r).is_some() {
                any = true;
                break;
            }
        }
        assert!(any, "no audio after sample-rate change");
    }

    #[test]
    fn param_set_folds_into_audio() {
        // A store write via `vxn_host_set_param` must reach the audio: drop
        // master volume to its floor on one host, leave it at default on
        // another, hold a note on both, and assert the low-volume host renders a
        // strictly smaller peak once the gain glide has settled.
        let vol_id = vxn2_engine::params::id_of("master-volume").expect("master-volume id") as u32;
        let ev = [Event::NoteOn { offset: 0, note: 60, velocity: 1.0 }];

        let peak = |host: &Host| {
            host.out_l
                .iter()
                .chain(&host.out_r)
                .fold(0.0f32, |m, s| m.max(s.abs()))
        };

        let mut quiet = Host::new(48_000.0);
        unsafe { vxn_host_set_param(&mut quiet as *mut Host, vol_id, -60.0) };
        load(&mut quiet, &ev);

        let mut loud = Host::new(48_000.0);
        load(&mut loud, &ev);

        // Render enough quanta for the master-gain smoother to settle.
        for q in 0..24 {
            let n = if q == 0 { 1 } else { 0 };
            quiet.render(n);
            loud.render(n);
        }

        assert!(
            peak(&quiet) < peak(&loud),
            "master-volume store write did not fold into the engine: quiet peak {} !< loud peak {}",
            peak(&quiet),
            peak(&loud)
        );
    }
}
