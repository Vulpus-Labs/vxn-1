//! Worklet audio-host (0286) — the production render loop.
//!
//! The web analogue of `vxn1b-clap`'s audio-thread half
//! (`VxnAudioProcessor::process`). Where the CLAP host hands `process()` a
//! sample-accurate event list *on the audio thread*, the browser splits
//! controller (main thread) from renderer (worklet). The transport that bridges
//! them is the SAB event ring; this host consumes it on the render side and
//! turns it into audio with the same sample-accurate slicing the plugin gets for
//! free.
//!
//! # Why a Rust host
//!
//! Driving the slice loop from JS would cross the JS↔wasm boundary
//! O(events + slices) times per quantum. This host does it in **one**
//! `vxn1b_host_render` call: JS copies the ring's raw wire bytes into a
//! linear-memory scratch, then Rust decodes ([`crate::codec`]), slices, and
//! renders entirely inside wasm. The engine is unchanged — `Engine::process_block`
//! still renders contiguous slices and chunks them internally by `CONTROL_BLOCK`,
//! exactly the plugin's contract.
//!
//! # Per-quantum loop (mirrors the CLAP batch loop)
//!
//! 1. The worklet folds the param store block-start via [`vxn1b_host_set_param`]
//!    for each id the store reports changed — the `LocalParams` analogue.
//! 2. Slice the block at event sample-offsets: apply every event at offset `k`,
//!    render `[prev..k)`, advance; render the tail.
//!
//! ## Why nothing is hoisted to block start
//!
//! vxn-1's host applies key-mode and split-point once, before ingesting events,
//! because they arrive as `vxn1b_host_render` arguments rather than on the wire.
//! VXN1b's non-automatable state is richer — key mode, split point, LFO 2 link,
//! **and per-slot matrix topology** — and all of it rides the ring, so all of it
//! is applied *at its offset* in the one slice loop.
//!
//! That is deliberate, not incidental. A preset load changes params and topology
//! together; if topology were hoisted to block start it would land ahead of the
//! param writes travelling with it, and a slot would briefly route the new source
//! at the old depth. On a matrix-heavy patch that is audible. Keeping both on the
//! same wire, applied in offset order, makes the pairing exact.

use crate::QUANTUM;
use crate::codec::{self, SLOT_BYTES};
use vxn1b_engine::{Engine, MeterFrame, SCOPE_DECIMATION, SCOPE_WINDOW};
use vxn1b_engine::MeterTap;

/// Max events decoded per quantum. Matches the ring capacity, so a full ring
/// drains in one render.
pub const MAX_EVENTS: usize = 1024;

/// Floats in one drained meter frame — one per tap, in `MeterTap` order.
/// Exported to JS so the telemetry SAB is sized from the engine rather than
/// from a number someone typed into a JS file (see ticket 0285).
pub const METER_LEN: usize = MeterTap::COUNT;

/// The worklet audio-host: an [`Engine`], its stereo output (read straight out
/// of linear memory by JS), and the event-decode scratch JS copies ring bytes
/// into.
pub struct Host {
    engine: Engine,
    out_l: [f32; QUANTUM],
    out_r: [f32; QUANTUM],
    /// Raw 16-byte wire records for the current quantum. JS writes here (via the
    /// pointer from [`vxn1b_host_events_ptr`]) then calls [`vxn1b_host_render`].
    events: [u8; SLOT_BYTES * MAX_EVENTS],
    /// Last drained meter frame, in `MeterTap` order. JS reads it out of linear
    /// memory via [`vxn1b_host_meters_ptr`] after [`vxn1b_host_drain_meters`].
    meters: [f32; METER_LEN],
    /// Last read scope window. A `Vec` rather than an array because
    /// `ScopeBus::read_window` fills one; it is reserved to `SCOPE_WINDOW` up
    /// front so the render path never allocates after construction.
    scope: Vec<f32>,
}

