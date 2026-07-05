//! VXN3 `clap.state` blob format (ticket 0174).
//!
//! A small, **versioned, self-describing** serialization of the host-facing
//! state: the fixed param table (the [`ParamCache`]) plus each track's active
//! [`EngineKind`] and a reserved per-track patch payload. The preset epic reads
//! the same bytes, so the layout is frozen here.
//!
//! ```text
//! magic     : b"VX3S"           (4 bytes)
//! version   : u16 LE            (= 2)
//! n_params  : u16 LE            (= TOTAL_PARAMS)
//! values    : f32 LE × n_params (from ParamCache, host-facing param values)
//! n_tracks  : u8                (= N_TRACKS)
//! per track : kind u8 ; patch_len u16 LE ; patch bytes[patch_len]
//! ```
//!
//! `patch` is the deep, faceplate-only per-engine layer (ADR 0003 §3, ADR 0005) —
//! the synthesis params below the host table, serialized by the active engine's
//! [`vxn3_engine::TrackEngine::serialize_patch`]. **v2** (0179) fills the slot 0174
//! reserved: [`save`] writes each track's engine patch; [`load`] hands the bytes
//! back so the rebuilt engine can `deserialize_patch` **before** the macro/mix cache
//! replays over it (the deep patch is the base layer; host-table values sit on top).
//!
//! Backward compat: a **v1** blob carried `patch_len == 0` per track; it still loads,
//! leaving each engine at its default patch. A v1 → load → save transition upgrades
//! the blob to v2 (each engine's current patch is written), which is intentional.
//!
//! Today the main thread holds no *edited* deep patch (the flavour store lands in
//! 0180), so a fresh project serializes each engine's **default** patch — real bytes,
//! deterministic. The format + trait + apply-through-swap wiring is what 0179 freezes;
//! per-engine non-default round-trips are covered in the engine crate.

use vxn3_engine::flavour::Flavour;
use vxn3_engine::io::FlavourStore;
use vxn3_engine::{EngineKind, N_TRACKS, TrackKinds, default_flavour_for, params_for};

use crate::params::{ParamCache, TOTAL_PARAMS};

const MAGIC: [u8; 4] = *b"VX3S";
const VERSION: u16 = 2;

/// Serialize the current host state to a blob. Deterministic — the same state
/// always produces identical bytes (required by `clap-validator`). Each track's deep
/// patch is its live **flavour** from the main-thread store (0185) — base vector +
/// binding table + macro names.
pub fn save(cache: &ParamCache, kinds: &TrackKinds, flavours: &FlavourStore) -> Vec<u8> {
    let mut b = Vec::with_capacity(4 + 2 + 2 + TOTAL_PARAMS * 4 + 1 + N_TRACKS * 3);
    b.extend_from_slice(&MAGIC);
    b.extend_from_slice(&VERSION.to_le_bytes());
    b.extend_from_slice(&(TOTAL_PARAMS as u16).to_le_bytes());
    for id in 0..TOTAL_PARAMS {
        b.extend_from_slice(&cache.get(id).to_le_bytes());
    }
    b.push(N_TRACKS as u8);
    let mut patch = Vec::new();
    for t in 0..N_TRACKS {
        b.push(kinds.get(t).as_u8());
        patch.clear();
        flavours.get(t).serialize(&mut patch);
        b.extend_from_slice(&(patch.len() as u16).to_le_bytes());
        b.extend_from_slice(&patch);
    }
    b
}

/// Restore host state from a blob into the cache + kind mirror + flavour store. Returns
/// `Err` on bad magic / unknown-future version / truncated stream (the shell maps this
/// to a failed `clap_plugin_state::load`). Each track's flavour is parsed from its patch
/// bytes; an empty patch (v1 blob) or a shape mismatch leaves the kind's **default**
/// flavour. The caller rebuilds each engine and applies the restored flavour.
#[allow(clippy::result_unit_err)] // parse-failure sentinel; shell maps it to PluginError
pub fn load(
    bytes: &[u8],
    cache: &ParamCache,
    kinds: &TrackKinds,
    flavours: &FlavourStore,
) -> Result<(), ()> {
    let mut r = Reader { b: bytes, pos: 0 };
    if r.take(4)? != MAGIC {
        return Err(());
    }
    if r.u16()? > VERSION {
        return Err(()); // a newer major format we can't understand
    }
    let n_params = r.u16()? as usize;
    for id in 0..n_params {
        let v = r.f32()?;
        if id < TOTAL_PARAMS {
            cache.set(id, v);
        }
    }
    let n_tracks = r.u8()? as usize;
    // `n_tracks` is the blob's count (may exceed N_TRACKS on a future blob); every
    // entry is read to stay aligned, only the first N_TRACKS are stored.
    #[allow(clippy::needless_range_loop)]
    for t in 0..n_tracks {
        let kind = EngineKind::from_u8(r.u8()?);
        let patch_len = r.u16()? as usize;
        let patch = r.take(patch_len)?;
        if t < N_TRACKS {
            kinds.set(t, kind);
            // Parse the flavour; empty / shape-mismatch → the kind's default flavour.
            let flav = if patch.is_empty() {
                None
            } else {
                Flavour::deserialize(patch, params_for(kind).len())?
            };
            flavours.set(t, flav.unwrap_or_else(|| default_flavour_for(kind)));
        }
    }
    Ok(())
}

