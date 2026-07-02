/// Single-source re-export of the VXN2 canonical edit list (ticket 0167).
///
/// `EDITS` is defined once in `vxn2_clap` (src/lib.rs) under
/// `#[cfg(any(test, feature = "test-support"))]`. The self-referencing
/// dev-dependency in `Cargo.toml` activates the `test-support` feature
/// during `cargo test`, making the const pub and accessible here.
///
/// Integration tests (`tests/smoke.rs`) reference it as
/// `test_support::EDITS`; unit tests (`src/lib.rs` `#[cfg(test)]` module)
/// reference it as `crate::EDITS`. There is exactly one definition.
pub use vxn2_clap::EDITS;
