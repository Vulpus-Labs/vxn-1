//! Canonical `clap.state` serialization for VXN1b.
//!
//! **Two layers (0216, ADR 0002 §4).** VXN1b is a dual-layer instrument: two
//! independent synths, each with its own patch + private mod matrix. The state
//! blob is therefore **two** single-layer records back to back — one
//! [`LayerState`] per synth. A [`LayerState`] is the flat param block followed by
//! the mod-matrix **topology**, exactly as the single-patch format was (0203):
//!
//! - **Depths ride the param block.** The 16 slot depths are ordinary CLAP
//!   params ([`ParamId::MatrixSlot0Depth`]…), so they serialise in the `f32`
//!   block like every other param — never in the topology bytes.
//! - **Topology is not automatable, so it lives here.** Each slot's
//!   `source`/`dest`/`curve`/`scale_src` is packed as a fixed 5-byte record.
//!
//! Layout (little-endian):
//!
//! ```text
//! magic   : b"VX1B"                       (4 bytes)
//! version : u32                           (bumped to 2 for the two-layer format)
//! layer 0 : LayerState                    (param block + 16 topology records)
//! layer 1 : LayerState                    (param block + 16 topology records)
//! key     : KeyState                      (4 bytes: layer2/split/point/lfo2-link)
//! ```
//!
//! and each `LayerState`:
//!
//! ```text
//! params  : f32 × ParamId::COUNT          (inner per-synth block; 16 depths incl.)
//! matrix  : [active, source, dest, curve, scale] × N_SLOTS   (5 bytes/slot)
//! ```
//!
//! **No migration pre-release.** Older blobs are rejected on read — every layout
//! change is a clean version bump (ADR 0002 Consequences). The single→dual
//! *preset* migration is a separate matter and is real: a legacy single-layer
//! preset TOML still loads (0221, [`crate::preset`]), because the text format is
//! name-keyed and sparse rather than positional.
//!
//! Depths are re-seeded onto each decoded [`MatrixTable`] from that layer's param
//! block on read, so the returned topology is render-ready and can never disagree
//! with the automatable depths (the param block is the single source of truth for
//! depth).

use crate::engine::KeyState;
use crate::matrix::{Curve, DestId, MatrixSlot, MatrixTable, SourceId};
use crate::params::{ParamId, Params};
use std::io::{self, Read, Write};

/// Format magic; first four bytes of every VXN1b state blob. Distinct from
/// VXN1's `b"VXN1"` — the two share no bytes (ADR 0001 §6).
pub const MAGIC: [u8; 4] = *b"VX1B";

/// Format version. `2` = the two-layer format (0216); `3` adds the per-layer
/// mix params (0220); `4` adds `FilterKeyTrack` (0245); `5` adds `CutoffTuned`
/// (0250) — each lengthens the layer's param block; `6` appends the [`KeyState`]
/// record (0221); `7` adds `LayerPan` (0248). Bump on any layout change — the
/// block length is positional, so an older blob read at a newer length would
/// slide topology bytes into param slots rather than fail cleanly. Rejecting the
/// old version is what makes that impossible.
pub const VERSION: u32 = 7;

/// Bytes per packed matrix-topology slot record: `[active, source, dest, curve,
/// scale]`.
const SLOT_RECORD: usize = 5;

/// Inner per-synth param-block length (f32 count) — one [`LayerState`] carries
/// this many values, ahead of its topology.
const LAYER_PARAMS: usize = ParamId::COUNT;

/// One layer's persisted patch: the flat param values (depths included) and the
/// mod-matrix topology. This is the single-patch unit; a [`PluginState`] holds
/// two. Runtime controller state (pitch-bend, mod wheel, per-voice pressure) is
/// transient MIDI and is deliberately *not* saved.
#[derive(Clone, Debug)]
pub struct LayerState {
    pub params: Params,
    pub matrix: MatrixTable,
}

impl LayerState {
    /// The factory-default patch for one layer: default param values with the
    /// default-patch matrix topology ([`crate::matrix::default_patch`]), the 16
    /// slot-depth params seeded from that patch so params and matrix agree on
    /// depth from the first frame (the depth-authority contract, 0205).
    pub fn factory_default() -> Self {
        use crate::params::MATRIX_SLOTS;
        let mut params = Params::default();
        let matrix = crate::matrix::default_patch();
        for slot in 0..MATRIX_SLOTS {
            if let Some(p) = ParamId::slot_depth(slot) {
                params.set(p, matrix.slots[slot].depth);
            }
        }
        Self { params, matrix }
    }

