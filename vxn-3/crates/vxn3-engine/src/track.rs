//! A track: one active engine + its pattern + mix settings + swap mailbox
//! (ADR 0001 §4). Eight of these make the instrument.

use std::sync::Arc;

use crate::engines::KickTone;
use crate::lane::Hit;
use crate::sequencer::Pattern;
use crate::swap::EngineSwap;
use crate::track_engine::TrackEngine;

pub struct Track {
    /// The single active engine. Swapped off-thread via [`Track::swap`].
    pub engine: Box<dyn TrackEngine>,
    /// Main↔audio swap mailbox; clone the `Arc` to drive swaps from the main
    /// thread.
    pub swap: Arc<EngineSwap>,
    /// Step grid.
    pub pattern: Pattern,
    /// Linear output gain.
    pub gain: f32,
    /// Stereo pan, -1 (left) .. +1 (right).
    pub pan: f32,
    /// Pre-allocated mono render scratch (sized at construction).
    mono: Vec<f32>,
}

impl Track {
    /// A track defaulting to a `Kick/Tone` engine and an empty pattern.
    pub fn new(sample_rate: f32, max_block: usize) -> Self {
        Self {
            engine: Box::new(KickTone::with_default_patch(sample_rate)),
            swap: EngineSwap::new(),
            pattern: Pattern::default(),
            gain: 1.0,
            pan: 0.0,
            mono: vec![0.0; max_block],
        }
    }

    /// Install a pending off-thread engine swap, if any. Audio-thread,
    /// allocation-free. Returns `true` when a swap happened.
    #[inline]
    pub fn poll_swap(&mut self) -> bool {
        self.swap.try_install(&mut self.engine)
    }

    /// Equal-power pan gains `(left, right)`, scaled by `gain`.
    #[inline]
    pub fn pan_gains(&self) -> (f32, f32) {
        let angle = (self.pan.clamp(-1.0, 1.0) * 0.5 + 0.5) * std::f32::consts::FRAC_PI_2;
        (self.gain * angle.cos(), self.gain * angle.sin())
    }

    /// Render this track for the block into its mono scratch, applying the
    /// pre-scheduled `hits` sample-accurately by slicing the render at each hit's
    /// frame offset. `hits` are frame-ordered and clamped to `[0, frames]` by the
    /// scheduler ([`crate::lane`]). Allocation-free.
    pub fn render_with_hits(&mut self, hits: &[Hit], frames: usize) {
        let frames = frames.min(self.mono.len());
        let engine: &mut dyn TrackEngine = &mut *self.engine;
        let mono = &mut self.mono[..frames];

        let mut pos = 0usize;
        for h in hits {
            let f = h.frame.min(frames);
            if f > pos {
                engine.render(&mut mono[pos..f]);
                pos = f;
            }
            engine.on_trig(h.note, h.velocity);
        }
        engine.render(&mut mono[pos..frames]);
    }

    /// Mix the rendered mono scratch into the stereo bus with gain/pan.
    #[inline]
    pub fn mix_into(&self, out_l: &mut [f32], out_r: &mut [f32], frames: usize) {
        let frames = frames.min(self.mono.len()).min(out_l.len()).min(out_r.len());
        let (gl, gr) = self.pan_gains();
        for f in 0..frames {
            let s = self.mono[f];
            out_l[f] += s * gl;
            out_r[f] += s * gr;
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.engine.set_sample_rate(sample_rate);
    }

    pub fn reset(&mut self) {
        self.engine.reset();
    }
}
