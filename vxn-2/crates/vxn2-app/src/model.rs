//! VXN2-specific extension trait: non-CLAP shared state the controller
//! reads / writes alongside the parameter table.
//!
//! The CLAP-automatable params (343 ids, see PARAMETERS.md) live in
//! [`vxn_core_app::ParamModel`]; the matrix-row topology + active flags +
//! slots 9-16 depth + the editor-view-only `edit_layer` ride here.

use vxn_core_app::ParamModel;

use crate::events::MatrixRow;

/// Per-voicing-mode layer cursor. Matches `vxn2_engine::matrix::Layer` /
/// `vxn2_engine::voicing::Layer` index semantics: 0 = Upper, 1 = Lower.
/// Defined here so this crate doesn't need to depend on the engine — the
/// engine's matching enum's discriminant is the wire-stable contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Layer {
    Upper = 0,
    Lower = 1,
}

impl Layer {
    pub const COUNT: usize = 2;
    pub const ALL: [Layer; 2] = [Layer::Upper, Layer::Lower];

    #[inline]
    pub fn from_u8(v: u8) -> Layer {
        match v {
            1 => Layer::Lower,
            _ => Layer::Upper,
        }
    }

    #[inline]
    pub fn raw(self) -> u8 {
        self as u8
    }
}

/// VXN2-specific extension of [`ParamModel`]. Holds the shared state the
/// CLAP param table can't carry: matrix-row topology, slot 9-16 depths,
/// and editor view state. Impl lives in `vxn2-engine` alongside
/// `SharedParams` (orphan rule).
pub trait Vxn2Params: ParamModel {
    /// Read a single matrix row. Slot index is `0..16`; layer is the
    /// per-voicing-mode cursor.
    fn matrix_row(&self, layer: Layer, slot: u8) -> MatrixRow;

    /// Write a single matrix row. Out-of-range slots silently no-op.
    fn set_matrix_row(&self, layer: Layer, slot: u8, row: MatrixRow);

    /// Current editor view-state cursor — which layer's per-layer
    /// controls the op-row / matrix overlays render. Not a CLAP param.
    fn edit_layer(&self) -> Layer;

    /// Set the editor view-state cursor.
    fn set_edit_layer(&self, layer: Layer);
}
