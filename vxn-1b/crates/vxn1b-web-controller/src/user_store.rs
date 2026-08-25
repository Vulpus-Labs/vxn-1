//! In-memory user-preset store for the VXN1b web port (E045 / 0290).
//!
//! The desktop user side is `std::fs` ([`vxn1b_engine::preset_io`]), which
//! cannot be reused here: on wasm those calls compile to stubs that fail
//! silently rather than erroring. This module is the replacement **storage
//! layer** — a synchronous in-memory cache the
//! [`PresetStore`](vxn_core_app::PresetStore) impl reads and writes, plus a
//! journal of pending persistence ops. Boot hydration (fill the cache from
//! IndexedDB) and the deferred-write flush (drain the journal to IndexedDB) are
//! driven by the C-ABI opcodes in `lib.rs` and, JS-side, by the shared
//! `preset-persistence.mjs` (0284); ticket 0293 wires that half. This layer
//! stays sync and testable.
//!
//! **The record format is the desktop format, verbatim.** Each user preset is
//! stored as the same sparse TOML the plugin writes
//! ([`vxn1b_engine::preset::write_preset`] / [`read_preset`]); the IndexedDB
//! value is the TOML text and the synthetic `folder/Name.toml` path is the key.
//! Name and folder sanitisation come from [`vxn1b_engine::preset_io`] rather
//! than being re-rolled, so the two backends cannot drift on what a preset file
//! contains or what a name means.
//!
//! Ported from `vxn2-web-controller::user_store`. The one structural difference
//! is the codec's shape: vxn-2's `write_preset` takes an opaque blob, VXN1b's
//! takes a [`PluginState`], so [`encode_record`] / [`decode_record`] convert
//! across the `PresetStore` trait's blob boundary — the same conversion
//! [`EnginePresetStore::user_save`](vxn1b_engine::preset_io) does natively.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use vxn1b_engine::preset::{Meta, read_preset, write_preset};
use vxn1b_engine::preset_io::{preset_filename, sanitize_name, unique_folder_name};
use vxn1b_engine::state::PluginState;
use vxn_core_app::{PresetLoad, PresetMeta, UserFolderEntry, UserPresetEntry};

/// A pending persistence op for the deferred-write flush to IndexedDB.
/// `key` is the synthetic preset path; folder ops carry the folder name.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum UserWrite {
    Put { key: String, bytes: Vec<u8> },
    Delete { key: String },
    PutFolder { name: String },
    DeleteFolder { name: String },
}

struct Record {
    meta: PresetMeta,
    blob: Vec<u8>,
    /// `None` = user-dir root; `Some(_)` = subfolder name (sanitised).
    folder: Option<String>,
    /// Warnings from the stored TOML's last parse (unknown keys, clamped
    /// values). Carried so a hydrated preset reports them on load exactly as a
    /// file-backed one does — VXN1b's codec produces real warnings and the
    /// faceplate flashes them.
    warnings: Vec<String>,
}

/// The in-memory user corpus + a journal of unflushed persistence ops.
#[derive(Default)]
pub(crate) struct UserState {
    presets: BTreeMap<String, Record>,
    /// Folders that exist, including empty ones a user just created.
    folders: BTreeSet<String>,
    journal: Vec<UserWrite>,
}

fn meta_to_engine(m: &PresetMeta) -> Meta {
    Meta {
        name: m.name.clone(),
        author: m.author.clone(),
        category: m.category.clone(),
        comment: m.comment.clone(),
    }
}

fn meta_to_app(m: &Meta) -> PresetMeta {
    PresetMeta {
        name: m.name.clone(),
        author: m.author.clone(),
        category: m.category.clone(),
        comment: m.comment.clone(),
    }
}

/// Serialise `(meta, state blob)` to the stored TOML record bytes (the
/// IndexedDB value). `blob` must be a valid [`PluginState`] snapshot — the
/// codec writes from the parsed state, not from the bytes.
pub(crate) fn encode_record(meta: &PresetMeta, blob: &[u8]) -> Result<Vec<u8>, String> {
    let state = PluginState::read(&mut &blob[..]).map_err(|e| e.to_string())?;
    let text = write_preset(&meta_to_engine(meta), &state)?;
    Ok(text.into_bytes())
}

