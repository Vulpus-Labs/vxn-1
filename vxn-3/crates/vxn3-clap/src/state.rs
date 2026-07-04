//! VXN3 `clap.state` blob format (ticket 0174).
//!
//! A small, **versioned, self-describing** serialization of the host-facing
//! state: the fixed param table (the [`ParamCache`]) plus each track's active
//! [`EngineKind`] and a reserved per-track patch payload. The preset epic reads
//! the same bytes, so the layout is frozen here.
//!
//! ```text
//! magic     : b"VX3S"           (4 bytes)
//! version   : u16 LE            (= 1)
//! n_params  : u16 LE            (= TOTAL_PARAMS)
//! values    : f32 LE × n_params (from ParamCache, host-facing param values)
//! n_tracks  : u8                (= N_TRACKS)
//! per track : kind u8 ; patch_len u16 LE ; patch bytes[patch_len]
//! ```
//!
//! `patch` is the deep, faceplate-only per-engine layer (ADR 0003 §3). No such
//! editable state exists yet, so `patch_len == 0` today — the field is reserved
//! so a future engine patch round-trips **through** a swap without a format
//! break (bump `VERSION` when its bytes are defined).

use vxn3_engine::{EngineKind, N_TRACKS, TrackKinds};

use crate::params::{ParamCache, TOTAL_PARAMS};

const MAGIC: [u8; 4] = *b"VX3S";
const VERSION: u16 = 1;

/// Serialize the current host state to a blob. Deterministic — the same state
/// always produces identical bytes (required by `clap-validator`).
pub fn save(cache: &ParamCache, kinds: &TrackKinds) -> Vec<u8> {
    let mut b = Vec::with_capacity(4 + 2 + 2 + TOTAL_PARAMS * 4 + 1 + N_TRACKS * 3);
    b.extend_from_slice(&MAGIC);
    b.extend_from_slice(&VERSION.to_le_bytes());
    b.extend_from_slice(&(TOTAL_PARAMS as u16).to_le_bytes());
    for id in 0..TOTAL_PARAMS {
        b.extend_from_slice(&cache.get(id).to_le_bytes());
    }
    b.push(N_TRACKS as u8);
    for t in 0..N_TRACKS {
        b.push(kinds.get(t).as_u8());
        b.extend_from_slice(&0u16.to_le_bytes()); // patch_len — reserved
    }
    b
}

/// Restore host state from a blob into the cache + kind mirror. Returns `Err` on
/// a bad magic / unknown-future version / truncated stream (the shell maps this
/// to a failed `clap_plugin_state::load`). Unknown trailing bytes are ignored,
/// and an unmapped param id past the current table is skipped — both keep older
/// / newer blobs loading cleanly.
#[allow(clippy::result_unit_err)] // parse-failure sentinel; shell maps it to PluginError
pub fn load(bytes: &[u8], cache: &ParamCache, kinds: &TrackKinds) -> Result<(), ()> {
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
    for t in 0..n_tracks {
        let kind = EngineKind::from_u8(r.u8()?);
        let patch_len = r.u16()? as usize;
        let _patch = r.take(patch_len)?; // reserved — no deep patch yet
        if t < N_TRACKS {
            kinds.set(t, kind);
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

    #[test]
    fn round_trips_params_and_kinds() {
        let cache = ParamCache::new();
        let kinds = TrackKinds::new();
        cache.set(0, 0.25);
        cache.set(TOTAL_PARAMS - 1, 0.9);
        kinds.set(2, EngineKind::Noise);
        kinds.set(5, EngineKind::Metal);

        let blob = save(&cache, &kinds);

        let cache2 = ParamCache::new();
        let kinds2 = TrackKinds::new();
        load(&blob, &cache2, &kinds2).unwrap();

        for id in 0..TOTAL_PARAMS {
            assert_eq!(cache.get(id), cache2.get(id), "param {id}");
        }
        for t in 0..N_TRACKS {
            assert_eq!(kinds.get(t), kinds2.get(t), "track {t} kind");
        }
    }

    #[test]
    fn resave_is_byte_identical() {
        // clap-validator state-reproducibility: load(save(x)) then save again ==.
        let cache = ParamCache::new();
        let kinds = TrackKinds::new();
        cache.set(10, 0.42);
        kinds.set(1, EngineKind::Noise);
        let blob1 = save(&cache, &kinds);

        let cache2 = ParamCache::new();
        let kinds2 = TrackKinds::new();
        load(&blob1, &cache2, &kinds2).unwrap();
        assert_eq!(blob1, save(&cache2, &kinds2));
    }

    #[test]
    fn empty_and_garbage_rejected() {
        let cache = ParamCache::new();
        let kinds = TrackKinds::new();
        assert!(load(&[], &cache, &kinds).is_err(), "empty state");
        assert!(load(b"nope", &cache, &kinds).is_err(), "bad magic");
        // Truncated after a valid header.
        assert!(load(b"VX3S\x01\x00", &cache, &kinds).is_err(), "truncated");
    }

    #[test]
    fn future_version_rejected() {
        let mut blob = save(&ParamCache::new(), &TrackKinds::new());
        blob[4] = 0xFF; // bump version low byte past VERSION
        assert!(load(&blob, &ParamCache::new(), &TrackKinds::new()).is_err());
    }
}
