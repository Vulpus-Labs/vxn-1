//! The vxn-3 engine.
//!
//! 0046 skeleton: owns the [`Transport`] clock and renders silence. The track
//! model, the engine-defined voicing trait (ADR 0001 §4/§5), and the three
//! voice engines land in 0047 / 0049.

use crate::transport::Transport;

/// Audio-thread engine state. Its [`process_block`](Self::process_block) is
/// allocation-free.
pub struct Engine {
    sample_rate: f32,
    /// Latest host transport snapshot. The shell refreshes it each block; the
    /// sequencer (0048) will consume it. Stored now so 0046 proves the clock
    /// reaches the engine layer.
    transport: Transport,
}

impl Engine {
    /// Build an engine for the given sample rate. `block_size` is accepted to
    /// match the shell's `activate` contract (and to size scratch state in
    /// later slices); unused while the engine renders silence.
    pub fn new(sample_rate: f32, _block_size: usize) -> Self {
        Self {
            sample_rate,
            transport: Transport::default(),
        }
    }

    /// Re-derive sample-rate-dependent coefficients. No-op until the voice
    /// engines land.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Hand the engine this block's host transport snapshot.
    pub fn set_transport(&mut self, transport: Transport) {
        self.transport = transport;
    }

    /// The transport the sequencer will consume. Read by tests now and the
    /// sequencer in 0048.
    pub fn transport(&self) -> Transport {
        self.transport
    }

    /// Render `left.len()` frames of stereo audio. 0046: silence.
    ///
    /// Allocation-free — writes zeros into caller-owned buffers, never
    /// allocates. Caller guarantees `left.len() == right.len()`.
    pub fn process_block(&mut self, left: &mut [f32], right: &mut [f32]) {
        left.fill(0.0);
        right.fill(0.0);
    }

    /// Drop voices / clear FX tails. No state to clear yet.
    pub fn reset(&mut self) {}
}
