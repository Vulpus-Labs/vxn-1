//! Print the assembled standalone-web faceplate page to stdout.
//!
//! Mirrors `vxn-ui-web`'s `gen-web-page` bin: an xtask web target can run this
//! (`cargo run -p vxn1b-ui-web --bin gen-web-page`) and redirect the output
//! into the web-dist `index.html`. Keeping the assembly in this crate — rather
//! than reimplementing the splice in xtask — keeps the param-descriptor JSON
//! single-sourced (byte-identical to the plugin's faceplate).

fn main() {
    print!("{}", vxn1b_ui_web::build_web_faceplate_html());
}
