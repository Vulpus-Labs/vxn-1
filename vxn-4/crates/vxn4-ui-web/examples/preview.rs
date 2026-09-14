//! Emit the assembled VXN4 faceplate HTML to stdout, for design preview and
//! for checking the page's rendered size against `EDITOR_HEIGHT`:
//!
//! ```sh
//! cargo run -p vxn4-ui-web --example preview > /tmp/vxn4-faceplate.html
//! ```
//!
//! The page runs standalone — it holds all its own state and posts nothing —
//! so every control is live in a plain browser. `#mixer` / `#operators` /
//! `#matrix` open straight onto one surface, which is how a design review
//! screenshots them one at a time.

fn main() {
    print!("{}", vxn4_ui_web::build_html());
}
