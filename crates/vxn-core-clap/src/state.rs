//! State save / load helpers.
//!
//! The wire format is whatever the synth's [`ParamModel`] produces /
//! consumes via `snapshot_bytes` / `restore_from_bytes`. This module
//! just routes the bytes through the clack `InputStream` /
//! `OutputStream` for the `state` extension. Each synth keeps wire-format
//! compatibility with its own historical blob there.

use std::io::{Read, Write};

use clack_plugin::stream::{InputStream, OutputStream};
use vxn_core_app::ParamModel;

/// Read `model`'s snapshot and write it to `output`.
pub fn save_blob<M: ParamModel>(model: &M, output: &mut OutputStream) -> std::io::Result<()> {
    let blob = model.snapshot_bytes();
    output.write_all(&blob)
}

/// Drain `input` and apply it to `model`. Returns the model's own
/// `restore_from_bytes` error on parse failure; an I/O error if the
/// stream itself fails.
pub fn load_blob<M: ParamModel>(model: &M, input: &mut InputStream) -> Result<(), String> {
    let mut blob = Vec::new();
    input
        .read_to_end(&mut blob)
        .map_err(|e| format!("state read failed: {e}"))?;
    model.restore_from_bytes(&blob)
}
