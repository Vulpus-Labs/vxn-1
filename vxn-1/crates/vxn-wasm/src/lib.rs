//! WASM/browser feasibility spike (ticket 0034).
//!
//! Exposes [`vxn_engine::Synth`] over a flat C ABI so a browser
//! `AudioWorkletProcessor` can instantiate the module with a raw
//! `WebAssembly.instantiate` (no wasm-bindgen) and drive it per render
//! quantum. Throwaway — not a product surface.
//!
//! Render quantum in Web Audio is fixed at 128 frames; the per-instance
//! output buffers are sized to match so JS reads straight out of linear
//! memory with no copy on the wasm side.

use vxn_engine::Synth;

/// Web Audio render-quantum size. AudioWorklet always calls `process()`
/// with 128-frame planar buffers.
const QUANTUM: usize = 128;

/// Boxed synth plus its scratch output buffers, owned by wasm linear
/// memory. JS holds the raw pointer as a `u32` handle.
pub struct Instance {
    synth: Synth,
    out_l: [f32; QUANTUM],
    out_r: [f32; QUANTUM],
}

/// Create a synth at `sample_rate`. Returns an opaque handle (pointer)
/// that every other call must pass back. Leaks the box deliberately;
/// `vxn_destroy` reclaims it.
#[unsafe(no_mangle)]
pub extern "C" fn vxn_new(sample_rate: f32) -> *mut Instance {
    let inst = Box::new(Instance {
        synth: Synth::new(sample_rate),
        out_l: [0.0; QUANTUM],
        out_r: [0.0; QUANTUM],
    });
    Box::into_raw(inst)
}

/// # Safety
/// `ptr` must be a handle returned by [`vxn_new`] and not yet destroyed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_destroy(ptr: *mut Instance) {
    if !ptr.is_null() {
        drop(unsafe { Box::from_raw(ptr) });
    }
}

/// # Safety
/// `ptr` must be a valid handle from [`vxn_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_note_on(ptr: *mut Instance, note: u8, velocity: f32) {
    if let Some(inst) = unsafe { ptr.as_mut() } {
        inst.synth.note_on(note, velocity);
    }
}

/// # Safety
/// `ptr` must be a valid handle from [`vxn_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_note_off(ptr: *mut Instance, note: u8) {
    if let Some(inst) = unsafe { ptr.as_mut() } {
        inst.synth.note_off(note);
    }
}

/// Render one 128-frame quantum into the instance's internal buffers.
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_process(ptr: *mut Instance) {
    if let Some(inst) = unsafe { ptr.as_mut() } {
        inst.synth.process(&mut inst.out_l, &mut inst.out_r);
    }
}

// --- ticket 0035: sub-block slicing + mid-block param events ---------------
//
// `Synth::process` renders contiguous slices only; sample-accuracy is owned by
// the host — exactly the CLAP shell's contract (vxn-clap/src/lib.rs:335-369).
// The worklet drains the SAB event ring, applies each event at its sample
// offset, and renders `[prev..k)` between events. These exports give the JS
// host the primitives to do that with no change to the engine.

/// Render the sub-range `[start, end)` of the current quantum into the
/// instance's output buffers. The worklet calls this once per slice between
/// applied events, mirroring `synth.process(&mut l[start..end], ...)` in the
/// CLAP loop. `start`/`end` are clamped to `[0, QUANTUM]`; a non-positive span
/// is a no-op (matches the `if start < end` guard in the plugin).
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_process_slice(ptr: *mut Instance, start: u32, end: u32) {
    if let Some(inst) = unsafe { ptr.as_mut() } {
        let s = (start as usize).min(QUANTUM);
        let e = (end as usize).min(QUANTUM);
        if s < e {
            inst.synth
                .process(&mut inst.out_l[s..e], &mut inst.out_r[s..e]);
        }
    }
}

/// Set a parameter by CLAP id, effective on the next rendered slice. Mirrors
/// the `synth.set_param(idx, value)` the CLAP shell makes inside the event
/// batch, before rendering the slice that follows it.
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_set_param(ptr: *mut Instance, index: u32, value: f32) {
    if let Some(inst) = unsafe { ptr.as_mut() } {
        inst.synth.set_param(index as usize, value);
    }
}

/// Pointer to the left-channel output buffer (`QUANTUM` f32s) in linear
/// memory. JS copies from here after each [`vxn_process`].
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_out_l(ptr: *mut Instance) -> *const f32 {
    match unsafe { ptr.as_ref() } {
        Some(inst) => inst.out_l.as_ptr(),
        None => core::ptr::null(),
    }
}

/// Pointer to the right-channel output buffer (`QUANTUM` f32s).
///
/// # Safety
/// `ptr` must be a valid handle from [`vxn_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vxn_out_r(ptr: *mut Instance) -> *const f32 {
    match unsafe { ptr.as_ref() } {
        Some(inst) => inst.out_r.as_ptr(),
        None => core::ptr::null(),
    }
}

/// Frames-per-quantum, so JS doesn't hard-code the constant.
#[unsafe(no_mangle)]
pub extern "C" fn vxn_quantum() -> u32 {
    QUANTUM as u32
}
