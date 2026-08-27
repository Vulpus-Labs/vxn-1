//! Single user-preset record codec (E019 / 0063).
//!
//! The web browser-storage store keeps user presets as one value per preset in
//! IndexedDB (the binary-blob decision, ADR 0009 addendum): the canonical
//! [`crate::state`] blob plus the view-facing [`PresetMeta`]. This module owns
//! that value's wire format so the controller-side writer (the deferred-write
//! journal, 0064) and reader (boot hydration, 0064) cannot drift. The synthetic
//! folder/name *path* is the IndexedDB key, kept outside the value.
//!
//! Layout (little-endian): `magic b"VXUR" + version u32 + blob(len u32; bytes) +
//! name(len u32; utf8) + opt author + opt category + opt comment` (opt = `u8`
//! present flag then the string when present).

use crate::PresetMeta;

/// Record magic; first four bytes of a user-preset value.
pub const MAGIC: [u8; 4] = *b"VXUR";
/// Record format version. Bump on any layout change.
pub const VERSION: u32 = 1;

/// One user preset as stored: its meta and the canonical state blob.
#[derive(Clone, Debug, PartialEq)]
pub struct PresetRecord {
    pub meta: PresetMeta,
    pub blob: Vec<u8>,
}

/// Serialize a user-preset record (the IndexedDB value).
pub fn encode(rec: &PresetRecord) -> Vec<u8> {
    let mut out = Vec::with_capacity(32 + rec.blob.len());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    push_bytes(&mut out, &rec.blob);
    push_str(&mut out, &rec.meta.name);
    push_opt(&mut out, rec.meta.author.as_deref());
    push_opt(&mut out, rec.meta.category.as_deref());
    push_opt(&mut out, rec.meta.comment.as_deref());
    out
}

/// Parse a user-preset record. Rejects a bad magic/version or truncation.
pub fn decode(bytes: &[u8]) -> Result<PresetRecord, String> {
    let mut c = Cursor { b: bytes, off: 0 };
    if c.take(4)? != MAGIC {
        return Err("preset record: bad magic".into());
    }
    if c.u32()? != VERSION {
        return Err("preset record: unsupported version".into());
    }
    let blob = c.bytes()?.to_vec();
    let name = c.string()?;
    let author = c.opt_string()?;
    let category = c.opt_string()?;
    let comment = c.opt_string()?;
    Ok(PresetRecord {
        meta: PresetMeta {
            name,
            author,
            category,
            comment,
        },
        blob,
    })
}

fn push_bytes(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u32).to_le_bytes());
    out.extend_from_slice(b);
}
fn push_str(out: &mut Vec<u8>, s: &str) {
    push_bytes(out, s.as_bytes());
}
fn push_opt(out: &mut Vec<u8>, s: Option<&str>) {
    match s {
        Some(s) => {
            out.push(1);
            push_str(out, s);
        }
        None => out.push(0),
    }
}

struct Cursor<'a> {
    b: &'a [u8],
    off: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.off.checked_add(n).ok_or("preset record: length overflow")?;
        if end > self.b.len() {
            return Err("preset record: truncated".into());
        }
        let s = &self.b[self.off..end];
        self.off = end;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32, String> {
        let s = self.take(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn bytes(&mut self) -> Result<&'a [u8], String> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    fn string(&mut self) -> Result<String, String> {
        let s = self.bytes()?;
        String::from_utf8(s.to_vec()).map_err(|_| "preset record: bad utf-8".into())
    }
    fn opt_string(&mut self) -> Result<Option<String>, String> {
        match self.take(1)?[0] {
            0 => Ok(None),
            _ => Ok(Some(self.string()?)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let rec = PresetRecord {
            meta: PresetMeta {
                name: "Mini Bass".into(),
                author: Some("VL".into()),
                category: None,
                comment: Some("punchy".into()),
            },
            blob: vec![9, 8, 7, 0, 255],
        };
        assert_eq!(decode(&encode(&rec)).unwrap(), rec);
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(&[0u8; 8]).is_err());
        let ok = encode(&PresetRecord {
            meta: PresetMeta::default(),
            blob: vec![1, 2],
        });
        assert!(decode(&ok[..ok.len() - 1]).is_err());
    }
}
