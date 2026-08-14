//! User-preset filesystem IO + the [`vxn_core_app::PresetStore`] adapter (E038).
//!
//! Resolves the per-OS **user** preset directory and provides the file ops the
//! browser needs: load/save a preset, enumerate one level of subfolders, create
//! / rename / delete folders, and rename / delete / move a user preset. All
//! **main/UI-thread** work — the audio thread never touches the filesystem or
//! serde.
//!
//! Ported from VXN1's `vxn-engine/src/preset_io.rs`, adapted to VXN1b's sparse
//! TOML codec ([`crate::preset`]) and its `(Meta, PluginState)` shape (VXN1b has
//! no `Performance` wrapper — meta + state serialise together via
//! [`crate::preset::write_preset`]). The **factory** side needs no IO at all —
//! that bank is baked into the binary ([`crate::factory`], 0212). Every mutating
//! call canonicalises its target path and refuses anything outside the user dir
//! ([`ensure_within_user_dir`]).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use vxn_core_app::{PresetLoad, PresetMeta, PresetStore, UserFolderEntry, UserPresetEntry};

use crate::preset::{Meta, PresetError, read_preset, write_preset};
use crate::state::PluginState;

// ── Name-sanitisation ─────────────────────────────────────────────────────────
//
// Local copies (matching VXN1's `vxn-app` and VXN2's `vxn2-engine` rules) so a
// preset filename / folder name can't drift between backends. Kept in-crate
// rather than depending on VXN1's `vxn-app` at runtime.

/// Sanitise a display name into a filesystem-safe stem (alphanumerics, space,
/// `-`, `_`; everything else → `_`). Empty → `"Untitled"`.
pub fn sanitize_name(name: &str) -> String {
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
pub fn preset_filename(name: &str) -> String {
    format!("{}.toml", sanitize_name(name))
}

/// Pick a folder name that doesn't collide (case-insensitively) with any in
/// `existing_ci`, suffixing ` 1`, ` 2`, … on collision.
pub fn unique_folder_name(stem: &str, existing_ci: &[String]) -> String {
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

/// The per-OS directory VXN1b reads and writes user presets in. Distinct from
/// VXN1's dir (`.../VXN1`) so the two synths' banks never collide. `None` only
/// if the platform's home/appdata environment variable is unset.
#[cfg(target_os = "macos")]
pub fn user_preset_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join("Library/Audio/Presets/Vulpus Labs/VXN1b"))
}

#[cfg(target_os = "windows")]
pub fn user_preset_dir() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(
        Path::new(&appdata)
            .join("Vulpus Labs")
            .join("VXN1b")
            .join("Presets"),
    )
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn user_preset_dir() -> Option<PathBuf> {
    // `$XDG_DATA_HOME/VXN1b/presets`, falling back to `~/.local/share/VXN1b/presets`.
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return Some(Path::new(&xdg).join("VXN1b").join("presets"));
        }
    }
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join(".local/share/VXN1b/presets"))
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

/// Canonicalise the target path against the user dir and refuse anything that
/// lands outside it. Targets that don't exist yet (Save-As, rename dest, new
/// folder) fall back to canonicalising the parent and rejoining the filename.
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

// ── PresetStore adapter ───────────────────────────────────────────────────────
//
// The controller talks to preset IO through `vxn_core_app::PresetStore`; this
// is the engine-side adapter. Stateless — the user calls go straight to the
// module functions below, the factory calls to the embedded bank.

pub struct EnginePresetStore;

impl EnginePresetStore {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EnginePresetStore {
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

/// Turn a parsed preset into the controller's byte-channel load. The `blob` is
/// the canonical two-layer `clap.state` format the model restores from
/// ([`crate::SharedParams::restore_from_bytes`]).
///
/// Since 0221 a preset carries the whole [`PluginState`] — both layers and the
/// keyboard record — so this is a straight re-encode. A single-layer file has
/// already been lifted into Layer 1 + factory Layer 2 + `Single` by the codec.
fn to_load(meta: Meta, state: PluginState, warnings: Vec<String>) -> Result<PresetLoad, String> {
    let mut blob = Vec::with_capacity(256);
    state.write(&mut blob).map_err(|e| e.to_string())?;
    Ok(PresetLoad {
        meta: meta_to_app(&meta),
        blob,
        warnings,
    })
}

impl PresetStore for EnginePresetStore {
    fn factory_len(&self) -> usize {
        crate::factory::factory().len()
    }

    fn factory_load(&self, index: usize) -> Result<PresetLoad, String> {
        let (meta, state, warnings) =
            crate::factory::load(index).ok_or_else(|| format!("no factory preset {index}"))?;
        to_load(meta, state, warnings)
    }

    fn factory_meta(&self, index: usize) -> Option<PresetMeta> {
        crate::factory::factory().get(index).map(|p| meta_to_app(&p.meta))
    }

