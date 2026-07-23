//! User-preset filesystem IO + the [`PresetStore`] impl (ADR 0005 §5/§6,
//! ADR 0006).
//!
//! Resolves the per-OS **user** preset directory and provides the file ops
//! the browser needs: load/save a preset, enumerate one level of subfolders,
//! create / rename / delete folders, and rename / delete / move a user
//! preset. All **main/UI-thread** work — the audio thread never touches the
//! filesystem or serde.
//!
//! The factory bank is embedded, not on disk ([`crate::factory`]); the
//! mutating ops here are the writable user side only. Every mutating call
//! canonicalises its target path and refuses anything outside the user dir
//! ([`ensure_within_user_dir`]) — defence in depth, since the UI never
//! *should* hand it a bad path.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use vxn_core_app::{
    PresetLoad, PresetMeta, PresetStore, UserFolderEntry, UserPresetEntry,
};

use crate::factory::factory;
use crate::preset::{Meta, from_toml_str, read_preset, write_preset};

/// The per-OS directory VXN2 reads and writes user presets in. `None` only if
/// the platform's home/appdata environment variable is unset.
#[cfg(target_os = "macos")]
pub fn user_preset_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join("Library/Audio/Presets/Vulpus Labs/VXN2"))
}

#[cfg(target_os = "windows")]
pub fn user_preset_dir() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(
        Path::new(&appdata)
            .join("Vulpus Labs")
            .join("VXN2")
            .join("Presets"),
    )
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn user_preset_dir() -> Option<PathBuf> {
    // `$XDG_DATA_HOME/VXN2/presets`, falling back to `~/.local/share/VXN2/presets`.
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return Some(Path::new(&xdg).join("VXN2").join("presets"));
        }
    }
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join(".local/share/VXN2/presets"))
}

fn no_dir_err() -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        "no user preset directory for this platform",
    )
}

/// Resolve and create the user preset directory (idempotent).
pub fn ensure_user_dir() -> io::Result<PathBuf> {
    let dir = user_preset_dir().ok_or_else(no_dir_err)?;
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Sanitise a folder or preset display name: keep alphanumerics, space, `-`,
/// `_`; map everything else (path separators, `.`, etc.) to `_`. Trim; empty
/// → `"Untitled"`. Folder names and preset filenames share this rule so they
/// can't drift.
fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "Untitled".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Preset filename derived from the display name (`<sanitized>.toml`).
fn preset_filename(name: &str) -> String {
    format!("{}.toml", sanitize_name(name))
}

/// Canonicalise the target path against the user dir and refuse anything that
/// lands outside it. Targets that don't exist yet (Save-As, rename
/// destination, new folder) fall back to canonicalising the parent and
/// rejoining the filename.
fn ensure_within_user_dir(target: &Path) -> io::Result<()> {
    let base = ensure_user_dir()?;
    let canon_base = fs::canonicalize(&base).unwrap_or(base);
    let canon_target = if target.exists() {
        fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf())
    } else if let Some(parent) = target.parent() {
        let canon_parent = fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
        match target.file_name() {
            Some(name) => canon_parent.join(name),
            None => canon_parent,
        }
    } else {
        target.to_path_buf()
    };
    if !canon_target.starts_with(&canon_base) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "preset path outside user directory",
        ));
    }
    Ok(())
}

/// Save `(meta, blob)` under the user-root (`folder = None`) or into the
/// named subfolder (creating it if missing). The filename is derived from
/// `meta.name`.
fn save_preset_in(meta: &Meta, blob: &[u8], folder: Option<&str>) -> io::Result<PathBuf> {
    let base = ensure_user_dir()?;
    let dir = match folder {
        Some(name) => base.join(sanitize_name(name)),
        None => base,
    };
    let path = dir.join(preset_filename(&meta.name));
    ensure_within_user_dir(&path)?;
    fs::create_dir_all(&dir)?;
    let text = write_preset(meta, blob).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(&path, text)?;
    Ok(path)
}

/// Read a preset file's display name (only — no blob built). `None` if the
/// file isn't a parseable `*.toml` preset.
fn read_display_name(path: &Path) -> Option<String> {
    if path.extension().and_then(|e| e.to_str()) != Some("toml") {
        return None;
    }
    let contents = fs::read_to_string(path).ok()?;
    let (meta, _values, _matrix, _curves, _eg_curves, _warnings) = read_preset(&contents).ok()?;
    Some(meta.name)
}

