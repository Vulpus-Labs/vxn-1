//! Standalone layout-probe window for inspecting the editor's computed geometry.
//!
//! Wrap views of interest in `vxn_ui::Probe::new(cx, "name", |cx| ...)` inside
//! the editor build code, then:
//!
//! ```text
//! cargo run -p vxn-ui --example layout_probe --features layout-probe \
//!   2>&1 | grep PROBE | sort -u
//! ```
//!
//! The window renders off-screen here, so no Screen Recording / window-server
//! permission is needed — the bounds are printed to stderr. See `vxn_ui::Probe`.

#[cfg(feature = "layout-probe")]
fn main() {
    vxn_ui::run_layout_probe();
}

#[cfg(not(feature = "layout-probe"))]
fn main() {
    eprintln!(
        "layout_probe needs its feature: \
         cargo run -p vxn-ui --example layout_probe --features layout-probe"
    );
}
