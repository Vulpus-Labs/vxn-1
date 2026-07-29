//! Canonical `clap.state` serialization for VXN1b (ticket 0203).
//!
//! A blob is the flat param block followed by the mod-matrix **topology**. The
//! split follows ADR 0001 §5–§6 (mirroring VXN2 ADR 0009 "Persistence"):
//!
//! - **Depths ride the param block.** The 16 slot depths are ordinary CLAP
//!   params ([`ParamId::MatrixSlot0Depth`]…), so they serialise in the `f32`
//!   block like every other param — never in the topology bytes.
//! - **Topology is not automatable, so it lives here.** Each slot's
//!   `source`/`dest`/`curve`/`scale_src` is packed as a fixed record (VXN2 ADR
//!   0009 byte layout: an active bit plus the four id bytes).
//!
//! Layout (little-endian):
//!
//! ```text
//! magic   : b"VX1B"                    (4 bytes)
//! version : u32                        (one record format; bumped on change)
//! params  : f32 × TOTAL_PARAMS         (includes the 16 slot depths)
//! matrix  : [active, source, dest, curve, scale] × N_SLOTS   (5 bytes/slot)
//! ```
//!
//! **Back-compat default read (ADR 0001 §6, ticket 0203).** The matrix block is
//! *optional on read*: a blob that ends after the param block — a pre-topology
//! or otherwise matrix-less save — decodes to all-`None` slots rather than
//! failing. Bytes are read a whole slot record at a time; a clean end at a
//! record boundary stops decoding and leaves the remaining slots inert. A
//! *truncated* record (a partial slot) is still an error — that is corruption,
//! not an old format.
//!
//! Depths are re-seeded onto the decoded [`MatrixTable`] from the param block on
//! read, so the returned topology is render-ready and can never disagree with
//! the automatable depths (the param block is the single source of truth for
//! depth).

use crate::matrix::{Curve, DestId, MatrixSlot, MatrixTable, SourceId};
use crate::params::{Params, TOTAL_PARAMS};
use std::io::{self, Read, Write};

/// Format magic; first four bytes of every VXN1b state blob. Distinct from
/// VXN1's `b"VXN1"` — the two share no bytes (ADR 0001 §6).
pub const MAGIC: [u8; 4] = *b"VX1B";

/// Format version. Bump on any layout change (no migration pre-release).
pub const VERSION: u32 = 1;

/// Bytes per packed matrix-topology slot record: `[active, source, dest, curve,
/// scale]`.
const SLOT_RECORD: usize = 5;

/// Everything a VXN1b patch persists: the flat param values (depths included)
/// and the mod-matrix topology. Runtime controller state (pitch-bend, mod
/// wheel, per-voice pressure) is transient MIDI and is deliberately *not* saved.
#[derive(Clone, Debug)]
pub struct PluginState {
    pub params: Params,
    pub matrix: MatrixTable,
}

impl PluginState {
    /// The factory-default patch: default param values with the default-patch
    /// matrix topology ([`crate::matrix::default_patch`]), and the 16 slot-depth
    /// params seeded from that patch so params and matrix agree on depth from the
    /// first frame (the depth-authority contract, 0205). Single source of truth
    /// for the factory state — [`crate::Engine::new`] and the shared param store
    /// both build from it.
    pub fn factory_default() -> Self {
        use crate::params::{MATRIX_SLOTS, ParamId};
        let mut params = Params::default();
        let matrix = crate::matrix::default_patch();
        for slot in 0..MATRIX_SLOTS {
            if let Some(p) = ParamId::slot_depth(slot) {
                params.set(p, matrix.slots[slot].depth);
            }
        }
        Self { params, matrix }
    }

    /// Write the canonical blob: magic, version, the param block, then one
    /// 5-byte topology record per slot. Slot depths are already in the param
    /// block, so the topology carries only `source`/`dest`/`curve`/`scale`.
    pub fn write(&self, w: &mut impl Write) -> io::Result<()> {
        w.write_all(&MAGIC)?;
        w.write_all(&VERSION.to_le_bytes())?;
        for i in 0..TOTAL_PARAMS {
            w.write_all(&self.params.get_index(i).to_le_bytes())?;
        }
        for slot in &self.matrix.slots {
            w.write_all(&[
                slot.is_active() as u8,
                slot.source as u8,
                slot.dest as u8,
                slot.curve as u8,
                slot.scale_src as u8,
            ])?;
        }
        Ok(())
    }

