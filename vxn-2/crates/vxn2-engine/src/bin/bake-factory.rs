//! `bake-factory` (ticket 0158) — serialise the embedded factory bank to stdout
//! as `factory.bin` for the web bundle.
//!
//! Runs through the SAME [`Vxn2PresetStore`] the plugin serves, so the web page
//! loads byte-identical factory presets. The format is a simple length-prefixed
//! bundle (little-endian), read back by the browser's factory loader (ticket
//! 0159):
//!
//! ```text
//! u32  count
//! per preset:
//!   u32 name_len ; [name_len UTF-8]
//!   u32 cat_len  ; [cat_len UTF-8]   (empty = uncategorised)
//!   u32 blob_len ; [blob_len bytes]  (canonical state blob)
//! ```
//!
//! Usage: `cargo run -p vxn2-engine --bin bake-factory > dist/factory.bin`

use std::io::Write;

use vxn2_engine::Vxn2PresetStore;
use vxn_core_app::PresetStore;

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_str(out: &mut Vec<u8>, s: &str) {
    put_u32(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
}

fn main() {
    let store = Vxn2PresetStore::new();
    let n = store.factory_len();
    let mut out = Vec::new();
    put_u32(&mut out, n as u32);
    for i in 0..n {
        let load = store
            .factory_load(i)
            .unwrap_or_else(|e| panic!("factory preset {i} failed to load: {e}"));
        put_str(&mut out, &load.meta.name);
        put_str(&mut out, load.meta.category.as_deref().unwrap_or(""));
        put_u32(&mut out, load.blob.len() as u32);
        out.extend_from_slice(&load.blob);
    }
    std::io::stdout()
        .write_all(&out)
        .expect("write factory.bin to stdout");
}
