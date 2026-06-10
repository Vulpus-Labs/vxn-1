//! VXN2-specific extension trait: non-CLAP shared state the controller
//! reads / writes alongside the parameter table.
//!
//! The CLAP-automatable params (179 ids, see PARAMETERS.md) live in
//! [`vxn_core_app::ParamModel`]; the matrix-row topology + active flags +
//! slots 9-16 depth ride here.
//!
//! Per [ADR 0002] the dual-layer surface is gone — there is one matrix
//! table per patch.

use vxn_core_app::ParamModel;

use crate::events::MatrixRow;

/// VXN2-specific extension of [`ParamModel`]. Holds the shared state the
/// CLAP param table can't carry: matrix-row topology + slot 9-16 depths.
/// Impl lives in `vxn2-engine` alongside `SharedParams` (orphan rule).
pub trait Vxn2Params: ParamModel {
    /// Read a single matrix row. Slot index is `0..16`.
    fn matrix_row(&self, slot: u8) -> MatrixRow;

    /// Write a single matrix row. Out-of-range slots silently no-op.
    fn set_matrix_row(&self, slot: u8, row: MatrixRow);

    /// Force every dirty bit on the Model. The next main-thread tick's
    /// drain will re-broadcast the full table (every `ParamChanged` +
    /// one `MatrixSnapshot`). Used by the page on boot to re-seed itself
    /// after late-binding primitives miss the initial broadcast.
    fn mark_all_dirty(&self);
}
