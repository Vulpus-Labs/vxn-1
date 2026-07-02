/// Canonical edit list used by the state round-trip tests in both
/// `tests/smoke.rs` (CLAP ABI layer) and `src/lib.rs`
/// (`plugin_state_save_load_round_trips_every_param`, SharedParams layer).
///
/// Covers float / int / enum / bool ids spanning per-op / master / matrix /
/// FX. Each value sits inside the descriptor's valid range.
///
/// Keep the two test files pointing at this single definition so a future
/// schema change that adds or renames a param is caught in both layers at once.
/// Broader test-support consolidation is tracked in ticket 0167.
pub const EDITS: &[(&str, f64)] = &[
    ("master-volume", -3.0),
    ("master-tune", 5.0),
    ("op1-num", 3.0),
    ("op6-level", 88.0),
    ("op4-pan", -0.7),
    ("mtx1-depth", 0.4),
    ("mtx8-depth", -0.7),
    ("reverb-decay", 4.5),
    ("delay-time", 250.0),
    ("assign-mode", 1.0),
    ("glide-time", 200.0),
];
