//! Canonical plugin-state serialization (ADR 0003 §6 / ticket 0007).
//!
//! One serializer covers everything that must persist: both per-patch blocks,
//! the global block, and the non-automatable shared state (key mode + split
//! point). It is the single format used by CLAP `state` save/load, and is
//! deliberately built so future preset management (ADR 0004) reuses it: a
//! per-patch block serializes as a **self-contained unit** ([`write_patch`] /
//! [`read_patch`]), so a single-patch preset can later load into one layer.
//!
//! Layout (little-endian):
//!
//! ```text
//! magic   : b"VXN1"            (4 bytes)
//! version : u32                (one record format; bumped if it ever changes)
//! global  : f32 × GLOBAL_COUNT
//! upper   : f32 × PATCH_COUNT  (a patch unit)
//! lower   : f32 × PATCH_COUNT  (a patch unit)
//! key_mode: u8
//! split   : u8                 (MIDI note, 0..=127)
//! ```
//!
//! **Pre-release: no backward compatibility with older saved state.** A blob
//! that does not start with the current magic + version is rejected; the host
//! falls back to defaults.

use crate::params::{GLOBAL_COUNT, GlobalValues, KeyMode, PATCH_COUNT, ParamValues, PatchValues};
use std::io::{self, Read, Write};

/// Format magic; first four bytes of every state blob.
pub const MAGIC: [u8; 4] = *b"VXN1";
/// Format version. Bump on any layout change (no migration pre-release).
pub const VERSION: u32 = 1;

/// Everything that persists: the full parameter set plus the shared state that
/// is *not* a CLAP parameter.
#[derive(Clone, Debug)]
pub struct PluginState {
    pub params: ParamValues,
    pub key_mode: KeyMode,
    pub split_point: u8,
}

/// Write a contiguous `f32` block: `count` little-endian values, `get(i)` each.
/// Both the per-patch and global blocks are this exact shape — only the count
/// and accessor differ.
fn write_block(
    count: usize,
    mut get: impl FnMut(usize) -> f32,
    w: &mut impl Write,
) -> io::Result<()> {
    for i in 0..count {
        w.write_all(&get(i).to_le_bytes())?;
    }
    Ok(())
}

/// Read a contiguous `f32` block: `count` little-endian values into `set(i, v)`.
fn read_block(
    count: usize,
    mut set: impl FnMut(usize, f32),
    r: &mut impl Read,
) -> io::Result<()> {
    for i in 0..count {
        set(i, read_f32(r)?);
    }
    Ok(())
}

/// Serialize one layer's per-patch block as a self-contained unit.
pub fn write_patch(p: &PatchValues, w: &mut impl Write) -> io::Result<()> {
    write_block(PATCH_COUNT, |i| p.get_index(i), w)
}

/// Deserialize one layer's per-patch block (clamped to descriptor ranges).
pub fn read_patch(r: &mut impl Read) -> io::Result<PatchValues> {
    let mut p = PatchValues::default();
    read_block(PATCH_COUNT, |i, v| p.set_index(i, v), r)?;
    Ok(p)
}

fn write_global(g: &GlobalValues, w: &mut impl Write) -> io::Result<()> {
    write_block(GLOBAL_COUNT, |i| g.get_index(i), w)
}

fn read_global(r: &mut impl Read) -> io::Result<GlobalValues> {
    let mut g = GlobalValues::default();
    read_block(GLOBAL_COUNT, |i, v| g.set_index(i, v), r)?;
    Ok(g)
}

#[inline]
fn read_f32(r: &mut impl Read) -> io::Result<f32> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)?;
    Ok(f32::from_le_bytes(buf))
}

#[inline]
fn read_u8(r: &mut impl Read) -> io::Result<u8> {
    let mut buf = [0u8; 1];
    r.read_exact(&mut buf)?;
    Ok(buf[0])
}

impl PluginState {
    /// Write the canonical blob.
    pub fn write(&self, w: &mut impl Write) -> io::Result<()> {
        w.write_all(&MAGIC)?;
        w.write_all(&VERSION.to_le_bytes())?;
        write_global(&self.params.global, w)?;
        write_patch(&self.params.layers[0], w)?; // Upper
        write_patch(&self.params.layers[1], w)?; // Lower
        w.write_all(&[self.key_mode as u8, self.split_point])?;
        Ok(())
    }

    /// Read the canonical blob. Rejects any blob whose magic/version does not
    /// match the current format (pre-release: no migration).
    pub fn read(r: &mut impl Read) -> io::Result<Self> {
        let mut magic = [0u8; 4];
        r.read_exact(&mut magic)?;
        if magic != MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unrecognised VXN1 state (bad magic)",
            ));
        }
        let mut ver = [0u8; 4];
        r.read_exact(&mut ver)?;
        let version = u32::from_le_bytes(ver);
        if version != VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported VXN1 state version",
            ));
        }
        let global = read_global(r)?;
        let upper = read_patch(r)?;
        let lower = read_patch(r)?;
        let key_mode = KeyMode::from_u8(read_u8(r)?);
        let split_point = read_u8(r)?;
        Ok(Self {
            params: ParamValues {
                layers: [upper, lower],
                global,
            },
            key_mode,
            split_point,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{Layer, PatchParam};

    #[test]
    fn roundtrips_params_and_shared_state() {
        let mut params = ParamValues::default();
        params
            .layer_mut(Layer::Upper)
            .set(PatchParam::Cutoff, 1234.0);
        params
            .layer_mut(Layer::Lower)
            .set(PatchParam::Cutoff, 5678.0);
        let st = PluginState {
            params,
            key_mode: KeyMode::Split,
            split_point: 64,
        };

        let mut buf = Vec::new();
        st.write(&mut buf).unwrap();
        let back = PluginState::read(&mut &buf[..]).unwrap();

        assert_eq!(back.layer_cutoff(Layer::Upper), 1234.0);
        assert_eq!(back.layer_cutoff(Layer::Lower), 5678.0);
        assert_eq!(back.key_mode, KeyMode::Split);
        assert_eq!(back.split_point, 64);
    }

    #[test]
    fn rejects_bad_magic() {
        let bad = [0u8; 64];
        assert!(PluginState::read(&mut &bad[..]).is_err());
    }

    impl PluginState {
        fn layer_cutoff(&self, l: Layer) -> f32 {
            self.params.layer(l).get(PatchParam::Cutoff)
        }
    }
}
