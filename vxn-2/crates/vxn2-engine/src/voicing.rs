//! Voicing modes (ticket 0009): Whole / Layer / Split.
//!
//! ADR §8 — a patch is one of three voicing modes:
//!
//! - **Whole** — one parameter set ([`Patch::upper`]) drives every voice.
//!   [`Patch::lower`] is ignored.
//! - **Layer** — both Upper and Lower parameter sets are triggered by every
//!   note, summed into the FX chain. Polyphony is effectively halved (each
//!   note consumes two stacks).
//! - **Split** — both parameter sets exist; each note is dispatched to one
//!   layer by [`VoicingParams::split_point`]. Notes ≥ split go to Upper;
//!   notes below go to Lower. Polyphony per note is unchanged.
//!
//! Two-layer infrastructure already exists (`PatchMatrix` in
//! [`crate::matrix`]). This module adds the **dispatch** — what gets
//! allocated, and on which layer — driven by [`PolyAlloc::note_on_patch`].
//!
//! ## Where the layer tag lives
//!
//! The allocator owns a `[Layer; N_STACKS]` array (`PolyAlloc::layers`). The
//! DSP [`vxn2_dsp::stack::Stack`] is layer-agnostic — it just renders. The
//! engine's matrix-evaluation step looks up `alloc.stack_layer(i)` to choose
//! the right [`crate::matrix::MatrixTable`] per stack.
//!
//! ## Voice cap across layers
//!
//! The 16-stack cap applies to the *total* across both layers. In Layer mode
//! each note consumes two stacks, so simultaneous polyphony halves to 8.
//! No extra logic needed — the allocator picks slots one at a time, and the
//! second Layer-mode allocation steals if necessary.
//!
//! ## Mode change during playback
//!
//! Existing voices play out; new note-ons honour the new mode. Stacks
//! captured their layer tag at note-on, so the matrix continues routing each
//! held stack through its original layer's table until its release tail
//! decays.
//!
//! ## Solo × Layer
//!
//! [`crate::alloc::AssignMode::Solo`] applies only in Whole voicing in v1.
//! [`PolyAlloc::note_on_patch`] dispatches Layer/Split allocations through
//! the Poly path regardless of `params.assign_mode` — Solo + Layer would
//! contend over the single `SOLO_SLOT`, and the UI gates the combination.
//!
//! ## Per-layer matrix slots
//!
//! [`crate::matrix::PatchMatrix`] already carries one [`MatrixTable`] per
//! layer (Upper / Lower). The engine's render step picks the right table by
//! consulting `alloc.stack_layer(i)`. See ticket 0008 for the matrix surface.
//!
//! ## FX chain
//!
//! FX (delay, reverb) are patch-level, not per-layer — both layers feed the
//! same FX chain at the mix bus. This module doesn't touch FX state; it just
//! ensures both layers' carrier outputs reach the same summation point.
//! Tickets 0010 / 0011 / 0012 own the FX wiring.

use vxn2_dsp::stack::StackParams;
use vxn2_dsp::voice::VoiceParams;

use crate::matrix::Layer;

/// Voicing mode discriminator. Default `Whole` — a fresh patch is single-layer.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum VoicingMode {
    #[default]
    Whole,
    Layer,
    Split,
}

/// Voicing controls: which mode the patch is in, and where Split divides the
/// keyboard.
#[derive(Clone, Copy, Debug)]
pub struct VoicingParams {
    pub mode: VoicingMode,
    /// Split point as MIDI note (0..127). Notes `>= split_point` route to
    /// Upper; notes `< split_point` route to Lower. Ignored when
    /// `mode != Split`.
    pub split_point: u8,
}

impl Default for VoicingParams {
    fn default() -> Self {
        Self {
            mode: VoicingMode::Whole,
            split_point: 60,
        }
    }
}

/// One layer's full parameter set: stack-macro params + voice params. Each
/// layer carries its own pair; the mod matrix slots for the layer live in
/// [`crate::matrix::PatchMatrix`] alongside.
#[derive(Clone, Copy, Debug, Default)]
pub struct LayerParams {
    pub stack: StackParams,
    pub voice: VoiceParams,
}

/// A complete patch: voicing controls + two layer parameter sets. Whole mode
/// ignores `lower`; Layer mode triggers both; Split mode dispatches by note.
#[derive(Clone, Copy, Debug, Default)]
pub struct Patch {
    pub voicing: VoicingParams,
    pub upper: LayerParams,
    pub lower: LayerParams,
}

impl Patch {
    /// Pick the layer a played note routes to under Split mode.
    #[inline]
    pub fn split_layer(&self, note: u8) -> Layer {
        if note >= self.voicing.split_point {
            Layer::Upper
        } else {
            Layer::Lower
        }
    }

    /// Borrow the params for a given layer.
    #[inline]
    pub fn layer(&self, layer: Layer) -> &LayerParams {
        match layer {
            Layer::Upper => &self.upper,
            Layer::Lower => &self.lower,
        }
    }
}
