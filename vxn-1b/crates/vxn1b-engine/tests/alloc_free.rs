//! Allocation-free hot-path gate (ticket 0202, acceptance 5).
//!
//! A process-wide counting allocator (this test binary only) tallies heap
//! allocations while **armed**. After warm-up (construction + one block, where
//! setup allocations are fine), the audio hot path — note on/off, pressure,
//! param sets, and `process_block` — must allocate **zero** times: VXN1b's
//! render is fixed-size arrays end to end (bank/eval/render/engine), no `Vec`,
//! `Box`, or growth.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use std::sync::Arc;

use vxn1b_engine::params::global_clap_id;
use vxn1b_engine::{Engine, MeterBus, MeterFrame, ParamId};

struct Counting;

static ARMED: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ARMED.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static A: Counting = Counting;

#[test]
fn hot_path_is_allocation_free() {
    let mut e = Engine::new(48_000.0, 512);
    let mut l = vec![0.0f32; 512];
    let mut r = vec![0.0f32; 512];
    // Allocated before arming — the shell builds its bus once, at plugin
    // construction, never on the audio thread.
    let bus = Arc::new(MeterBus::new());

    // Warm-up (allocations permitted): fill the voices, exercise every op path
    // once so any lazy setup is done before arming.
    e.set_param(ParamId::Env2Attack as usize, 0.002);
    for i in 0..16 {
        e.note_on(0, 48 + i as u8, 1.0);
    }
    e.process_block(&mut l, &mut r);

    // Arm: from here, the audio-thread API must not touch the heap.
    ARMED.store(true, Ordering::Relaxed);

    e.set_param(ParamId::Cutoff as usize, 4000.0);
    e.set_pitch_bend(0.3);
    e.set_mod_wheel(0.5);
    e.channel_pressure(0, 0.7);
    e.poly_pressure(0, 60, 0.4);
    e.note_on(1, 72, 0.8); // steals — still alloc-free
    e.process_block(&mut l, &mut r);
    e.note_off(0, 48);
    e.process_block(&mut l, &mut r);
    // Dual mode with the cross-layer LFO 2 link on (0217): both synths tick and
    // layer 2 slaves to layer 1's phase — still no heap.
    e.set_layer2_on(true);
    e.set_lfo2_link(true);
    e.note_on(2, 64, 0.9);
    e.process_block(&mut l, &mut r);
    // Global drift on (0218): re-cooks both synths' per-lane envelope trims and
    // runs the drifted cutoff/resonance path — all fixed-size, still no heap.
    e.set_param(global_clap_id(ParamId::MasterDrift).unwrap(), 0.8);
    e.process_block(&mut l, &mut r);
    // Meter publish (0240) rides every `process_block` above; assert it is on
    // an adopted (shared) bus too, since that is the CLAP shell's arrangement —
    // an `Arc<MeterBus>` clone must not put the tap on a different code path.
    // `fetch_update`'s closure captures by copy; there is nothing to box.
    e.set_meters(bus.clone());
    e.process_block(&mut l, &mut r);

    ARMED.store(false, Ordering::Relaxed);

    let n = ALLOCS.load(Ordering::Relaxed);
    assert_eq!(n, 0, "audio hot path allocated {n} times");
    // Sanity: the armed blocks really did drive the tap, so a zero count above
    // means "allocation-free", not "never ran".
    assert!(
        !MeterFrame::drain(&bus).is_silent(),
        "the metered blocks must have published something"
    );
}
