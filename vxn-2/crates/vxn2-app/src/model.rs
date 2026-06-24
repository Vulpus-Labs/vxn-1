//! VXN2-specific extension trait: non-CLAP shared state the controller
//! reads / writes alongside the parameter table.
//!
//! The CLAP-automatable params (173 ids, see PARAMETERS.md) live in
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

    /// Read every op's per-side KS level-curve selector as `[[L, R]; 6]`
    /// discriminants (`ks::KsCurve` 0..=3).
    fn ks_curves(&self) -> [[u8; 2]; 6];

    /// Write op `op`'s `side` (0 = left, 1 = right) KS curve selector.
    /// Out-of-range op / side silently no-op.
    fn set_ks_curve(&self, op: u8, side: u8, curve: u8);

    /// Drain the KS-curve dirty flag (set on any `set_ks_curve` / bulk
    /// store). `true` means the controller should push a fresh
    /// `KsCurveSnapshot` this tick.
    fn take_dirty_ks_curve(&self) -> bool;

    /// Read every op's EG level-curve selector as `[u8; 6]` discriminants
    /// (`eg::EgCurve`: 0 = Exp, 1 = Lin). Ticket 0128.
    fn eg_curves(&self) -> [u8; 6];

    /// Write op `op`'s EG curve selector. Out-of-range op silently no-ops.
    fn set_eg_curve(&self, op: u8, curve: u8);

    /// Drain the EG-curve dirty flag (set on any `set_eg_curve` / bulk
    /// store). `true` means the controller should push a fresh
    /// `EgCurveSnapshot` this tick.
    fn take_dirty_eg_curve(&self) -> bool;

    /// Force every dirty bit on the Model. The next main-thread tick's
    /// drain will re-broadcast the full table (every `ParamChanged` +
    /// one `MatrixSnapshot`). Used by the page on boot to re-seed itself
    /// after late-binding primitives miss the initial broadcast.
    fn mark_all_dirty(&self);
}
