//! Emit the assembled VXN3 faceplate HTML to stdout, for design preview / iteration:
//!
//! ```sh
//! cargo run -p vxn3-ui-web --example preview > /tmp/vxn3-faceplate.html
//! ```
//!
//! The page runs standalone (no `window.ipc`); opcodes silently no-op, so the flavour
//! editor, grid, and knobs are fully clickable for visual review.

fn main() {
    // No model to build from outside a plugin instance; an empty slice falls back
    // to a default lane per track, which is what a fresh instrument looks like.
    print!("{}", vxn3_ui_web::build_html(&[]));
}