/// Walk the user directory one level deep: root group first (`name == None`),
/// then each subfolder alpha-sorted. Files that don't parse are skipped.
fn list_user_tree_io() -> io::Result<Vec<UserFolderEntry>> {
    let Some(base) = user_preset_dir() else {
        return Ok(Vec::new());
    };
    if !base.exists() {
        return Ok(Vec::new());
    }

    let mut root: Vec<UserPresetEntry> = Vec::new();
    let mut subfolders: Vec<(String, Vec<UserPresetEntry>)> = Vec::new();

    for entry in fs::read_dir(&base)? {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_file() {
            if let Some(name) = read_display_name(&path) {
                root.push(UserPresetEntry {
                    path,
                    meta: PresetMeta { name, ..Default::default() },
                    folder: None,
                });
            }
        } else if ft.is_dir() {
            let Some(folder_name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let mut presets = Vec::new();
            for sub in fs::read_dir(&path)? {
                let sub = sub?;
                if sub.file_type()?.is_file() {
                    if let Some(name) = read_display_name(&sub.path()) {
                        presets.push(UserPresetEntry {
                            path: sub.path(),
                            meta: PresetMeta { name, ..Default::default() },
                            folder: Some(folder_name.clone()),
                        });
                    }
                }
            }
            presets.sort_by(|a, b| a.meta.name.to_lowercase().cmp(&b.meta.name.to_lowercase()));
            subfolders.push((folder_name, presets));
        }
    }
    root.sort_by(|a, b| a.meta.name.to_lowercase().cmp(&b.meta.name.to_lowercase()));
    subfolders.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

    let mut out = Vec::with_capacity(1 + subfolders.len());
    out.push(UserFolderEntry { name: None, presets: root });
    for (name, presets) in subfolders {
        out.push(UserFolderEntry { name: Some(name), presets });
    }
    Ok(out)
}

fn existing_folder_names_ci(base: &Path) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    if base.exists() {
        for entry in fs::read_dir(base)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                if let Some(n) = entry.file_name().to_str() {
                    names.push(n.to_lowercase());
                }
            }
        }
    }
    Ok(names)
}

fn unique_folder_name(stem: &str, existing_ci: &[String]) -> String {
    let stem_l = stem.to_lowercase();
    if !existing_ci.iter().any(|e| e == &stem_l) {
        return stem.to_string();
    }
    let mut n = 1;
    loop {
        let candidate = format!("{stem} {n}");
        if !existing_ci.iter().any(|e| e == &candidate.to_lowercase()) {
            return candidate;
        }
        n += 1;
    }
}

/// The concrete [`PresetStore`] for VXN2: an embedded factory bank plus
/// user-dir TOML IO. Stateless — every call goes straight to the module
/// functions and the [`factory`] bank.
pub struct Vxn2PresetStore;

impl Vxn2PresetStore {
    pub fn new() -> Self {
        Self
    }
}

impl Default for Vxn2PresetStore {
    fn default() -> Self {
        Self::new()
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

fn load_from_toml(contents: &str) -> Result<PresetLoad, String> {
    let (meta, blob, warnings) = from_toml_str(contents).map_err(|e| e.to_string())?;
    Ok(PresetLoad {
        meta: meta_to_app(&meta),
        blob,
        warnings,
    })
}

impl PresetStore for Vxn2PresetStore {
    fn factory_len(&self) -> usize {
        factory().len()
    }

    fn factory_load(&self, index: usize) -> Result<PresetLoad, String> {
        let bank = factory();
        let fp = bank.get(index).ok_or("factory index out of range")?;
        load_from_toml(fp.contents)
    }

    fn factory_meta(&self, index: usize) -> Option<PresetMeta> {
        factory().get(index).map(|fp| PresetMeta {
            // The browser groups factory presets by directory category, not
            // the optional `[meta] category` — override so the published
            // corpus carries the directory grouping.
            name: fp.name.clone(),
            author: fp.meta.author.clone(),
            category: Some(fp.category.clone()),
            comment: fp.meta.comment.clone(),
        })
    }

    fn user_load(&self, path: &Path) -> Result<PresetLoad, String> {
        let contents = fs::read_to_string(path).map_err(|e| e.to_string())?;
        load_from_toml(&contents)
    }

    fn user_save(
        &self,
        name: &str,
        folder: Option<&str>,
        meta: &PresetMeta,
        blob: &[u8],
    ) -> Result<PathBuf, String> {
        let m = Meta {
            name: name.to_string(),
            author: meta.author.clone(),
            category: meta.category.clone(),
            comment: meta.comment.clone(),
        };
        save_preset_in(&m, blob, folder).map_err(|e| e.to_string())
    }

    fn user_delete(&self, path: &Path) -> Result<(), String> {
        ensure_within_user_dir(path).map_err(|e| e.to_string())?;
        fs::remove_file(path).map_err(|e| e.to_string())
    }

    fn user_rename(&self, path: &Path, new_name: &str) -> Result<PathBuf, String> {
        rename_user_preset(path, new_name).map_err(|e| e.to_string())
    }

    fn user_move(&self, path: &Path, dest_folder: Option<&str>) -> Result<PathBuf, String> {
        move_user_preset(path, dest_folder).map_err(|e| e.to_string())
    }

    fn user_create_folder(&self, suggested: &str) -> Result<(PathBuf, String), String> {
        create_user_folder(suggested).map_err(|e| e.to_string())
    }

    fn user_rename_folder(&self, old: &str, new: &str) -> Result<(PathBuf, String), String> {
        rename_user_folder(old, new).map_err(|e| e.to_string())
    }

    fn user_delete_folder(&self, name: &str) -> Result<(), String> {
        let base = ensure_user_dir().map_err(|e| e.to_string())?;
        let path = base.join(sanitize_name(name));
        ensure_within_user_dir(&path).map_err(|e| e.to_string())?;
        fs::remove_dir_all(&path).map_err(|e| e.to_string())
    }

    fn list_user_tree(&self) -> Vec<UserFolderEntry> {
        list_user_tree_io().unwrap_or_default()
    }
}

// Mutating user ops (io::Result; the trait wraps to String).

fn create_user_folder(suggested: &str) -> io::Result<(PathBuf, String)> {
    let base = ensure_user_dir()?;
    let stem = sanitize_name(suggested);
    let existing = existing_folder_names_ci(&base)?;
    let name = unique_folder_name(&stem, &existing);
    let path = base.join(&name);
    ensure_within_user_dir(&path)?;
    fs::create_dir(&path)?;
    Ok((path, name))
}

fn rename_user_folder(old: &str, new: &str) -> io::Result<(PathBuf, String)> {
    let base = ensure_user_dir()?;
    let old_path = base.join(sanitize_name(old));
    let new_name = sanitize_name(new);
    let new_path = base.join(&new_name);
    ensure_within_user_dir(&old_path)?;
    ensure_within_user_dir(&new_path)?;
    if old_path == new_path {
        return Ok((new_path, new_name));
    }
    if new_path.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "folder already exists"));
    }
    fs::rename(&old_path, &new_path)?;
    Ok((new_path, new_name))
}

