//! Bake the embedded factory bank into the flat `factory.bin` asset (E019 /
//! 0062). Run by xtask's web build; emits the asset to stdout.
//!
//! Enumerates the bank through the SAME `EnginePresetStore` projection the
//! desktop build serves (category from the directory, canonical state blob),
//! so the web factory list is identical to native. The web controller — which
//! deps only `vxn-app` (ADR 0009) — fetches this asset at boot and parses it
//! with `vxn_app::factory_asset::decode`.

use std::io::Write;

use vxn_app::factory_asset::{encode, FactoryEntry};
use vxn_app::PresetStore;
use vxn_engine::EnginePresetStore;

fn main() {
    let store = EnginePresetStore::new();
    let n = store.factory_len();
    let mut entries = Vec::with_capacity(n);
    for i in 0..n {
        let meta = store
            .factory_meta(i)
            .unwrap_or_else(|| panic!("factory_meta({i}) missing"));
        let load = store
            .factory_load(i)
            .unwrap_or_else(|e| panic!("factory_load({i}) failed: {e}"));
        entries.push(FactoryEntry {
            meta,
            blob: load.blob,
        });
    }
    let bytes = encode(&entries);
    std::io::stdout()
        .write_all(&bytes)
        .expect("write factory.bin to stdout");
    eprintln!("baked {n} factory presets ({} bytes)", bytes.len());
}
