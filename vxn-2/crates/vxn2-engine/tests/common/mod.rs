//! Shared test apparatus for vxn2-engine integration tests.

/// 4th-difference click detector: maximum `|b[i+2] − 4b[i+1] + 6b[i] − 4b[i−1] + b[i−2]|`
/// over `range` (caller ensures 2 ≤ range.start and range.end + 2 ≤ buf.len()).
/// Suppresses smooth carriers by f^4 while preserving the full amplitude of a
/// slope discontinuity — the same probe used by the note-off-click harness (0079).
pub fn worst_d4(buf: &[f32], range: std::ops::Range<usize>) -> f64 {
    range
        .map(|i| {
            (buf[i + 2] - 4.0 * buf[i + 1] + 6.0 * buf[i] - 4.0 * buf[i - 1] + buf[i - 2])
                .abs() as f64
        })
        .fold(0.0, f64::max)
}