/// Parse a stored TOML record back into `(meta, state blob, warnings)`. The
/// blob is the canonical snapshot, ready for `restore_from_bytes`.
pub(crate) fn decode_record(bytes: &[u8]) -> Result<(PresetMeta, Vec<u8>, Vec<String>), String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "preset record is not UTF-8".to_string())?;
    let (meta, state, warnings) = read_preset(text).map_err(|e| e.to_string())?;
    let mut blob = Vec::with_capacity(256);
    state.write(&mut blob).map_err(|e| e.to_string())?;
    Ok((meta_to_app(&meta), blob, warnings))
}

/// Synthetic key for a preset: `folder/filename`, or just `filename` at root.
fn key_for(folder: Option<&str>, filename: &str) -> String {
    match folder {
        Some(f) => format!("{f}/{filename}"),
        None => filename.to_string(),
    }
}

fn path_str(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| "preset path is not valid UTF-8".to_string())
}

// `take_journal` / `hydrate_*` are the boot-hydration + deferred-write surface,
// driven by the opcodes in lib.rs and the shared `preset-persistence.mjs`.
impl UserState {
    /// Drain the pending persistence ops (the bridge ships them to IndexedDB).
    pub fn take_journal(&mut self) -> Vec<UserWrite> {
        std::mem::take(&mut self.journal)
    }

    /// Insert a hydrated preset (boot path) — fills the cache from IndexedDB
    /// WITHOUT journalling, since it is already persisted. `key` is the stored
    /// synthetic path; the folder is derived from it.
    pub fn hydrate_preset(&mut self, key: &str, meta: PresetMeta, blob: Vec<u8>, warnings: Vec<String>) {
        let folder = key.rsplit_once('/').map(|(f, _)| f.to_string());
        if let Some(f) = &folder {
            self.folders.insert(f.clone());
        }
        self.presets.insert(
            key.to_string(),
            Record {
                meta,
                blob,
                folder,
                warnings,
            },
        );
    }

    /// Register a hydrated (already-persisted) folder, including empties.
    pub fn hydrate_folder(&mut self, name: &str) {
        self.folders.insert(name.to_string());
    }

    pub fn load(&self, path: &Path) -> Result<PresetLoad, String> {
        let key = path_str(path)?;
        let rec = self.presets.get(&key).ok_or("preset not found")?;
        Ok(PresetLoad {
            meta: rec.meta.clone(),
            blob: rec.blob.clone(),
            warnings: rec.warnings.clone(),
        })
    }

    pub fn save(
        &mut self,
        name: &str,
        folder: Option<&str>,
        meta: &PresetMeta,
        blob: &[u8],
    ) -> Result<PathBuf, String> {
        let folder = folder.map(sanitize_name);
        let filename = preset_filename(name);
        let key = key_for(folder.as_deref(), &filename);
        let stored_meta = PresetMeta {
            name: name.to_string(),
            author: meta.author.clone(),
            category: meta.category.clone(),
            comment: meta.comment.clone(),
        };
        // Encode BEFORE mutating so a malformed blob leaves the cache untouched.
        let bytes = encode_record(&stored_meta, blob)?;
        if let Some(f) = &folder {
            self.ensure_folder(f);
        }
        self.presets.insert(
            key.clone(),
            Record {
                meta: stored_meta,
                blob: blob.to_vec(),
                folder,
                // Freshly written from the live model: nothing to warn about.
                warnings: Vec::new(),
            },
        );
        self.journal.push(UserWrite::Put {
            key: key.clone(),
            bytes,
        });
        Ok(PathBuf::from(key))
    }

    pub fn delete(&mut self, path: &Path) -> Result<(), String> {
        let key = path_str(path)?;
        if self.presets.remove(&key).is_none() {
            return Err("preset not found".into());
        }
        self.journal.push(UserWrite::Delete { key });
        Ok(())
    }

    pub fn rename(&mut self, path: &Path, new_name: &str) -> Result<PathBuf, String> {
        let old_key = path_str(path)?;
        let rec = self.presets.get(&old_key).ok_or("preset not found")?;
        let folder = rec.folder.clone();
        let new_filename = preset_filename(new_name);
        let new_key = key_for(folder.as_deref(), &new_filename);
        if new_key != old_key && self.presets.contains_key(&new_key) {
            return Err("preset already exists".into());
        }
        // Re-key with the new display name.
        let mut rec = self.presets.remove(&old_key).unwrap();
        rec.meta.name = new_name.to_string();
        let bytes = encode_record(&rec.meta, &rec.blob)?;
        self.presets.insert(new_key.clone(), rec);
        if new_key != old_key {
            self.journal.push(UserWrite::Delete { key: old_key });
        }
        self.journal.push(UserWrite::Put {
            key: new_key.clone(),
            bytes,
        });
        Ok(PathBuf::from(new_key))
    }

