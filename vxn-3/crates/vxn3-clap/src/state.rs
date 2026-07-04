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

use vxn3_engine::{EngineKind, N_TRACKS, TrackKinds, make};

use crate::params::{ParamCache, TOTAL_PARAMS};

const MAGIC: [u8; 4] = *b"VX3S";
const VERSION: u16 = 2;

/// Serialize the current host state to a blob. Deterministic — the same state
/// always produces identical bytes (required by `clap-validator`).
///
/// `sample_rate` is only used to build a transient engine per track so it can emit
/// its patch bytes; `serialize_patch` reads sr-independent patch fields, so the
/// output is identical regardless of the value passed.
pub fn save(cache: &ParamCache, kinds: &TrackKinds, sample_rate: f32) -> Vec<u8> {
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
        let kind = kinds.get(t);
        b.push(kind.as_u8());
        // The deep patch of this track's engine. No live main-thread engine yet
        // (0180 adds the flavour store), so serialize a default of the same kind.
        patch.clear();
        make(kind, sample_rate).serialize_patch(&mut patch);
        b.extend_from_slice(&(patch.len() as u16).to_le_bytes());
        b.extend_from_slice(&patch);
    }
    b
}

/// Restore host state from a blob into the cache + kind mirror. Returns `Err` on
/// a bad magic / unknown-future version / truncated stream (the shell maps this
/// to a failed `clap_plugin_state::load`). Unknown trailing bytes are ignored,
/// and an unmapped param id past the current table is skipped — both keep older
/// / newer blobs loading cleanly.
/// `patches[t]` receives track `t`'s raw deep-patch bytes (empty for a v1 blob or a
/// patch-less engine); the caller rebuilds each engine and feeds these to its
/// `deserialize_patch` **before** replaying the macro/mix cache. Cleared and refilled
/// on success; untouched on `Err`.
#[allow(clippy::result_unit_err)] // parse-failure sentinel; shell maps it to PluginError
pub fn load(
    bytes: &[u8],
    cache: &ParamCache,
    kinds: &TrackKinds,
    patches: &mut [Vec<u8>; N_TRACKS],
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
    for p in patches.iter_mut() {
        p.clear();
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
            patches[t].extend_from_slice(patch);
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

    const SR: f32 = 48_000.0;

    fn patch_buf() -> [Vec<u8>; N_TRACKS] {
        Default::default()
    }

    #[test]
    fn round_trips_params_and_kinds() {
        let cache = ParamCache::new();
        let kinds = TrackKinds::new();
        cache.set(0, 0.25);
        cache.set(TOTAL_PARAMS - 1, 0.9);
        kinds.set(2, EngineKind::Noise);
        kinds.set(5, EngineKind::Metal);

        let blob = save(&cache, &kinds, SR);

        let cache2 = ParamCache::new();
        let kinds2 = TrackKinds::new();
        let mut patches = patch_buf();
        load(&blob, &cache2, &kinds2, &mut patches).unwrap();

        for id in 0..TOTAL_PARAMS {
            assert_eq!(cache.get(id), cache2.get(id), "param {id}");
        }
        for (t, patch) in patches.iter().enumerate() {
            assert_eq!(kinds.get(t), kinds2.get(t), "track {t} kind");
            // v2 writes a real (default) patch for every track.
            assert!(!patch.is_empty(), "track {t} patch bytes surfaced");
        }
    }

    #[test]
    fn resave_is_byte_identical() {
        // clap-validator state-reproducibility: load(save(x)) then save again ==.
        let cache = ParamCache::new();
        let kinds = TrackKinds::new();
        cache.set(10, 0.42);
        kinds.set(1, EngineKind::Noise);
        let blob1 = save(&cache, &kinds, SR);

        let cache2 = ParamCache::new();
        let kinds2 = TrackKinds::new();
        let mut patches = patch_buf();
        load(&blob1, &cache2, &kinds2, &mut patches).unwrap();
        assert_eq!(blob1, save(&cache2, &kinds2, SR));
        // A v2 blob is version 2.
        assert_eq!(u16::from_le_bytes([blob1[4], blob1[5]]), VERSION);
    }

    #[test]
    fn v1_blob_loads_with_default_patch_then_upgrades() {
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
        let mut patches = patch_buf();
        load(&v1, &cache2, &kinds2, &mut patches).unwrap();
        assert_eq!(cache2.get(3), 0.7);
        assert_eq!(kinds2.get(0), EngineKind::Metal);
        for (t, patch) in patches.iter().enumerate() {
            assert!(patch.is_empty(), "v1 track {t} carries no patch → default kept");
        }
        // Resaving a loaded v1 blob upgrades it to v2 (documented, intentional).
        let up = save(&cache2, &kinds2, SR);
        assert_eq!(u16::from_le_bytes([up[4], up[5]]), 2, "v1 → save is v2");
        assert!(up.len() > v1.len(), "v2 carries patch bytes v1 did not");
    }

    #[test]
    fn empty_and_garbage_rejected() {
        let cache = ParamCache::new();
        let kinds = TrackKinds::new();
        let mut patches = patch_buf();
        assert!(load(&[], &cache, &kinds, &mut patches).is_err(), "empty state");
        assert!(load(b"nope", &cache, &kinds, &mut patches).is_err(), "bad magic");
        // Truncated after a valid header.
        assert!(load(b"VX3S\x01\x00", &cache, &kinds, &mut patches).is_err(), "truncated");
    }

    #[test]
    fn future_version_rejected() {
        let mut blob = save(&ParamCache::new(), &TrackKinds::new(), SR);
        blob[4] = 0xFF; // bump version low byte past VERSION
        let mut patches = patch_buf();
        assert!(load(&blob, &ParamCache::new(), &TrackKinds::new(), &mut patches).is_err());
    }
}
