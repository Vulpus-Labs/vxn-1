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
    /// Per-page state: which operator the op-detail panel is showing.
    /// Pure view state on the page; the controller forwards as a
    /// [`Vxn2ViewCustom::OpTabChanged`] echo so the page can re-render
    /// against the controller-owned mirror (matters when host-driven
    /// state load needs to seed the page's op cursor).
    SetOpTab { op: u8 },

    /// Write a matrix row's topology + active flag (and depth for slots
    /// 9-16; slots 1-8 depth flows through the CLAP `SetParam` path).
    SetMatrixRow { slot: u8, row: MatrixRow },

    /// Page-side seed: the page asks the controller to push the full
    /// 16-row matrix snapshot. Dispatched from JS right after
    /// `EditorReady` so the overlay can render from a known state.
    RequestMatrixSnapshot,

    /// Write op `op`'s `side` (0 = left/below BP, 1 = right/above BP) KS
    /// level-curve selector (`curve` = `ks::KsCurve` discriminant 0..=3).
    /// Non-CLAP patch state — see [`MatrixRow`] for the parallel mechanism.
    SetKsCurve { op: u8, side: u8, curve: u8 },

    /// Page-side seed: ask the controller to push the full KS-curve
    /// snapshot. Dispatched alongside `RequestMatrixSnapshot` so the op-row
    /// graphs render their real per-side shapes from a known state.
    RequestKsCurveSnapshot,

    /// Page-side seed: flip every dirty bit on the Model so the next
    /// main-thread tick re-broadcasts the full table (every
    /// `ParamChanged` + one `MatrixSnapshot`). The page fires this once
    /// it has finished binding primitives so it doesn't depend on the
    /// initial `SharedParams::new` seed surviving until after bind.
    RequestFullRebroadcast,
}

/// VXN2-only view echoes.
#[derive(Clone, Debug)]
pub enum Vxn2ViewCustom {
    OpTabChanged { op: u8 },
    /// Full 16-row matrix snapshot. Emitted on
    /// `Vxn2UiCustom::RequestMatrixSnapshot` so the overlay can render
    /// without polling.
    MatrixSnapshot { rows: [MatrixRow; 16] },
    /// Full KS-curve snapshot: per op (outer, 0..6), per side (inner, [L, R])
    /// curve discriminant. Emitted on `RequestKsCurveSnapshot` and whenever
    /// the model's curve state drifts (preset load), so each op-row graph
    /// paints its real shapes without polling.
    KsCurveSnapshot { curves: [[u8; 2]; 6] },
}