impl Host {
    fn new(sample_rate: f32) -> Self {
        Host {
            // `max_frames` is the quantum: AudioWorklet always calls `process()`
            // with 128-frame buffers, so the engine never sees a longer block.
            engine: Engine::new(sample_rate),
            out_l: [0.0; QUANTUM],
            out_r: [0.0; QUANTUM],
            events: [0u8; SLOT_BYTES * MAX_EVENTS],
            meters: [0.0; METER_LEN],
            scope: Vec::with_capacity(SCOPE_WINDOW),
        }
    }

    /// The render loop proper, factored out so it is unit-testable without the
    /// C-ABI pointer dance. Renders one quantum into `out_l`/`out_r` from the
    /// first `n` records in `events`, slicing at each record's sample offset.
    fn render(&mut self, n: usize) {
        // Disjoint field borrows so decode (reads `events`) and render (writes
        // the output buffers, mutates `engine`) can coexist.
        let Host { engine, out_l, out_r, events, .. } = self;

        let n = n.min(MAX_EVENTS);
        let q = QUANTUM;
        let mut prev = 0usize;
        let mut i = 0usize;

        while i < n {
            let off = (events[i * SLOT_BYTES + 1] as usize).min(q);
            // Render everything strictly before this event's offset.
            if off > prev {
                engine.process_block(&mut out_l[prev..off], &mut out_r[prev..off]);
                prev = off;
            }
            // Apply ALL events at this same offset (one CLAP batch boundary).
            while i < n && (events[i * SLOT_BYTES + 1] as usize).min(q) == off {
                let base = i * SLOT_BYTES;
                codec::decode_and_apply(&events[base..base + SLOT_BYTES], engine);
                i += 1;
            }
        }
        // Render the tail.
        if prev < q {
            engine.process_block(&mut out_l[prev..q], &mut out_r[prev..q]);
        }
    }

    /// Test/bench accessor for the rendered left channel.
    #[cfg(test)]
    fn out_l(&self) -> &[f32; QUANTUM] {
        &self.out_l
    }

    #[cfg(test)]
    fn engine(&self) -> &Engine {
        &self.engine
    }
}

// ── C ABI (raw `WebAssembly.instantiate`, no wasm-bindgen) ──────────────────

/// Create a host at `sample_rate`. Returns an opaque handle (pointer) every
/// other call passes back. Leaks the box; [`vxn1b_host_destroy`] reclaims it.
#[unsafe(no_mangle)]
pub extern "C" fn vxn1b_host_new(sample_rate: f32) -> *mut Host {
    Box::into_raw(Box::new(Host::new(sample_rate)))
}

/// # Safety
/// `ptr` must be a handle from [`vxn1b_host_new`], not yet destroyed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn1b_host_destroy(ptr: *mut Host) {
    if !ptr.is_null() {
        drop(unsafe { Box::from_raw(ptr) });
    }
}

/// Pointer to the event-decode scratch in linear memory. JS copies drained ring
/// records here (`n * 16` bytes, offset-ordered) before [`vxn1b_host_render`].
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn1b_host_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn1b_host_events_ptr(ptr: *mut Host) -> *mut u8 {
    match unsafe { ptr.as_mut() } {
        Some(h) => h.events.as_mut_ptr(),
        None => core::ptr::null_mut(),
    }
}

/// Capacity of the event scratch in records (so JS never overruns it).
#[unsafe(no_mangle)]
pub extern "C" fn vxn1b_host_max_events() -> u32 {
    MAX_EVENTS as u32
}

/// Frames per Web Audio render quantum, so JS sizes its scratch buffers to match
/// the engine instead of hard-coding the constant.
#[unsafe(no_mangle)]
pub extern "C" fn vxn1b_quantum() -> u32 {
    QUANTUM as u32
}

/// Total addressable CLAP ids. The JS side asserts its declared mirror against
/// this at controller-instantiate; ticket 0285 is what happens when that mirror
/// is allowed to rot, so this export is the whole reason the check can exist.
#[unsafe(no_mangle)]
pub extern "C" fn vxn1b_total_params() -> u32 {
    codec::TOTAL_PARAMS as u32
}