    /// Write one layer: the inner param block, then one 5-byte topology record
    /// per slot. Slot depths are already in the param block, so the topology
    /// carries only `source`/`dest`/`curve`/`scale`.
    fn write(&self, w: &mut impl Write) -> io::Result<()> {
        for i in 0..LAYER_PARAMS {
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

    /// Read one layer: the inner param block, then as many whole topology records
    /// as the stream holds (a clean end at a record boundary leaves the rest
    /// inert — the default read; a truncated record is a hard error). Depths are
    /// re-seeded from the param block so the returned topology is render-ready.
    fn read(r: &mut impl Read) -> io::Result<Self> {
        let mut params = Params::default();
        for i in 0..LAYER_PARAMS {
            params.set_index(i, read_f32(r)?);
        }
        let mut matrix = MatrixTable::default();
        for slot in matrix.slots.iter_mut() {
            match read_slot(r)? {
                Some(rec) => *slot = decode_slot(rec),
                None => break,
            }
        }
        for (i, slot) in matrix.slots.iter_mut().enumerate() {
            slot.depth = params.slot_depth(i);
        }
        Ok(Self { params, matrix })
    }
}

/// Everything a VXN1b patch persists: **both layers'** patches plus the global
/// non-automatable [`KeyState`] — layer-2 enable, split enable + point, and the
/// cross-layer LFO 2 link (0221). The two toggles are what [`crate::KeyMode`] is
/// derived from, so persisting them persists the routing mode; nothing else in
/// the blob knows about `KeyMode` as such.
///
/// `KeyState` is *not* a CLAP param (ADR 0002 §3), so unlike the patch params it
/// has no host-automation path to replay it — the blob is its only home. Before
/// 0221 it was simply lost on save/reload: a split patch came back Single.
#[derive(Clone, Debug)]
pub struct PluginState {
    pub layers: [LayerState; 2],
    pub key: KeyState,
}

impl PluginState {
    /// The factory-default state: both layers at the factory patch, keyboard at
    /// its default (Layer 2 off → Single). Single source of truth for the factory
    /// state — [`crate::Engine::new`] and the shared param store both build from
    /// it. (Layer 2 is off by default, so its patch is idle until the user
    /// enables it.)
    pub fn factory_default() -> Self {
        Self {
            layers: [LayerState::factory_default(), LayerState::factory_default()],
            key: KeyState::default(),
        }
    }

    /// Write the canonical blob: magic, version, the two layer records, then the
    /// key record. Key state goes **last** so the layer blocks keep the offsets
    /// they had in version 5 — the two-layer reader is unchanged up to that point.
    pub fn write(&self, w: &mut impl Write) -> io::Result<()> {
        w.write_all(&MAGIC)?;
        w.write_all(&VERSION.to_le_bytes())?;
        for layer in &self.layers {
            layer.write(w)?;
        }
        self.key.write(w)?;
        Ok(())
    }

    /// Read the canonical blob. Rejects any blob whose magic/version does not
    /// match the current two-layer format (pre-release: no migration).
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
        let l0 = LayerState::read(r)?;
        let l1 = LayerState::read(r)?;
        // A blob that ends before the key record is truncated, not "old": the
        // version gate above already rejected every earlier layout, so a short
        // read here can only be corruption. `KeyState::read` fails hard on it.
        let key = KeyState::read(r)?;
        Ok(Self { layers: [l0, l1], key })
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

    /// Bytes in the trailing [`KeyState`] record (0221).
    const KEY_RECORD: usize = 4;

    fn nondefault_layer(cutoff: f32) -> LayerState {
        let mut params = Params::default();
        params.set(ParamId::Cutoff, cutoff);
        params.set(ParamId::MatrixSlot0Depth, 1.0);
        params.set(ParamId::MatrixSlot3Depth, -0.5);
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
        LayerState { params, matrix }
    }

    fn nondefault_state() -> PluginState {
        // Deliberately distinct layers so a round-trip can't accidentally pass by
        // symmetry (0216: distinct matrices per layer survive save/reload).
        let l1 = nondefault_layer(1234.0);
        let mut l2 = nondefault_layer(220.0);
        // Give layer 2 a different topology in a slot layer 1 leaves inert.
        l2.params.set(ParamId::MatrixSlot5Depth, 0.75);
        l2.matrix.slots[5] = MatrixSlot {
            source: SourceId::Lfo1,
            dest: DestId::Cutoff,
            depth: 0.75,
            curve: Curve::Lin,
            scale_src: SourceId::None,
        };
        PluginState {
            layers: [l1, l2],
            // Every key field off its default, so a round-trip can't pass by
            // accidentally rebuilding the factory record.
            key: KeyState {
                layer2_on: true,
                split_enabled: true,
                split_point: 48,
                lfo2_link: true,
            },
        }
    }

    #[test]
    fn roundtrips_both_layers_independently() {
        let st = nondefault_state();
        let mut buf = Vec::new();
        st.write(&mut buf).unwrap();
        let back = PluginState::read(&mut &buf[..]).unwrap();

        // Layer 1.
        assert_eq!(back.layers[0].params.get(ParamId::Cutoff), 1234.0);
        assert_eq!(back.layers[0].matrix.slots[0].source, SourceId::Env2);
        assert_eq!(back.layers[0].matrix.slots[0].depth, 1.0);
        assert_eq!(back.layers[0].matrix.slots[3].scale_src, SourceId::ModWheel);

        // Layer 2 — distinct params + a slot layer 1 doesn't use.
        assert_eq!(back.layers[1].params.get(ParamId::Cutoff), 220.0);
        assert_eq!(back.layers[1].matrix.slots[5].source, SourceId::Lfo1);
        assert_eq!(back.layers[1].matrix.slots[5].dest, DestId::Cutoff);
        assert_eq!(back.layers[1].matrix.slots[5].depth, 0.75);
        // The distinguishing slot is inert on layer 1.
        assert!(!back.layers[0].matrix.slots[5].is_active());
    }

    #[test]
    fn round_trips_key_state() {
        // 0221: KeyMode/split/link are not CLAP params, so the blob is the only
        // thing that can carry them across a save/reload.
        let st = nondefault_state();
        let mut buf = Vec::new();
        st.write(&mut buf).unwrap();
        let back = PluginState::read(&mut &buf[..]).unwrap();
        assert_eq!(back.key, st.key);
        assert_eq!(back.key.key_mode(), crate::engine::KeyMode::Split);
        assert_eq!(back.key.split_point, 48);
        assert!(back.key.lfo2_link);
    }

    #[test]
    fn factory_state_is_single_mode() {
        let st = PluginState::factory_default();
        assert_eq!(st.key.key_mode(), crate::engine::KeyMode::Single);
        let mut buf = Vec::new();
        st.write(&mut buf).unwrap();
        let back = PluginState::read(&mut &buf[..]).unwrap();
        assert_eq!(back.key, KeyState::default());
    }

    #[test]
    fn blob_length_is_two_full_layers_plus_the_key_record() {
        let st = PluginState::factory_default();
        let mut buf = Vec::new();
        st.write(&mut buf).unwrap();
        let layer = LAYER_PARAMS * 4 + N_SLOTS * SLOT_RECORD;
        assert_eq!(buf.len(), 4 + 4 + 2 * layer + KEY_RECORD);
    }

    #[test]
    fn missing_key_record_is_an_error() {
        // A v5-shaped blob (two layers, no key record) stamped with the current
        // version is corruption, and must not decode as "default keyboard".
        let st = nondefault_state();
        let mut buf = Vec::new();
        st.write(&mut buf).unwrap();
        buf.truncate(buf.len() - KEY_RECORD);
        assert!(PluginState::read(&mut &buf[..]).is_err());
    }

    #[test]
    fn rejects_bad_magic_and_version() {
        let bad = [0u8; 64];
        assert!(PluginState::read(&mut &bad[..]).is_err());

        let st = PluginState::factory_default();
        let mut buf = Vec::new();
        st.write(&mut buf).unwrap();
        buf[4] = 0xff; // corrupt the version
        assert!(PluginState::read(&mut &buf[..]).is_err());
    }

    #[test]
    fn truncated_second_layer_is_an_error() {
        let st = nondefault_state();
        let mut buf = Vec::new();
        st.write(&mut buf).unwrap();
        // Drop two bytes so layer 1's final topology record is incomplete.
        buf.truncate(buf.len() - 2);
        assert!(PluginState::read(&mut &buf[..]).is_err());
    }

    #[test]
    fn empty_blob_is_an_error() {
        // The 0196 empty-state contract: an empty blob is a hard failure.
        assert!(PluginState::read(&mut &[][..]).is_err());
    }

    #[test]
    fn depths_ride_the_param_block_not_topology() {
        // Each layer's topology block is exactly 5 bytes per slot — no room for a
        // depth f32 — so depth can only be riding the param block.
        let st = PluginState::factory_default();
        let mut buf = Vec::new();
        st.write(&mut buf).unwrap();
        let layer = LAYER_PARAMS * 4 + N_SLOTS * SLOT_RECORD;
        assert_eq!(buf.len(), 4 + 4 + 2 * layer + KEY_RECORD);
    }
}