    pub fn move_preset(&mut self, path: &Path, dest_folder: Option<&str>) -> Result<PathBuf, String> {
        let old_key = path_str(path)?;
        if !self.presets.contains_key(&old_key) {
            return Err("preset not found".into());
        }
        let dest = dest_folder.map(sanitize_name);
        // Filename is preserved — only the parent folder changes.
        let filename = old_key
            .rsplit_once('/')
            .map(|(_, f)| f)
            .unwrap_or(&old_key)
            .to_string();
        let new_key = key_for(dest.as_deref(), &filename);
        if new_key == old_key {
            return Ok(PathBuf::from(new_key));
        }
        if self.presets.contains_key(&new_key) {
            return Err("destination already exists".into());
        }
        if let Some(f) = &dest {
            self.ensure_folder(f);
        }
        let mut rec = self.presets.remove(&old_key).unwrap();
        rec.folder = dest;
        let bytes = encode_record(&rec.meta, &rec.blob)?;
        self.presets.insert(new_key.clone(), rec);
        self.journal.push(UserWrite::Delete { key: old_key });
        self.journal.push(UserWrite::Put {
            key: new_key.clone(),
            bytes,
        });
        Ok(PathBuf::from(new_key))
    }

    pub fn create_folder(&mut self, suggested: &str) -> Result<(PathBuf, String), String> {
        let stem = sanitize_name(suggested);
        let existing: Vec<String> = self.folders.iter().map(|f| f.to_lowercase()).collect();
        let name = unique_folder_name(&stem, &existing);
        self.folders.insert(name.clone());
        self.journal.push(UserWrite::PutFolder { name: name.clone() });
        Ok((PathBuf::from(&name), name))
    }

    pub fn rename_folder(&mut self, old: &str, new: &str) -> Result<(PathBuf, String), String> {
        let old_name = sanitize_name(old);
        let new_name = sanitize_name(new);
        if old_name == new_name {
            return Ok((PathBuf::from(&new_name), new_name));
        }
        if self.folders.contains(&new_name) {
            return Err("folder already exists".into());
        }
        if !self.folders.remove(&old_name) {
            return Err("folder not found".into());
        }
        self.folders.insert(new_name.clone());
        self.journal.push(UserWrite::DeleteFolder {
            name: old_name.clone(),
        });
        self.journal.push(UserWrite::PutFolder {
            name: new_name.clone(),
        });
        // Re-key every preset under the old folder.
        let prefix = format!("{old_name}/");
        let moved: Vec<String> = self
            .presets
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();
        for old_key in moved {
            let mut rec = self.presets.remove(&old_key).unwrap();
            let filename = old_key
                .rsplit_once('/')
                .map(|(_, f)| f)
                .unwrap_or(&old_key)
                .to_string();
            let new_key = key_for(Some(&new_name), &filename);
            rec.folder = Some(new_name.clone());
            let bytes = encode_record(&rec.meta, &rec.blob)?;
            self.presets.insert(new_key.clone(), rec);
            self.journal.push(UserWrite::Delete { key: old_key });
            self.journal.push(UserWrite::Put { key: new_key, bytes });
        }
        Ok((PathBuf::from(&new_name), new_name))
    }

    /// Delete a folder and everything in it. Unlike [`Self::rename_folder`] this
    /// does not error on a folder that isn't there: the browser panel can fire a
    /// delete for a group it is already not showing, and the end state is the
    /// same either way.
    pub fn delete_folder(&mut self, name: &str) -> Result<(), String> {
        let folder = sanitize_name(name);
        self.folders.remove(&folder);
        let prefix = format!("{folder}/");
        let removed: Vec<String> = self
            .presets
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();
        for key in removed {
            self.presets.remove(&key);
            self.journal.push(UserWrite::Delete { key });
        }
        self.journal.push(UserWrite::DeleteFolder { name: folder });
        Ok(())
    }

