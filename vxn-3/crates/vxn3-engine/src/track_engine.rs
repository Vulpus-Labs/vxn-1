//! The `TrackEngine` trait — the load-bearing abstraction (ADR 0001 §4/§5).
//!
//! A track holds **one** active engine behind a `Box<dyn TrackEngine>`. Dispatch
//! is **per block, per track** — one vtable call into [`TrackEngine::render`],
//! which then runs its own monomorphic SoA lane loop with *no* further dispatch
//! (the vxn-1/vxn-2 "no enum match inside the lane loop" lesson). What a lane
//! *means* is the engine's choice: voices for a poly engine, modes for a
//! resonator (0049). Same trait, different voicing — so the resonator slots in
//! without reshaping this surface.
//!
//! Sample accuracy is the *host's* job, not a per-sample parameter here: the
//! instrument [`crate::engine::Engine`] slices each block at trig boundaries and
//! calls `render` on the contiguous sub-spans, with [`TrackEngine::on_trig`]
//! between them. So `render` only ever sees a plain contiguous span and an
//! engine never needs to reason about frame offsets.

/// Lane budget ceiling. A poly engine uses lanes as voices (≤ 4, the agreed
/// poly cap → one NEON `f32x4`); a resonator uses them as modes. Engines store
/// their SoA state in `[_; LANES]` arrays.
pub const LANES: usize = 4;

/// The per-track voice/resonator engine.
///
/// `Send` so a freshly-built engine can be handed from the main thread to the
/// audio thread over the [`crate::swap`] channel.
pub trait TrackEngine: Send {
    /// Render `out.len()` mono samples, **overwriting** the span, advancing
    /// voice/resonator state. Allocation-free.
    fn render(&mut self, out: &mut [f32]);

    /// Trigger the engine. Poly: allocate/steal a voice at `note` (equal-tempered
    /// MIDI, fractional allowed) and `velocity` (0..1). Resonator: inject
    /// excitation into the persistent state. Called by the host between render
    /// sub-spans, so it is sample-accurate.
    fn on_trig(&mut self, note: f32, velocity: f32);

    /// Silence all voices / collapse decaying state (transport stop, reset).
    fn reset(&mut self);

    /// Re-cook sample-rate-dependent coefficients.
    fn set_sample_rate(&mut self, sample_rate: f32);

    /// A short identifier for the active engine kind (UI / introspection / swap
    /// assertions). Stable across instances of the same engine.
    fn kind(&self) -> EngineKind;
}

/// The closed engine roster (ADR 0001 §6). `Metal` / `Noise` land in 0049.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EngineKind {
    KickTone,
    Metal,
    Noise,
}
