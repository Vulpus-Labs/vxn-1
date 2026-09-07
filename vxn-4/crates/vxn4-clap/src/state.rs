//! VXN4's `clap.state` blob.
//!
//! Everything the host can address is eleven floats, so the blob is those
//! floats and a header that says how many there are:
//!
//! ```text
//! magic     : b"VX4S"           (4 bytes)
//! version   : u16 LE            (= 1)
//! n_params  : u16 LE            (= TOTAL_PARAMS at save time)
//! values    : f32 LE x n_params
//! ```
//!
//! `n_params` is written rather than assumed, which is the whole forward-compat
//! story: a blob saved by a build with fewer params loads into a build with more
//! (the extra params keep their defaults), and a blob with more loads into one
//! with fewer (the tail is skipped). That is only sound while ids are **never
//! reused for a different meaning** — the positional scheme in
//! [`crate::params::decode`] is what has to hold, and its
//! `the_positional_scheme_is_stable` test is what holds it.
//!
//! There is no deeper layer to serialize. The patches are hardwired and the
//! matrix is patch state, not user state, so a project restores to a known
//! patch plus knob positions. When patches become editable this blob gains a
//! payload and a version bump; the header is shaped for that.

use crate::params::{ParamCache, TOTAL_PARAMS};

const MAGIC: [u8; 4] = *b"VX4S";
const VERSION: u16 = 1;

/// Serialize the cache. Deterministic — the same state always produces
/// identical bytes, which `clap-validator` requires.
pub fn save(cache: &ParamCache) -> Vec<u8> {
    let mut b = Vec::with_capacity(8 + TOTAL_PARAMS * 4);
    b.extend_from_slice(&MAGIC);
    b.extend_from_slice(&VERSION.to_le_bytes());
    b.extend_from_slice(&(TOTAL_PARAMS as u16).to_le_bytes());
    for id in 0..TOTAL_PARAMS {
        b.extend_from_slice(&cache.get(id).to_le_bytes());
    }
    b
}

/// Restore into `cache`. `Err` on bad magic, a future version, or a truncated
/// stream — the shell maps that to a failed `clap_plugin_state::load` rather
/// than loading half a project silently.
///
/// Values go through [`ParamCache::set`], which clamps: a corrupted blob cannot
/// put a patch index out of range.
#[allow(clippy::result_unit_err)] // parse-failure sentinel; the shell maps it
pub fn load(bytes: &[u8], cache: &ParamCache) -> Result<(), ()> {
    let mut r = Reader { b: bytes, pos: 0 };
    if r.take(4)? != MAGIC {
        return Err(());
    }
    if r.u16()? > VERSION {
        return Err(());
    }
    let n = r.u16()? as usize;
    for id in 0..n {
        let v = f32::from_le_bytes(r.take(4)?.try_into().map_err(|_| ())?);
        // Ids past our table are skipped, not an error — see the module note on
        // forward compatibility. The read still has to happen, or the stream
        // desynchronises.
        if id < TOTAL_PARAMS {
            cache.set(id, v);
        }
    }
    Ok(())
}

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

    fn u16(&mut self) -> Result<u16, ()> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().map_err(|_| ())?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{N_FIXED, default_value};

    fn filled() -> ParamCache {
        let c = ParamCache::new();
        c.set(0, 3.0); // patch: saws
        c.set(1, 1.0); // quality: 16x
        c.set(2, 0.75); // master gain
        for m in 0..vxn4_engine::N_MACROS {
            c.set(N_FIXED + m, 0.1 * m as f32);
        }
        c
    }

    #[test]
    fn a_blob_round_trips_every_param() {
        let a = filled();
        let blob = save(&a);
        let b = ParamCache::new();
        load(&blob, &b).expect("load");
        for id in 0..TOTAL_PARAMS {
            assert_eq!(a.get(id), b.get(id), "param {id}");
        }
    }

    /// Byte-for-byte determinism: `clap-validator` saves twice and compares.
    #[test]
    fn saving_the_same_state_twice_gives_the_same_bytes() {
        let c = filled();
        assert_eq!(save(&c), save(&c));
    }

    #[test]
    fn the_blob_is_the_size_the_header_claims() {
        let blob = save(&ParamCache::new());
        assert_eq!(blob.len(), 8 + TOTAL_PARAMS * 4);
        assert_eq!(&blob[..4], b"VX4S");
    }

    #[test]
    fn rubbish_is_rejected_rather_than_half_loaded() {
        let c = ParamCache::new();
        assert!(load(b"", &c).is_err(), "empty");
        assert!(load(b"NOPE\x01\x00\x00\x00", &c).is_err(), "bad magic");

        let mut future = save(&c);
        future[4..6].copy_from_slice(&(VERSION + 1).to_le_bytes());
        assert!(load(&future, &c).is_err(), "future version");

        let short = &save(&filled())[..10];
        assert!(load(short, &c).is_err(), "truncated");
        // And a rejected load must not have moved anything.
        for id in 0..TOTAL_PARAMS {
            assert_eq!(
                c.get(id),
                default_value(id),
                "param {id} moved on a failed load"
            );
        }
    }

    /// A blob from a build with a shorter table loads, leaving the params it
    /// never knew about at their defaults.
    #[test]
    fn a_shorter_blob_loads_and_leaves_the_rest_alone() {
        let mut b = Vec::new();
        b.extend_from_slice(&MAGIC);
        b.extend_from_slice(&VERSION.to_le_bytes());
        b.extend_from_slice(&3u16.to_le_bytes());
        for v in [4.0f32, 1.0, 0.5] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let c = ParamCache::new();
        load(&b, &c).expect("short blob should load");
        assert_eq!(c.get(0), 4.0);
        assert_eq!(c.get(1), 1.0);
        assert_eq!(c.get(2), 0.5);
        for m in 0..vxn4_engine::N_MACROS {
            assert_eq!(c.get(N_FIXED + m), 0.0, "macro {m} should be untouched");
        }
    }

    /// A blob from a build with a longer table loads, ignoring the tail rather
    /// than desynchronising on it.
    #[test]
    fn a_longer_blob_loads_and_ignores_the_tail() {
        let mut b = save(&filled());
        let extra = 4usize;
        b[6..8].copy_from_slice(&((TOTAL_PARAMS + extra) as u16).to_le_bytes());
        for _ in 0..extra {
            b.extend_from_slice(&9.5f32.to_le_bytes());
        }
        let c = ParamCache::new();
        load(&b, &c).expect("long blob should load");
        assert_eq!(c.get(0), 3.0);
        assert_eq!(c.get(TOTAL_PARAMS - 1), filled().get(TOTAL_PARAMS - 1));
    }

    /// A corrupted value cannot become an out-of-range patch index.
    #[test]
    fn a_corrupt_value_is_clamped_not_trusted() {
        let mut b = save(&ParamCache::new());
        b[8..12].copy_from_slice(&1e9f32.to_le_bytes());
        let c = ParamCache::new();
        load(&b, &c).expect("load");
        assert_eq!(c.get(0), (vxn4_engine::N_PATCHES - 1) as f32);
    }
}