/// Minimal forward byte reader — `Err` on underrun.
struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ()> {
        let end = self.pos.checked_add(n).ok_or(())?;
        let s = self.b.get(self.pos..end).ok_or(())?;
        self.pos = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, ()> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, ()> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> Result<f32, ()> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vxn3_engine::io::FlavourStore;

    #[test]
    fn round_trips_params_kinds_and_flavours() {
        let cache = ParamCache::new();
        let kinds = TrackKinds::new();
        cache.set(0, 0.25);
        cache.set(TOTAL_PARAMS - 1, 0.9);
        kinds.set(2, EngineKind::Noise);
        kinds.set(5, EngineKind::Metal);
        let store = FlavourStore::new();
        // Track 2 (Noise): a non-default flavour with an edited base + a macro name.
        let mut nf = default_flavour_for(EngineKind::Noise);
        nf.base[0] = 0.42;
        nf.macro_names[0] = "Body".into();
        store.set(2, nf.clone());
        store.set(5, default_flavour_for(EngineKind::Metal));

        let blob = save(&cache, &kinds, &store);

        let cache2 = ParamCache::new();
        let kinds2 = TrackKinds::new();
        let store2 = FlavourStore::new();
        load(&blob, &cache2, &kinds2, &store2).unwrap();

        for id in 0..TOTAL_PARAMS {
            assert_eq!(cache.get(id), cache2.get(id), "param {id}");
        }
        for t in 0..N_TRACKS {
            assert_eq!(kinds.get(t), kinds2.get(t), "track {t} kind");
        }
        assert_eq!(store2.get(2), nf, "non-default flavour (base + macro name) round-trips");
    }

    #[test]
    fn resave_is_byte_identical() {
        // clap-validator state-reproducibility: load(save(x)) then save again ==.
        let cache = ParamCache::new();
        let kinds = TrackKinds::new();
        cache.set(10, 0.42);
        kinds.set(1, EngineKind::Noise);
        let store = FlavourStore::new();
        store.set(1, default_flavour_for(EngineKind::Noise));
        let blob1 = save(&cache, &kinds, &store);

        let cache2 = ParamCache::new();
        let kinds2 = TrackKinds::new();
        let store2 = FlavourStore::new();
        load(&blob1, &cache2, &kinds2, &store2).unwrap();
        assert_eq!(blob1, save(&cache2, &kinds2, &store2));
        assert_eq!(u16::from_le_bytes([blob1[4], blob1[5]]), VERSION);
    }

    #[test]
    fn v1_blob_loads_with_default_flavour_then_upgrades() {
        // A v1 blob: version 1, patch_len == 0 per track (0174's reserved format).
        let cache = ParamCache::new();
        let kinds = TrackKinds::new();
        cache.set(3, 0.7);
        kinds.set(0, EngineKind::Metal);
        let mut v1 = Vec::new();
        v1.extend_from_slice(&MAGIC);
        v1.extend_from_slice(&1u16.to_le_bytes());
        v1.extend_from_slice(&(TOTAL_PARAMS as u16).to_le_bytes());
        for id in 0..TOTAL_PARAMS {
            v1.extend_from_slice(&cache.get(id).to_le_bytes());
        }
        v1.push(N_TRACKS as u8);
        for t in 0..N_TRACKS {
            v1.push(kinds.get(t).as_u8());
            v1.extend_from_slice(&0u16.to_le_bytes()); // patch_len == 0
        }

        let cache2 = ParamCache::new();
        let kinds2 = TrackKinds::new();
        let store2 = FlavourStore::new();
        load(&v1, &cache2, &kinds2, &store2).unwrap();
        assert_eq!(cache2.get(3), 0.7);
        assert_eq!(kinds2.get(0), EngineKind::Metal);
        // Track 0's empty patch → the kind's default flavour.
        assert_eq!(store2.get(0), default_flavour_for(EngineKind::Metal));
        // Resaving upgrades to v2 with real flavour bytes.
        let up = save(&cache2, &kinds2, &store2);
        assert_eq!(u16::from_le_bytes([up[4], up[5]]), 2, "v1 → save is v2");
        assert!(up.len() > v1.len(), "v2 carries flavour bytes v1 did not");
    }

    #[test]
    fn empty_and_garbage_rejected() {
        let cache = ParamCache::new();
        let kinds = TrackKinds::new();
        let store = FlavourStore::new();
        assert!(load(&[], &cache, &kinds, &store).is_err(), "empty state");
        assert!(load(b"nope", &cache, &kinds, &store).is_err(), "bad magic");
        assert!(load(b"VX3S\x01\x00", &cache, &kinds, &store).is_err(), "truncated");
    }

    #[test]
    fn future_version_rejected() {
        let mut blob = save(&ParamCache::new(), &TrackKinds::new(), &FlavourStore::new());
        blob[4] = 0xFF; // bump version low byte past VERSION
        assert!(load(&blob, &ParamCache::new(), &TrackKinds::new(), &FlavourStore::new()).is_err());
    }
}
