//! `gen-web-page` (ticket 0157) — emit the standalone web faceplate `index.html`
//! to stdout.
//!
//! The page bundles the faceplate markup + CSS + classic JS (bootstrap with the
//! param-descriptor / matrix-list / default-patch / subdivision JSON spliced in,
//! every panel, and main.js), plus the `window.ipc` boot-queue stub and the
//! `<script type="module">` that boots `faceplate-bridge.mjs`. The transport ES
//! modules and the two `.wasm` files are placed alongside by the xtask bundler
//! (`cargo xtask web`, ticket 0158); this bin only produces the HTML.
//!
//! Usage:
//!   cargo run -p vxn2-ui-web --bin gen-web-page > dist/index.html

fn main() {
    print!("{}", vxn2_ui_web::build_web_faceplate_html());
}