/// Set a param by CLAP id. The worklet calls this block-start for each param the
/// store reports changed (the `LocalParams` fold), before [`vxn1b_host_render`].
/// Sample-accurate param automation does NOT use this — it rides the ring as
/// `EV_PARAM` and is applied at its offset.
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn1b_host_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn1b_host_set_param(ptr: *mut Host, index: u32, value: f32) {
    if let Some(h) = unsafe { ptr.as_mut() } {
        h.engine.set_param(index as usize, value);
    }
}

/// Read a param's current PLAIN value by CLAP id. The main-thread coordinator
/// snapshots these off a throwaway host to SEED the param store with the
/// engine's defaults before the worklet starts — otherwise the store's
/// zero-initialised slots would fold zeros over every param on the first quantum
/// and silence the voice.
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn1b_host_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn1b_host_get_param(ptr: *mut Host, index: u32) -> f32 {
    match unsafe { ptr.as_ref() } {
        Some(h) => h.engine.param(index as usize),
        None => 0.0,
    }
}

/// Render one quantum, applying the first `n_events` records in the scratch at
/// their sample offsets. Returns nothing; JS reads the output through
/// [`vxn1b_host_out_l`] / [`vxn1b_host_out_r`].
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn1b_host_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn1b_host_render(ptr: *mut Host, n_events: u32) {
    if let Some(h) = unsafe { ptr.as_mut() } {
        h.render(n_events as usize);
    }
}

/// Pointer to the rendered left channel (`QUANTUM` f32s) in linear memory.
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn1b_host_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn1b_host_out_l(ptr: *mut Host) -> *const f32 {
    match unsafe { ptr.as_ref() } {
        Some(h) => h.out_l.as_ptr(),
        None => core::ptr::null(),
    }
}

/// Pointer to the rendered right channel (`QUANTUM` f32s) in linear memory.
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn1b_host_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn1b_host_out_r(ptr: *mut Host) -> *const f32 {
    match unsafe { ptr.as_ref() } {
        Some(h) => h.out_r.as_ptr(),
        None => core::ptr::null(),
    }
}

/// Drop every sounding voice (panic button / context suspend).
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn1b_host_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn1b_host_reset(ptr: *mut Host) {
    if let Some(h) = unsafe { ptr.as_mut() } {
        h.engine.reset();
    }
}

// ── Telemetry: the audio→view direction (0288) ──────────────────────────────
//
// Natively the meter and scope buses are `Arc`-shared with the ~60 Hz timer, and
// the frames ride the existing `ViewEvent` batch for free. Here the engine is a
// separate wasm with its own linear memory, so the worklet has to read the
// frames out and publish them into a return SAB. These exports are that read.
//
// Both fill a host-owned buffer and return how much is valid; JS then copies
// from the buffer pointer into the SAB under a seqlock. Nothing here converts
// units or applies ballistics — meter values stay linear peak magnitudes,
// because the dB mapping and the decay belong to the view.

/// Floats in a meter frame. JS sizes its SAB region from this rather than from a
/// literal, so adding a tap does not silently truncate the frame.
#[unsafe(no_mangle)]
pub extern "C" fn vxn1b_meter_len() -> u32 {
    METER_LEN as u32
}

/// Samples in a scope window.
#[unsafe(no_mangle)]
pub extern "C" fn vxn1b_scope_window() -> u32 {
    SCOPE_WINDOW as u32
}

/// Drain every meter tap into the host's frame buffer, CLEARING the bus.
///
/// Read-and-clear is the contract: the frame reports the extreme since the
/// previous drain. The caller therefore controls the measurement window by how
/// often it calls this — the worklet divides down to ~60 Hz so each frame covers
/// the span the UI is about to show. Draining every quantum would instead report
/// only the newest quantum's peak and discard the rest unseen.
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn1b_host_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn1b_host_drain_meters(ptr: *mut Host) {
    if let Some(h) = unsafe { ptr.as_mut() } {
        let f = MeterFrame::drain(h.engine.meters());
        h.meters = [
            f.layer1.0,
            f.layer1.1,
            f.layer2.0,
            f.layer2.1,
            f.dynamics_in.0,
            f.dynamics_in.1,
            f.dynamics_out.0,
            f.dynamics_out.1,
            f.dynamics_gr,
            f.master.0,
            f.master.1,
        ];
    }
}