    /// Folder-aware listing: root group first, then folders alpha-sorted, empty
    /// folders kept. Presets alpha-sorted by display name within each group —
    /// the same ordering the desktop `list_user_tree` produces.
    pub fn list_tree(&self) -> Vec<UserFolderEntry> {
        let entry = |key: &str, rec: &Record| UserPresetEntry {
            path: PathBuf::from(key),
            meta: PresetMeta {
                name: rec.meta.name.clone(),
                ..Default::default()
            },
            folder: rec.folder.clone(),
        };

        let mut root: Vec<UserPresetEntry> = self
            .presets
            .iter()
            .filter(|(_, r)| r.folder.is_none())
            .map(|(k, r)| entry(k, r))
            .collect();
        root.sort_by_key(|p| p.meta.name.to_lowercase());

        let mut out = vec![UserFolderEntry {
            name: None,
            presets: root,
        }];

        for folder in &self.folders {
            let mut presets: Vec<UserPresetEntry> = self
                .presets
                .iter()
                .filter(|(_, r)| r.folder.as_deref() == Some(folder.as_str()))
                .map(|(k, r)| entry(k, r))
                .collect();
            presets.sort_by_key(|p| p.meta.name.to_lowercase());
            out.push(UserFolderEntry {
                name: Some(folder.clone()),
                presets,
            });
        }
        out
    }

