//! Shared test apparatus for vxn2-engine integration tests.

/// 4th-difference click detector: maximum `|b[i+2] − 4b[i+1] + 6b[i] − 4b[i−1] + b[i−2]|`
/// over `range` (caller ensures 2 ≤ range.start and range.end + 2 ≤ buf.len()).
/// Suppresses smooth carriers by f^4 while preserving the full amplitude of a
/// slope discontinuity — the same probe used by the note-off-click harness (0079).
// Canonical in vxn-core-dsp (0226); re-exported so `common::worst_d4` resolves.
pub use vxn_core_dsp::test_util::worst_d4;
