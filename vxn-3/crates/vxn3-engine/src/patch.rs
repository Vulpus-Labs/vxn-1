//! Shared little-endian byte reader for per-engine **deep-patch** deserialization
//! (0179 / ADR 0005).
//!
//! Each engine serializes its patch field-explicit — a leading `u8` patch-version
//! tag, then the patch fields as LE `f32` — via `Vec::extend_from_slice`, and reads
//! it back through [`PatchReader`]. The reader returns `Err(())` on underrun so a
//! truncated blob is **rejected** rather than silently zero-filled. Deserialization
//! runs on the main thread (before the engine is handed to the audio thread over
//! [`crate::swap`]), so it may allocate / cook freely.
//!
//! The byte layout mirrors the outer `clap.state` blob's discipline (field-explicit,
//! reviewable, stable) precisely because these bytes *are* a flavour's base vector —
//! the voice-roster epic (E034) persists the same format.

/// Minimal forward LE reader over a patch byte slice. `Err(())` on underrun.
/// Crate-internal — only the roster engines read patches.
pub(crate) struct PatchReader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> PatchReader<'a> {
    pub(crate) fn new(b: &'a [u8]) -> Self {
        Self { b, pos: 0 }
    }

    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8], ()> {
        let end = self.pos.checked_add(n).ok_or(())?;
        let s = self.b.get(self.pos..end).ok_or(())?;
        self.pos = end;
        Ok(s)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, ()> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn f32(&mut self) -> Result<f32, ()> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
}