    fn user_load(&self, path: &Path) -> Result<PresetLoad, String> {
        let (meta, state, warnings) = load_preset_file(path).map_err(|e| e.to_string())?;
        to_load(meta, state, warnings)
    }

    fn user_save(
        &self,
        name: &str,
        folder: Option<&str>,
        meta: &PresetMeta,
        blob: &[u8],
    ) -> Result<PathBuf, String> {
        // A preset captures the whole plugin state: both layers plus the
        // keyboard record (0221). The codec drops the sections that are still at
        // their defaults, so a single-layer patch still saves as a single-layer
        // file.
        let state = PluginState::read(&mut &blob[..]).map_err(|e| e.to_string())?;
        let file_meta = Meta {
            name: name.to_string(),
            author: meta.author.clone(),
            category: meta.category.clone(),
            comment: meta.comment.clone(),
        };
        save_preset_in(&file_meta, &state, folder).map_err(|e| e.to_string())
    }

    fn user_delete(&self, path: &Path) -> Result<(), String> {
        delete_user_preset(path).map_err(|e| e.to_string())
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
        delete_user_folder(name).map_err(|e| e.to_string())
    }

    fn list_user_tree(&self) -> Vec<UserFolderEntry> {
        list_user_tree()
            .unwrap_or_default()
            .into_iter()
            .map(|f| UserFolderEntry {
                name: f.name.clone(),
                presets: f
                    .presets
                    .into_iter()
                    .map(|p| UserPresetEntry {
                        path: p.path,
                        meta: PresetMeta {
                            name: p.name,
                            ..Default::default()
                        },
                        folder: p.folder,
                    })
                    .collect(),
            })
            .collect()
    }
}

/// Save a preset (meta + state) under the user-root (`folder = None`) or into
/// the named subfolder (creating it if missing). The filename derives from
/// `meta.name`.
pub fn save_preset_in(meta: &Meta, state: &PluginState, folder: Option<&str>) -> io::Result<PathBuf> {
    let base = ensure_user_dir()?;
    let dir = match folder {
        Some(name) => base.join(sanitize_name(name)),
        None => base,
    };
    let path = dir.join(preset_filename(&meta.name));
    ensure_within_user_dir(&path)?;
    fs::create_dir_all(&dir)?;
    let text = write_preset(meta, state).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(&path, text)?;
    Ok(path)
}

/// A user preset on disk, for the browser's listing.
#[derive(Clone, Debug)]
pub struct UserPreset {
    pub path: PathBuf,
    pub name: String,
    /// `None` = root group; `Some(name)` = subdirectory.
    pub folder: Option<String>,
}

/// One folder's worth of user presets. `name == None` is the virtual root.
#[derive(Clone, Debug)]
pub struct UserFolder {
    pub name: Option<String>,
    pub presets: Vec<UserPreset>,
}

/// Walk one level deep: root group first, then each subfolder alpha-sorted.
/// Empty subfolders are kept. Files that don't parse are skipped silently.
pub fn list_user_tree() -> io::Result<Vec<UserFolder>> {
    let Some(base) = user_preset_dir() else {
        return Ok(Vec::new());
    };
    if !base.exists() {
        return Ok(Vec::new());
    }

    let mut root_presets = Vec::new();
    let mut subfolders: Vec<(String, Vec<UserPreset>)> = Vec::new();

    for entry in fs::read_dir(&base)? {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_file() {
            if let Some(p) = read_preset_at(&path, None) {
                root_presets.push(p);
            }
        } else if ft.is_dir() {
            let Some(folder_name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let mut presets = Vec::new();
            for sub in fs::read_dir(&path)? {
                let sub = sub?;
                if sub.file_type()?.is_file() {
                    if let Some(p) = read_preset_at(&sub.path(), Some(folder_name.clone())) {
                        presets.push(p);
                    }
                }
            }
            presets.sort_by_key(|p| p.name.to_lowercase());
            subfolders.push((folder_name, presets));
        }
    }
    root_presets.sort_by_key(|p| p.name.to_lowercase());
    subfolders.sort_by_key(|a| a.0.to_lowercase());

    let mut out = Vec::with_capacity(1 + subfolders.len());
    out.push(UserFolder {
        name: None,
        presets: root_presets,
    });
    for (name, presets) in subfolders {
        out.push(UserFolder {
            name: Some(name),
            presets,
        });
    }
    Ok(out)
}

fn read_preset_at(path: &Path, folder: Option<String>) -> Option<UserPreset> {
    if path.extension().and_then(|e| e.to_str()) != Some("toml") {
        return None;
    }
    let contents = fs::read_to_string(path).ok()?;
    let (meta, _state, _warnings) = read_preset(&contents).ok()?;
    Some(UserPreset {
        path: path.to_path_buf(),
        name: meta.name,
        folder,
    })
}

/// Create a new user subfolder with a unique name. Returns `(path, chosen_name)`.
pub fn create_user_folder(suggested: &str) -> io::Result<(PathBuf, String)> {
    let base = ensure_user_dir()?;
    let stem = sanitize_name(suggested);
    let existing = existing_folder_names_ci(&base)?;
    let name = unique_folder_name(&stem, &existing);
    let path = base.join(&name);
    ensure_within_user_dir(&path)?;
    fs::create_dir(&path)?;
    Ok((path, name))
}

/// Rename an existing user subfolder. Refuses to overwrite an existing
/// destination. Returns `(new_path, sanitised_new_name)`.
pub fn rename_user_folder(old: &str, new: &str) -> io::Result<(PathBuf, String)> {
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
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "folder already exists",
        ));
    }
    fs::rename(&old_path, &new_path)?;
    Ok((new_path, new_name))
}

