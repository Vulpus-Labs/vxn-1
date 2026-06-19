//! Baked factory-bank asset codec (E019 / ticket 0062).
//!
//! The factory presets live in `vxn-engine` (embedded TOML + the preset
//! format). The web *controller* deliberately deps only `vxn-app` (ADR 0009),
//! so it cannot reach the engine bank directly. Instead xtask's web build runs
//! `vxn-engine`'s `bake-factory` bin, which serializes the bank — each preset's
//! [`PresetMeta`] + its canonical state blob (the [`crate::state`] format) —
//! into a flat asset with [`encode`]. The page fetches that asset at boot and
//! feeds the bytes to the controller, which parses them with [`decode`] into a
//! read-only `WebPresetStore`.
//!
//! One module owns the format so the encoder (engine-side) and decoder
//! (controller-side) cannot drift; a round-trip test guards it.
//!
//! Layout (little-endian):
//!
//! ```text
//! magic   : b"VXFB"            (4 bytes — Vxn Factory Bank)
//! version : u32
//! count   : u32
//! per entry:
//!   blob_len : u32 ; blob bytes          (the state blob)
//!   name     : u32 len ; utf-8
//!   author   : u8 present ; (u32 len ; utf-8) if present
//!   category : u8 present ; (u32 len ; utf-8) if present
//!   comment  : u8 present ; (u32 len ; utf-8) if present
//! ```

use crate::PresetMeta;

/// Asset magic; first four bytes of a baked factory bank.
pub const MAGIC: [u8; 4] = *b"VXFB";
/// Asset format version. Bump on any layout change.
pub const VERSION: u32 = 1;

/// One baked factory entry: the view-facing meta and the preset's state blob.
#[derive(Clone, Debug, PartialEq)]
pub struct FactoryEntry {
    pub meta: PresetMeta,
    pub blob: Vec<u8>,
}

/// Serialize the factory bank into the flat asset (engine-side, in `bake-factory`).
pub fn encode(entries: &[FactoryEntry]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for e in entries {
        push_bytes(&mut out, &e.blob);
        push_str(&mut out, &e.meta.name);
        push_opt(&mut out, e.meta.author.as_deref());
        push_opt(&mut out, e.meta.category.as_deref());
        push_opt(&mut out, e.meta.comment.as_deref());
    }
    out
}

/// Parse a baked factory asset (controller-side). Rejects a bad magic/version
/// or a truncated/overlong buffer.
pub fn decode(bytes: &[u8]) -> Result<Vec<FactoryEntry>, String> {
    let mut c = Cursor { b: bytes, off: 0 };
    if c.take(4)? != MAGIC {
        return Err("factory asset: bad magic".into());
    }
    let version = c.u32()?;
    if version != VERSION {
        return Err(format!("factory asset: unsupported version {version}"));
    }
    let count = c.u32()? as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let blob = c.bytes()?.to_vec();
        let name = c.string()?;
        let author = c.opt_string()?;
        let category = c.opt_string()?;
        let comment = c.opt_string()?;
        out.push(FactoryEntry {
            meta: PresetMeta {
                name,
                author,
                category,
                comment,
            },
            blob,
        });
    }
    if c.off != bytes.len() {
        return Err("factory asset: trailing bytes".into());
    }
    Ok(out)
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
        let end = self.off.checked_add(n).ok_or("factory asset: length overflow")?;
        if end > self.b.len() {
            return Err("factory asset: truncated".into());
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
        String::from_utf8(s.to_vec()).map_err(|_| "factory asset: bad utf-8".into())
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
    fn round_trips_entries_with_optional_meta() {
        let entries = vec![
            FactoryEntry {
                meta: PresetMeta {
                    name: "Mini Bass".into(),
                    author: Some("VL".into()),
                    category: Some("Bass".into()),
                    comment: None,
                },
                blob: vec![1, 2, 3, 4, 5],
            },
            FactoryEntry {
                meta: PresetMeta {
                    name: "Pad".into(),
                    author: None,
                    category: Some("Pads".into()),
                    comment: Some("lush".into()),
                },
                blob: vec![],
            },
        ];
        let bytes = encode(&entries);
        assert_eq!(decode(&bytes).unwrap(), entries);
    }

    #[test]
    fn rejects_bad_magic_and_truncation() {
        assert!(decode(&[0u8; 12]).is_err());
        let ok = encode(&[]);
        assert!(decode(&ok[..ok.len() - 1]).is_err());
    }
}