fn move_user_preset(path: &Path, dest_folder: Option<&str>) -> io::Result<PathBuf> {
    ensure_within_user_dir(path)?;
    let base = ensure_user_dir()?;
    let dest_dir = match dest_folder {
        Some(name) => base.join(sanitize_name(name)),
        None => base,
    };
    let filename = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "preset has no filename"))?;
    let new_path = dest_dir.join(filename);
    ensure_within_user_dir(&new_path)?;
    if new_path == path {
        return Ok(new_path);
    }
    if new_path.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "destination already exists"));
    }
    fs::create_dir_all(&dest_dir)?;
    fs::rename(path, &new_path)?;
    Ok(new_path)
}

fn rename_user_preset(path: &Path, new_name: &str) -> io::Result<PathBuf> {
    ensure_within_user_dir(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "preset has no parent"))?;
    let new_path = parent.join(preset_filename(new_name));
    ensure_within_user_dir(&new_path)?;
    if new_path != path && new_path.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "preset already exists"));
    }
    let contents = fs::read_to_string(path)?;
    let (mut meta, blob, _w) =
        from_toml_str(&contents).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    meta.name = new_name.to_string();
    let text = write_preset(&meta, &blob).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(&new_path, text)?;
    if new_path != path {
        fs::remove_file(path)?;
    }
    Ok(new_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::{ParamModel, SharedParams};

    #[test]
    fn filename_is_sanitized() {
        assert_eq!(preset_filename("Sync Lead"), "Sync Lead.toml");
        assert_eq!(preset_filename("a/b:c*?"), "a_b_c__.toml");
        assert_eq!(preset_filename("   "), "Untitled.toml");
    }

    #[test]
    fn unique_folder_name_suffixes_collisions() {
        assert_eq!(unique_folder_name("Pads", &[]), "Pads");
        let existing = vec!["pads".to_string()];
        assert_eq!(unique_folder_name("Pads", &existing), "Pads 1");
        let existing = vec!["pads".to_string(), "pads 1".to_string()];
        assert_eq!(unique_folder_name("Pads", &existing), "Pads 2");
    }

    #[test]
    fn factory_store_loads_every_preset() {
        let store = Vxn2PresetStore::new();
        let n = store.factory_len();
        assert!(n >= 5, "expected >= 5 factory presets, got {n}");
        for i in 0..n {
            let load = store.factory_load(i).expect("factory load");
            assert!(!load.meta.name.is_empty());
            assert!(load.warnings.is_empty(), "{:?}", load.warnings);
            // Blob applies cleanly to a model.
            let sp = SharedParams::new();
            ParamModel::load_bytes(&sp, &load.blob).expect("blob loads");
            // Every factory preset carries a directory category.
            assert!(store.factory_meta(i).unwrap().category.is_some());
        }
    }
}