    fn ensure_folder(&mut self, name: &str) {
        if self.folders.insert(name.to_string()) {
            self.journal.push(UserWrite::PutFolder {
                name: name.to_string(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(name: &str) -> PresetMeta {
        PresetMeta {
            name: name.into(),
            author: None,
            category: Some("Bass".into()),
            comment: None,
        }
    }

    /// A real, restorable state blob: the factory default written through the
    /// same `PluginState` codec `encode_record` / `decode_record` use, so it is
    /// a codec fixpoint. Synthetic raw values on stepped/enum params would
    /// quantise on the TOML round-trip and not compare equal.
    fn real_blob() -> Vec<u8> {
        let mut blob = Vec::new();
        PluginState::factory_default().write(&mut blob).unwrap();
        blob
    }

    #[test]
    fn save_list_load_round_trips() {
        let mut s = UserState::default();
        let blob = real_blob();
        let path = s.save("Mini Bass", None, &meta("Mini Bass"), &blob).unwrap();
        assert_eq!(path, PathBuf::from("Mini Bass.toml"));

        let tree = s.list_tree();
        assert_eq!(tree[0].name, None); // root group first
        assert_eq!(tree[0].presets.len(), 1);
        assert_eq!(tree[0].presets[0].meta.name, "Mini Bass");

        let load = s.load(&path).unwrap();
        assert_eq!(load.blob, blob);
        assert_eq!(load.meta.name, "Mini Bass");
        assert_eq!(load.meta.category.as_deref(), Some("Bass"));
    }

    /// The stored bytes are the desktop format: what the journal ships to
    /// IndexedDB must be parseable by the plugin's own reader, or web and
    /// desktop presets have drifted.
    #[test]
    fn stored_record_is_the_desktop_toml_format() {
        let mut s = UserState::default();
        let blob = real_blob();
        s.save("Web Patch", None, &meta("Web Patch"), &blob).unwrap();
        let ops = s.take_journal();
        let bytes = ops
            .iter()
            .find_map(|o| match o {
                UserWrite::Put { key, bytes } if key == "Web Patch.toml" => Some(bytes.clone()),
                _ => None,
            })
            .expect("a Put for Web Patch.toml");
        let text = std::str::from_utf8(&bytes).expect("record is UTF-8");
        // Parsed by the engine's own reader, not by ours.
        let (m, state, _w) = read_preset(text).expect("desktop reader parses the web record");
        assert_eq!(m.name, "Web Patch");
        assert_eq!(m.category.as_deref(), Some("Bass"));
        let mut round = Vec::new();
        state.write(&mut round).unwrap();
        assert_eq!(round, blob, "state did not survive the TOML round-trip");
    }

    #[test]
    fn save_into_folder_creates_folder_group() {
        let mut s = UserState::default();
        let path = s
            .save("Lead", Some("Leads"), &meta("Lead"), &real_blob())
            .unwrap();
        assert_eq!(path, PathBuf::from("Leads/Lead.toml"));
        let tree = s.list_tree();
        let leads = tree
            .iter()
            .find(|f| f.name.as_deref() == Some("Leads"))
            .unwrap();
        assert_eq!(leads.presets[0].meta.name, "Lead");
    }

    #[test]
    fn rename_rekeys_and_updates_name() {
        let mut s = UserState::default();
        let p = s.save("Old", None, &meta("Old"), &real_blob()).unwrap();
        let np = s.rename(&p, "New").unwrap();
        assert_eq!(np, PathBuf::from("New.toml"));
        assert!(s.load(&p).is_err());
        assert_eq!(s.load(&np).unwrap().meta.name, "New");
    }

    #[test]
    fn rename_refuses_existing_destination() {
        let mut s = UserState::default();
        let a = s.save("A", None, &meta("A"), &real_blob()).unwrap();
        s.save("B", None, &meta("B"), &real_blob()).unwrap();
        assert!(s.rename(&a, "B").is_err());
        assert_eq!(s.load(&a).unwrap().meta.name, "A"); // A survives the refusal
    }

    #[test]
    fn move_changes_folder_keeps_filename() {
        let mut s = UserState::default();
        let blob = real_blob();
        let p = s.save("Pad", None, &meta("Pad"), &blob).unwrap();
        let np = s.move_preset(&p, Some("Pads")).unwrap();
        assert_eq!(np, PathBuf::from("Pads/Pad.toml"));
        assert!(s.load(&p).is_err());
        assert_eq!(s.load(&np).unwrap().blob, blob);
    }

    #[test]
    fn create_folder_uniquifies() {
        let mut s = UserState::default();
        let (_, a) = s.create_folder("Pads").unwrap();
        let (_, b) = s.create_folder("Pads").unwrap();
        assert_eq!(a, "Pads");
        assert_eq!(b, "Pads 1");
    }

    #[test]
    fn rename_folder_rekeys_its_presets() {
        let mut s = UserState::default();
        let blob = real_blob();
        s.save("X", Some("Old"), &meta("X"), &blob).unwrap();
        let (_, new) = s.rename_folder("Old", "New").unwrap();
        assert_eq!(new, "New");
        assert!(s.load(&PathBuf::from("Old/X.toml")).is_err());
        assert_eq!(s.load(&PathBuf::from("New/X.toml")).unwrap().blob, blob);
    }

    #[test]
    fn delete_folder_removes_its_presets() {
        let mut s = UserState::default();
        let p = s.save("Y", Some("Tmp"), &meta("Y"), &real_blob()).unwrap();
        s.delete_folder("Tmp").unwrap();
        assert!(s.load(&p).is_err());
        assert!(s.list_tree().iter().all(|f| f.name.as_deref() != Some("Tmp")));
    }

    /// The journal records persistence ops; hydration replays a stored record
    /// WITHOUT journalling and round-trips the blob.
    #[test]
    fn journal_and_hydration() {
        let mut s = UserState::default();
        let blob = real_blob();
        s.save("Z", Some("F"), &meta("Z"), &blob).unwrap();
        let ops = s.take_journal();
        assert!(ops.iter().any(|o| matches!(o, UserWrite::PutFolder { name } if name == "F")));
        let put = ops
            .iter()
            .find_map(|o| match o {
                UserWrite::Put { key, bytes } if key == "F/Z.toml" => Some(bytes.clone()),
                _ => None,
            })
            .expect("a Put for F/Z.toml");
        assert!(s.take_journal().is_empty(), "journal drained");

        // A fresh store hydrated from that Put reproduces the preset.
        let mut s2 = UserState::default();
        s2.hydrate_folder("F");
        let (m, b, w) = decode_record(&put).unwrap();
        s2.hydrate_preset("F/Z.toml", m, b, w);
        assert!(s2.take_journal().is_empty(), "hydration does not journal");
        assert_eq!(s2.load(&PathBuf::from("F/Z.toml")).unwrap().blob, blob);
    }

    #[test]
    fn save_rejects_malformed_blob_without_mutating() {
        let mut s = UserState::default();
        // A too-short blob is not a valid PluginState; save must fail and leave
        // the cache + journal untouched.
        assert!(s.save("Bad", None, &meta("Bad"), &[1, 2, 3]).is_err());
        assert!(s.list_tree()[0].presets.is_empty());
        assert!(s.take_journal().is_empty());
    }
}