/// Pointer to the drained meter frame (`vxn1b_meter_len()` f32s) in linear
/// memory.
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn1b_host_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn1b_host_meters_ptr(ptr: *mut Host) -> *const f32 {
    match unsafe { ptr.as_ref() } {
        Some(h) => h.meters.as_ptr(),
        None => core::ptr::null(),
    }
}

/// Read the latest scope window into the host's buffer. Returns the sample
/// count, or 0 when the ring holds less than a full window — freshly cleared, or
/// the tap is Off.
///
/// Unlike the meter drain this does NOT clear: the ring is a moving window, so
/// two reads without an intervening block legitimately overlap, and nothing
/// downstream needs frames to be disjoint.
///
/// Goes to `ScopeBus::read_window` directly rather than through
/// `ScopeFrame::read`, which allocates a fresh `Vec` per call. The host's buffer
/// is reserved to `SCOPE_WINDOW` at construction, so in steady state this path
/// allocates nothing — it runs on the render thread.
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn1b_host_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn1b_host_read_scope(ptr: *mut Host) -> u32 {
    match unsafe { ptr.as_mut() } {
        Some(h) => {
            let Host { engine, scope, .. } = h;
            if engine.scope().read_window(SCOPE_DECIMATION, SCOPE_WINDOW, scope) {
                scope.len() as u32
            } else {
                0
            }
        }
        None => 0,
    }
}

