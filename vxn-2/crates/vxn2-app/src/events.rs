//! VXN2-specific custom event payloads.
//!
//! UI → controller: [`Vxn2UiCustom`] (wrapped in [`vxn_core_app::UiEvent::Custom`]).
//! Controller → view: [`Vxn2ViewCustom`] (wrapped in [`vxn_core_app::ViewEvent::Custom`]).
//!
//! Plain (non-Custom) CLAP-param edits ride the shared vocabulary —
//! `UiEvent::SetParam` etc. The matrix slot 1-8 depth values are CLAP
//! params, so they flow through that path; everything else about a matrix
//! row (source / dest / curve / active flag; depths 9-16) is non-CLAP and
//! rides Custom.

use crate::model::Layer;

/// Single mod-matrix row. Source / dest are opaque u8 indices into the
/// engine's `matrix::SourceId` / `matrix::DestId` enums; the engine's
/// `Vxn2Params` impl decodes them at storage time. `curve` is an index
/// into `matrix::CurveKind`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MatrixRow {
    pub source: u8,
    pub dest: u8,
    pub curve: u8,
    pub active: bool,
    pub depth: f32,
}

impl Default for MatrixRow {
    fn default() -> Self {
        Self {
            source: 0,
            dest: 0,
            curve: 0,
            active: false,
            depth: 0.0,
        }
    }
}

/// VXN2-only UI intents.
#[derive(Clone, Debug)]
pub enum Vxn2UiCustom {
    /// Editor-side cursor switch: which layer the op-row + matrix overlays
    /// edit. View-state only — never a CLAP param. Persists via the
    /// preset blob so a re-open lands on the layer the user was on.
    SetEditLayer { layer: Layer },

    /// Per-page state: which operator the op-detail panel is showing.
    /// Pure view state on the page; the controller forwards as a
    /// [`Vxn2ViewCustom::OpTabChanged`] echo so the page can re-render
    /// against the controller-owned mirror (matters when host-driven
    /// state load needs to seed the page's op cursor).
    SetOpTab { layer: Layer, op: u8 },

    /// Write a matrix row's topology + active flag (and depth for slots
    /// 9-16; slots 1-8 depth flows through the CLAP `SetParam` path).
    SetMatrixRow {
        layer: Layer,
        slot: u8,
        row: MatrixRow,
    },

    /// Page-side seed: the page asks the controller to push the full
    /// 16 × 2 matrix snapshot. Dispatched from JS right after
    /// `EditorReady` so the overlay can render from a known state.
    RequestMatrixSnapshot,
}

/// VXN2-only view echoes.
#[derive(Clone, Debug)]
pub enum Vxn2ViewCustom {
    EditLayerChanged { layer: Layer },
    OpTabChanged { layer: Layer, op: u8 },
    MatrixRowChanged {
        layer: Layer,
        slot: u8,
        row: MatrixRow,
    },
    /// Full 16 × 2 matrix snapshot. Emitted on
    /// `Vxn2UiCustom::RequestMatrixSnapshot` and on
    /// `Vxn2UiCustom::SetEditLayer` so the overlay can render the
    /// now-current layer's rows without polling.
    MatrixSnapshot {
        upper: [MatrixRow; 16],
        lower: [MatrixRow; 16],
    },
}
