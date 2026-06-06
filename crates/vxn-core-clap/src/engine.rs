//! Engine traits the CLAP shell programs against.
//!
//! Each synth's engine implements these against its own DSP. The clack
//! `Plugin` impl on the synth side bridges between host events and
//! these methods; the helpers in this crate ([`crate::dispatch_event`])
//! call them directly.

/// Block render surface.
pub trait EngineProcess {
    /// Render `left.len()` frames of stereo audio.
    /// Caller guarantees `left.len() == right.len()`.
    fn process_block(&mut self, left: &mut [f32], right: &mut [f32]);

    /// Drop any sustained voices, clear delay / reverb feedback state.
    fn reset(&mut self);

    /// Re-derive sample-rate-dependent coefficients.
    fn set_sample_rate(&mut self, sample_rate: f32);

    /// Hint the maximum block size the engine will see. Used to size
    /// scratch buffers / smoothers; not a hard limit.
    fn set_block_size(&mut self, _samples: usize) {}

    /// Update host BPM for tempo-synced LFOs / delays. Default impl is
    /// a no-op for engines that don't sync to host transport.
    fn set_tempo(&mut self, _bpm: f32) {}
}

/// MIDI / CLAP-note surface.
pub trait EngineNotes {
    fn note_on(&mut self, key: u8, velocity: f32);
    fn note_off(&mut self, key: u8);
    fn pitch_bend(&mut self, value: f32);
    fn mod_wheel(&mut self, value: f32);
    fn aftertouch(&mut self, value: f32);
}

/// Lock-free shared parameter store. The audio thread reads through
/// `get`; the main thread (host automation flush, UI-driven controller)
/// writes through `set`. Concrete impls back this with `[AtomicU32; N]`
/// or similar.
pub trait SharedStore: Send + Sync {
    fn get(&self, id: usize) -> f32;
    fn set(&self, id: usize, value: f32);
}
