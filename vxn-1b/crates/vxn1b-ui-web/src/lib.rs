//! VXN1b HTML faceplate — skeleton (0197).
//!
//! Placeholder so the product's crate layout is complete and wired into the
//! workspace. The compact three-row faceplate and the mod-matrix overlay
//! (ADR 0001 §7) — plus the tabbed FX section (§8) — land in E038; this crate
//! re-exports the shared wry-host handle types those tickets build on, and pins
//! the intended editor dimensions.

// Re-exported so the future clack shell (0204) can name the editor handle /
// error without reaching into `vxn-core-ui-web` directly.
pub use vxn_core_ui_web::{EditorHandle, OpenEditorError};

/// Provisional faceplate size — narrower than VXN1's four-row panel, reflecting
/// the compact layout goal (ADR 0001 §7). Tuned when the faceplate lands (E038).
pub const EDITOR_WIDTH: u32 = 760;
/// Provisional faceplate height (see [`EDITOR_WIDTH`]).
pub const EDITOR_HEIGHT: u32 = 480;