    /// Read the canonical blob. Rejects any blob whose magic/version does not
    /// match the current format (pre-release: no migration). A blob that ends
    /// after the param block decodes with all-`None` matrix slots (default
    /// read); a truncated slot record is a hard error.
    pub fn read(r: &mut impl Read) -> io::Result<Self> {
        let mut magic = [0u8; 4];
        r.read_exact(&mut magic)?;
        if magic != MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unrecognised VXN1b state (bad magic)",
            ));
        }
        let mut ver = [0u8; 4];
        r.read_exact(&mut ver)?;
        if u32::from_le_bytes(ver) != VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported VXN1b state version",
            ));
        }

        let mut params = Params::default();
        for i in 0..TOTAL_PARAMS {
            params.set_index(i, read_f32(r)?);
        }

        // Topology is optional: decode as many whole slot records as the stream
        // holds; a clean end leaves the rest inert (default read).
        let mut matrix = MatrixTable::default();
        for slot in matrix.slots.iter_mut() {
            match read_slot(r)? {
                Some(rec) => *slot = decode_slot(rec),
                None => break,
            }
        }
        // Depth authority is the param block — re-seed it onto every slot so the
        // returned topology is render-ready (the evaluator reads `slot.depth`).
        for (i, slot) in matrix.slots.iter_mut().enumerate() {
            slot.depth = params.slot_depth(i);
        }

        Ok(Self { params, matrix })
    }
}

/// Decode one 5-byte topology record. The active byte is the authoritative
/// gate (ADR 0009): a cleared bit yields the inert default slot regardless of
/// the id bytes, and out-of-range ids degrade to `None` via `from_u8`.
fn decode_slot(rec: [u8; SLOT_RECORD]) -> MatrixSlot {
    if rec[0] == 0 {
        return MatrixSlot::default();
    }
    MatrixSlot {
        source: SourceId::from_u8(rec[1]),
        dest: DestId::from_u8(rec[2]),
        curve: Curve::from_u8(rec[3]),
        // depth is re-seeded from the param block by the caller.
        depth: 0.0,
        scale_src: SourceId::from_u8(rec[4]),
    }
}

/// Read one whole slot record. `Ok(None)` on a clean end at a record boundary
/// (no bytes left — the default-read case); `Ok(Some(_))` on a full record; an
/// `UnexpectedEof` error on a partial record (corruption). Tolerates a reader
/// that returns the record across several `read` calls.
fn read_slot(r: &mut impl Read) -> io::Result<Option<[u8; SLOT_RECORD]>> {
    let mut buf = [0u8; SLOT_RECORD];
    let mut filled = 0;
    while filled < SLOT_RECORD {
        match r.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    match filled {
        0 => Ok(None),
        SLOT_RECORD => Ok(Some(buf)),
        _ => Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "truncated matrix topology record",
        )),
    }
}