/// Pointer to the last read scope window in linear memory. Valid for the count
/// [`vxn1b_host_read_scope`] returned.
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn1b_host_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn1b_host_scope_ptr(ptr: *mut Host) -> *const f32 {
    match unsafe { ptr.as_ref() } {
        Some(h) => h.scope.as_ptr(),
        None => core::ptr::null(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{Event, encode};
    use vxn1b_engine::params::{Layer, ParamId, clap_id_of};

    fn l1(p: ParamId) -> usize {
        clap_id_of(Layer::L1, p)
    }

    /// Write `events` into the host's scratch in the order given (the ring
    /// guarantees offset order, so tests supply them that way too).
    fn load(host: &mut Host, events: &[Event]) -> usize {
        for (i, ev) in events.iter().enumerate() {
            let base = i * SLOT_BYTES;
            host.events[base..base + SLOT_BYTES].copy_from_slice(&encode(ev));
        }
        events.len()
    }

    /// A patch that responds within a quantum, so the slicing assertions below
    /// are about the render loop rather than about envelope timing.
    fn snappy_host() -> Host {
        let mut h = Host::new(48_000.0);
        h.engine.set_param(l1(ParamId::Env2Attack), 0.0);
        h.engine.set_param(l1(ParamId::Env2Sustain), 1.0);
        h.engine.set_param(l1(ParamId::Env2Release), 0.0);
        h
    }

    fn peak(buf: &[f32]) -> f32 {
        buf.iter().fold(0.0f32, |m, &x| m.max(x.abs()))
    }

    #[test]
    fn an_idle_host_renders_exact_silence() {
        let mut h = snappy_host();
        h.render(0);
        assert_eq!(peak(h.out_l()), 0.0, "no note, no output");
    }

    #[test]
    fn a_note_on_makes_sound() {
        let mut h = snappy_host();
        let n = load(&mut h, &[Event::NoteOn { offset: 0, channel: 0, note: 60, velocity: 1.0 }]);
        h.render(n);
        assert!(peak(h.out_l()) > 0.0, "a note-on must be audible");
    }

    /// The point of doing the slice loop in Rust: an event at offset `k` must
    /// take effect at sample `k`, not at the top of the quantum. Everything
    /// before `k` is rendered by the pre-event engine state, which here means
    /// exact zero — the engine's silent-skip path drives idle buffers to 0.0.
    #[test]
    fn an_event_applies_at_its_sample_offset_not_at_block_start() {
        const K: usize = 64;
        let mut h = snappy_host();
        let n = load(&mut h, &[Event::NoteOn { offset: K as u8, channel: 0, note: 60, velocity: 1.0 }]);
        h.render(n);

        let out = h.out_l();
        assert_eq!(peak(&out[..K]), 0.0, "silent before the note-on offset");
        assert!(peak(&out[K..]) > 0.0, "sounding after it");
    }

    /// Several events sharing an offset are one batch boundary — all applied
    /// before the next slice renders, in the order the ring delivered them.
    #[test]
    fn events_at_the_same_offset_apply_as_one_batch() {
        const K: usize = 32;
        let mut h = snappy_host();
        let n = load(
            &mut h,
            &[
                Event::SetParam { offset: K as u8, id: l1(ParamId::Cutoff) as u16, plain: 12_000.0 },
                Event::NoteOn { offset: K as u8, channel: 0, note: 60, velocity: 1.0 },
            ],
        );
        h.render(n);
        assert_eq!(peak(&h.out_l()[..K]), 0.0);
        assert!(peak(&h.out_l()[K..]) > 0.0);
        assert!((h.engine().param(l1(ParamId::Cutoff)) - 12_000.0).abs() < 1.0);
    }

    /// MPE: the channel threads all the way through. A note-off on a *different*
    /// channel must not release the voice — that is the whole reason the channel
    /// is on the wire, and it is what distinguishes VXN1b's dispatch from
    /// vxn-1's channel-agnostic one.
    #[test]
    fn a_note_off_on_the_wrong_channel_does_not_release_the_voice() {
        let mut h = snappy_host();
        let n = load(&mut h, &[Event::NoteOn { offset: 0, channel: 3, note: 60, velocity: 1.0 }]);
        h.render(n);
        h.render(0);
        let held = peak(h.out_l());
        assert!(held > 0.0, "note must be sounding");

        // Wrong channel: nothing happens, the voice keeps sustaining.
        let n = load(&mut h, &[Event::NoteOff { offset: 0, channel: 5, note: 60 }]);
        h.render(n);
        h.render(0);
        assert!(peak(h.out_l()) > 0.0, "a channel-5 note-off must not release a channel-3 voice");

        // Right channel: released, and with a zero release the tail dies fast.
        let n = load(&mut h, &[Event::NoteOff { offset: 0, channel: 3, note: 60 }]);
        h.render(n);
        for _ in 0..8 {
            h.render(0);
        }
        assert!(
            peak(h.out_l()) < held * 0.01,
            "a channel-3 note-off must release the channel-3 voice"
        );
    }

    /// Unknown tags are skipped without disturbing the slice loop: the record
    /// still occupies its offset, it just applies nothing.
    #[test]
    fn an_unknown_tag_in_the_stream_is_skipped_without_breaking_slicing() {
        let mut h = snappy_host();
        let n = load(&mut h, &[Event::NoteOn { offset: 0, channel: 0, note: 60, velocity: 1.0 }]);
        // Overwrite the tag with something this build doesn't know.
        h.events[0] = 200;
        h.render(n);
        assert_eq!(peak(h.out_l()), 0.0, "an undecodable record must apply nothing");
    }

    /// Offsets beyond the quantum clamp to its end rather than panicking on an
    /// out-of-range slice — a malformed producer must not take the audio thread
    /// down.
    #[test]
    fn an_out_of_range_offset_clamps_to_the_end_of_the_quantum() {
        let mut h = snappy_host();
        let n = load(&mut h, &[Event::NoteOn { offset: 255, channel: 0, note: 60, velocity: 1.0 }]);
        h.render(n);
        assert_eq!(peak(h.out_l()), 0.0, "applied at the very end, so this quantum is silent");
        h.render(0);
        assert!(peak(h.out_l()) > 0.0, "and audible from the next one");
    }

    // ── Telemetry (0288) ────────────────────────────────────────────────

    #[test]
    fn draining_meters_reports_the_master_peak_and_then_clears() {
        let mut h = snappy_host();
        let n = load(&mut h, &[Event::NoteOn { offset: 0, channel: 0, note: 60, velocity: 1.0 }]);
        h.render(n);

        unsafe { vxn1b_host_drain_meters(&mut h) };
        let master_l = h.meters[9];
        assert!(master_l > 0.0, "a sounding note must register on the master tap");

        // Read-and-clear: a second drain with nothing rendered in between
        // reports rest, which is what lets the view's decay start falling.
        unsafe { vxn1b_host_drain_meters(&mut h) };
        assert_eq!(h.meters[9], 0.0, "the drain must clear the bus");
    }

    #[test]
    fn the_meter_frame_is_one_float_per_tap() {
        assert_eq!(METER_LEN, vxn1b_meter_len() as usize);
        assert_eq!(METER_LEN, vxn1b_engine::MeterTap::COUNT);
    }

    /// The scope ring only yields a window once it holds one: a fresh host has
    /// captured nothing, so the reader must be told "no frame" rather than
    /// handed a half-full buffer that would draw as a truncated trace.
    #[test]
    fn a_cold_scope_ring_yields_no_window() {
        let mut h = snappy_host();
        assert_eq!(unsafe { vxn1b_host_read_scope(&mut h) }, 0);
    }

    #[test]
    fn the_scope_captures_the_selected_tap_and_nothing_while_off() {
        let quanta_for_a_window = (SCOPE_DECIMATION * SCOPE_WINDOW).div_ceil(QUANTUM) + 2;

        // Tap off (the default): render plenty, still no window.
        let mut off = snappy_host();
        let n = load(&mut off, &[Event::NoteOn { offset: 0, channel: 0, note: 60, velocity: 1.0 }]);
        off.render(n);
        for _ in 0..quanta_for_a_window {
            off.render(0);
        }
        assert_eq!(
            unsafe { vxn1b_host_read_scope(&mut off) },
            0,
            "an unselected ring must stay empty — the audio thread only captures what is watched"
        );

        // Tap on Layer 1: a full window arrives, and it is not flat.
        let mut on = snappy_host();
        let n = load(
            &mut on,
            &[
                Event::ScopeTapEv { offset: 0, tap: 1 },
                Event::NoteOn { offset: 0, channel: 0, note: 60, velocity: 1.0 },
            ],
        );
        on.render(n);
        for _ in 0..quanta_for_a_window {
            on.render(0);
        }
        let count = unsafe { vxn1b_host_read_scope(&mut on) };
        assert_eq!(count as usize, SCOPE_WINDOW, "a full window or nothing");
        assert!(
            on.scope.iter().any(|&s| s != 0.0),
            "the captured window must not be flat while a note is sounding"
        );
    }

    /// `ScopeFrame::read` allocates a fresh Vec per call, which is why this path
    /// goes to `read_window` with a host-owned buffer instead. The buffer is
    /// reserved at construction, so the render thread never reallocates.
    #[test]
    fn repeated_scope_reads_do_not_reallocate() {
        let mut h = snappy_host();
        let n = load(
            &mut h,
            &[
                Event::ScopeTapEv { offset: 0, tap: 1 },
                Event::NoteOn { offset: 0, channel: 0, note: 60, velocity: 1.0 },
            ],
        );
        h.render(n);
        for _ in 0..((SCOPE_DECIMATION * SCOPE_WINDOW).div_ceil(QUANTUM) + 2) {
            h.render(0);
        }

        assert!(h.scope.capacity() >= SCOPE_WINDOW, "reserved up front");
        let cap = h.scope.capacity();
        for _ in 0..16 {
            assert_eq!(unsafe { vxn1b_host_read_scope(&mut h) } as usize, SCOPE_WINDOW);
            assert_eq!(h.scope.capacity(), cap, "steady state must not grow the buffer");
        }
    }

    #[test]
    fn more_events_than_the_scratch_holds_are_bounded_not_overrun() {
        let mut h = snappy_host();
        h.render(MAX_EVENTS as usize * 4);
    }
}
