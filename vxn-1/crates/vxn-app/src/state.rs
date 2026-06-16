//! Canonical plugin-state codec (E019 / ticket 0062).
//!
//! The single serializer for everything that persists: the full parameter set
//! (both per-patch layers + the global block) plus the non-automatable shared
//! state (key mode + split point). It is the source of truth for *every*
//! backend — native CLAP host state ([`crate::ParamModel::snapshot_bytes`] via
//! the engine's `SharedParams`) and the web controller's `WebModel` alike —
//! so the wire format cannot drift between native and wasm.
//!
//! It works purely through the [`ParamModel`] + [`Vxn1Params`] trait surface,
//! addressing values by CLAP id, so it lives here in `vxn-app` (no engine
//! types, wasm-clean) rather than next to the engine's `ParamValues`.
//!
//! Layout (little-endian) — **byte-identical** to the engine's historical
//! `vxn-engine::state` format (a drift-guard test asserts this):
//!
//! ```text
//! magic   : b"VXN1"            (4 bytes)
//! version : u32                (bumped on any layout change; no migration)
//! global  : f32 × GLOBAL_COUNT (clap ids [TOTAL-GLOBAL_COUNT .. TOTAL))
//! upper   : f32 × PATCH_COUNT  (clap ids [0 .. PATCH_COUNT))
//! lower   : f32 × PATCH_COUNT  (clap ids [PATCH_COUNT .. 2*PATCH_COUNT))
//! key_mode: u8
//! split   : u8                 (MIDI note, 0..=127)
//! ```
//!
//! **Pre-release: no backward compatibility.** A blob whose magic/version does
//! not match the current format is rejected; the caller falls back to defaults.

use crate::domain::KeyMode;
use crate::model::{ParamModel, Vxn1Params};
use crate::params::{GLOBAL_COUNT, PATCH_COUNT, TOTAL_PARAMS};
use crate::ParamId;

/// Format magic; first four bytes of every state blob.
pub const MAGIC: [u8; 4] = *b"VXN1";
/// Format version. Bump on any layout change (no migration pre-release).
pub const VERSION: u32 = 1;

/// Total serialized size of a state blob: header + every param + shared state.
pub const BLOB_LEN: usize = 4 + 4 + TOTAL_PARAMS * 4 + 2;

/// Clap id of the first global param (globals occupy the tail of the id space).
const GLOBAL_START: usize = TOTAL_PARAMS - GLOBAL_COUNT;

/// Serialize the model + shared state into the canonical blob.
///
/// Generic over any concrete model implementing both traits (the engine's
/// `SharedParams` and the web `WebModel`), so the one codec serves every
/// backend.
pub fn write_state_bytes<M: ParamModel + Vxn1Params + ?Sized>(model: &M) -> Vec<u8> {
    let mut out = Vec::with_capacity(BLOB_LEN);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    // Canonical order: global, then upper, then lower (matches the historical
    // engine layout, which is *not* the CLAP id order).
    for id in GLOBAL_START..TOTAL_PARAMS {
        push_f32(&mut out, model.get(ParamId::new(id)));
    }
    for id in 0..PATCH_COUNT {
        push_f32(&mut out, model.get(ParamId::new(id)));
    }
    for id in PATCH_COUNT..(2 * PATCH_COUNT) {
        push_f32(&mut out, model.get(ParamId::new(id)));
    }
    out.push(model.key_mode() as u8);
    out.push(model.split_point());
    out
}

/// Apply a canonical blob into the model + shared state. Rejects any blob whose
/// magic/version/length does not match the current format (pre-release: no
/// migration), leaving the model untouched on error.
pub fn read_state_into<M: ParamModel + Vxn1Params + ?Sized>(
    model: &M,
    blob: &[u8],
) -> Result<(), String> {
    if blob.len() != BLOB_LEN {
        return Err(format!(
            "bad state blob length: {} (expected {BLOB_LEN})",
            blob.len()
        ));
    }
    if blob[0..4] != MAGIC {
        return Err("unrecognised VXN1 state (bad magic)".into());
    }
    let version = u32::from_le_bytes([blob[4], blob[5], blob[6], blob[7]]);
    if version != VERSION {
        return Err(format!("unsupported VXN1 state version: {version}"));
    }

    let mut off = 8;
    let next = |off: &mut usize| -> f32 {
        let v = f32::from_le_bytes([blob[*off], blob[*off + 1], blob[*off + 2], blob[*off + 3]]);
        *off += 4;
        v
    };
    for id in GLOBAL_START..TOTAL_PARAMS {
        model.set(ParamId::new(id), next(&mut off));
    }
    for id in 0..PATCH_COUNT {
        model.set(ParamId::new(id), next(&mut off));
    }
    for id in PATCH_COUNT..(2 * PATCH_COUNT) {
        model.set(ParamId::new(id), next(&mut off));
    }
    model.set_key_mode(KeyMode::from_u8(blob[off]));
    model.set_split_point(blob[off + 1]);
    Ok(())
}

#[inline]
fn push_f32(buf: &mut Vec<u8>, v: f32) {
    buf.extend_from_slice(&v.to_le_bytes());
}