#[inline]
fn read_f32(r: &mut impl Read) -> io::Result<f32> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)?;
    Ok(f32::from_le_bytes(buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matrix::N_SLOTS;
    use crate::params::ParamId;

    fn nondefault_state() -> PluginState {
        let mut params = Params::default();
        params.set(ParamId::Cutoff, 1234.0);
        params.set(ParamId::MatrixSlot0Depth, 1.0);
        params.set(ParamId::MatrixSlot3Depth, -0.5);
        // Topology: two routed slots + a scaled one.
        let mut matrix = MatrixTable::default();
        matrix.slots[0] = MatrixSlot {
            source: SourceId::Env2,
            dest: DestId::Amp,
            depth: 1.0,
            curve: Curve::Lin,
            scale_src: SourceId::None,
        };
        matrix.slots[3] = MatrixSlot {
            source: SourceId::Lfo2,
            dest: DestId::Pitch,
            depth: -0.5,
            curve: Curve::Bipolar,
            scale_src: SourceId::ModWheel,
        };
        PluginState { params, matrix }
    }

    #[test]
    fn roundtrips_params_and_topology() {
        let st = nondefault_state();
        let mut buf = Vec::new();
        st.write(&mut buf).unwrap();
        let back = PluginState::read(&mut &buf[..]).unwrap();

        assert_eq!(back.params.get(ParamId::Cutoff), 1234.0);
        assert_eq!(back.params.get(ParamId::MatrixSlot0Depth), 1.0);
        assert_eq!(back.params.get(ParamId::MatrixSlot3Depth), -0.5);

        // Topology round-trips; depth is re-seeded from the params.
        let s0 = back.matrix.slots[0];
        assert_eq!(s0.source, SourceId::Env2);
        assert_eq!(s0.dest, DestId::Amp);
        assert_eq!(s0.curve, Curve::Lin);
        assert_eq!(s0.scale_src, SourceId::None);
        assert_eq!(s0.depth, 1.0);

        let s3 = back.matrix.slots[3];
        assert_eq!(s3.source, SourceId::Lfo2);
        assert_eq!(s3.dest, DestId::Pitch);
        assert_eq!(s3.curve, Curve::Bipolar);
        assert_eq!(s3.scale_src, SourceId::ModWheel);
        assert_eq!(s3.depth, -0.5);

        // Every other slot is inert.
        for (i, slot) in back.matrix.slots.iter().enumerate() {
            if i != 0 && i != 3 {
                assert!(!slot.is_active(), "slot {i} should be inert");
            }
        }
    }

    #[test]
    fn depths_are_not_double_encoded_in_topology() {
        // The topology block is exactly 5 bytes per slot — no room for a depth
        // f32 — so depth can only be riding the param block.
        let st = PluginState {
            params: Params::default(),
            matrix: MatrixTable::default(),
        };
        let mut buf = Vec::new();
        st.write(&mut buf).unwrap();
        let expected = 4 + 4 + TOTAL_PARAMS * 4 + N_SLOTS * SLOT_RECORD;
        assert_eq!(buf.len(), expected);
    }

    #[test]
    fn blob_without_topology_reads_all_none() {
        // A pre-topology blob: magic + version + param block only.
        let params = {
            let mut p = Params::default();
            p.set(ParamId::Cutoff, 555.0);
            p
        };
        let mut buf = Vec::new();
        buf.extend_from_slice(&MAGIC);
        buf.extend_from_slice(&VERSION.to_le_bytes());
        for i in 0..TOTAL_PARAMS {
            buf.extend_from_slice(&params.get_index(i).to_le_bytes());
        }

        let back = PluginState::read(&mut &buf[..]).unwrap();
        assert_eq!(back.params.get(ParamId::Cutoff), 555.0);
        for (i, slot) in back.matrix.slots.iter().enumerate() {
            assert_eq!(*slot, MatrixSlot::default(), "slot {i} must default to None");
        }
    }

    #[test]
    fn partial_topology_reads_present_slots_and_defaults_rest() {
        // Write a full blob, then truncate to keep only the first slot record.
        let st = nondefault_state();
        let mut buf = Vec::new();
        st.write(&mut buf).unwrap();
        let head = 4 + 4 + TOTAL_PARAMS * 4;
        buf.truncate(head + SLOT_RECORD);

        let back = PluginState::read(&mut &buf[..]).unwrap();
        assert_eq!(back.matrix.slots[0].source, SourceId::Env2);
        for slot in &back.matrix.slots[1..] {
            assert!(!slot.is_active(), "slots past the truncation are inert");
        }
    }

    #[test]
    fn truncated_slot_record_is_an_error() {
        let st = nondefault_state();
        let mut buf = Vec::new();
        st.write(&mut buf).unwrap();
        // Drop two bytes so the last slot record is incomplete.
        buf.truncate(buf.len() - 2);
        assert!(PluginState::read(&mut &buf[..]).is_err());
    }

    #[test]
    fn rejects_bad_magic_and_version() {
        let bad = [0u8; 64];
        assert!(PluginState::read(&mut &bad[..]).is_err());

        let st = nondefault_state();
        let mut buf = Vec::new();
        st.write(&mut buf).unwrap();
        buf[4] = 0xff; // corrupt the version
        assert!(PluginState::read(&mut &buf[..]).is_err());
    }

    #[test]
    fn empty_blob_is_an_error() {
        // The 0196 empty-state contract: an empty blob is a hard failure, not a
        // silent accept (the CLAP layer maps this to a `false` load in 0204).
        assert!(PluginState::read(&mut &[][..]).is_err());
    }
}
