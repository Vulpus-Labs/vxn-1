//! Print the assembled standalone-web faceplate page to stdout (E018 / 0057).
//!
//! `cargo xtask web` runs this (`cargo run -p vxn-ui-web --bin gen-web-page`)
//! and redirects the output into `target/web-dist/index.html`. Keeping the
//! assembly in this crate — rather than reimplementing the splice in xtask —
//! keeps the param-descriptor JSON single-sourced (byte-identical to the
//! plugin's faceplate) and lets xtask stay dependency-free.

fn main() {
    print!("{}", vxn_ui_web::build_web_faceplate_html());
}