/// Delete a user subfolder and everything in it (recursive).
pub fn delete_user_folder(name: &str) -> io::Result<()> {
    let base = ensure_user_dir()?;
    let path = base.join(sanitize_name(name));
    ensure_within_user_dir(&path)?;
    fs::remove_dir_all(&path)
}

/// Delete a user preset file. Refuses paths outside the user directory.
pub fn delete_user_preset(path: &Path) -> io::Result<()> {
    ensure_within_user_dir(path)?;
    fs::remove_file(path)
}

/// Move a user preset into the named subfolder (or back to the root with
/// `dest_folder = None`). The on-disk filename is preserved.
pub fn move_user_preset(path: &Path, dest_folder: Option<&str>) -> io::Result<PathBuf> {
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
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "destination already exists",
        ));
    }
    fs::create_dir_all(&dest_dir)?;
    fs::rename(path, &new_path)?;
    Ok(new_path)
}

/// Rename a user preset: load → mutate `meta.name` → write under the new
/// filename → remove the old. The parent directory is unchanged.
pub fn rename_user_preset(path: &Path, new_name: &str) -> io::Result<PathBuf> {
    ensure_within_user_dir(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "preset has no parent"))?;
    let new_path = parent.join(preset_filename(new_name));
    ensure_within_user_dir(&new_path)?;
    if new_path != path && new_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "preset already exists",
        ));
    }
    let (mut meta, state, warnings) = load_preset_file(path).map_err(load_err_to_io)?;
    if !warnings.is_empty() {
        eprintln!(
            "vxn1b: rename_user_preset({}): {} parse warning(s): {}",
            path.display(),
            warnings.len(),
            warnings.join("; ")
        );
    }
    meta.name = new_name.to_string();
    let text = write_preset(&meta, &state).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(&new_path, text)?;
    if new_path != path {
        fs::remove_file(path)?;
    }
    Ok(new_path)
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

/// Why a preset file failed to load.
#[derive(Debug)]
pub enum LoadError {
    Io(io::Error),
    Parse(PresetError),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io(e) => write!(f, "reading preset file: {e}"),
            LoadError::Parse(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LoadError {}

fn load_err_to_io(e: LoadError) -> io::Error {
    match e {
        LoadError::Io(e) => e,
        LoadError::Parse(e) => io::Error::new(io::ErrorKind::InvalidData, e.to_string()),
    }
}

/// Read and parse a single preset file into `(meta, state, warnings)`.
pub fn load_preset_file(path: &Path) -> Result<(Meta, PluginState, Vec<String>), LoadError> {
    let contents = fs::read_to_string(path).map_err(LoadError::Io)?;
    read_preset(&contents).map_err(LoadError::Parse)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The browser reaches the factory bank through `PresetStore`, not through
    /// [`crate::factory`] directly — so exercise that seam. Every entry must
    /// enumerate, expose meta, and decode to a blob `restore_from_bytes` accepts
    /// (0212's "loads in the browser" criterion, minus the webview).
    #[test]
    fn the_factory_bank_loads_through_the_store() {
        let store = EnginePresetStore::new();
        let n = store.factory_len();
        assert!(n > 0, "the browser would show an empty factory side");

        for i in 0..n {
            let meta = store.factory_meta(i).expect("meta for every index");
            assert!(!meta.name.is_empty(), "factory preset {i} has no name");
            assert!(meta.category.is_some(), "factory preset `{}` has no category", meta.name);

            let load = store
                .factory_load(i)
                .unwrap_or_else(|e| panic!("factory preset `{}` failed to load: {e}", meta.name));
            assert!(load.warnings.is_empty(), "`{}` warned: {:?}", meta.name, load.warnings);
            assert_eq!(load.meta.name, meta.name, "meta and load disagree at index {i}");

            // The blob is what the model actually restores from.
            let sp = crate::SharedParams::new();
            sp.restore_from_bytes(&load.blob)
                .unwrap_or_else(|e| panic!("`{}` produced an unloadable blob: {e}", meta.name));
        }
    }

    /// Past the end is an error, not a panic or a silent factory-default load.
    #[test]
    fn a_factory_index_past_the_end_is_an_error() {
        let store = EnginePresetStore::new();
        assert!(store.factory_load(store.factory_len()).is_err());
        assert!(store.factory_meta(store.factory_len()).is_none());
    }
}
