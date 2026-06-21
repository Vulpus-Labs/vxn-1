//! User-preset filesystem IO (E007 / 0026, E008 / 0029, ADR 0005 §5, ADR 0006).
//!
//! Resolves the per-OS **user** preset directory and provides the file ops the
//! browser needs: load/save a [`Performance`], enumerate one level of
//! subfolders, create / rename / delete folders, and rename / delete / move a
//! user preset. All **main/UI-thread** work: the audio thread never touches
//! the filesystem or serde (ADR 0005 §6, ticket 0026 §Threading).
//!
//! The factory bank is embedded, not on disk ([`crate::factory`]); this module
//! is only the writable user side. Every mutating call canonicalises its target
//! path and refuses anything outside the user dir
//! ([`ensure_within_user_dir`]) — defence in depth, since the UI never
//! *should* hand it a bad path (ADR 0006 §5).

use crate::preset::{Meta, Performance, PresetError, from_toml_str};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

// Name sanitisation rules are shared with the web browser-storage store via
// `vxn-app` (E019 / 0063) so folder names and preset filenames can't drift
// between backends.
use vxn_app::{preset_filename, sanitize_name, unique_folder_name};

// `UNCATEGORIZED` (virtual root group label) lives in `vxn-app::domain` —
// see [`crate::UNCATEGORIZED`] for the re-export.

/// The per-OS directory VXN1 reads and writes user presets in (ADR 0005 §5).
/// `None` only if the platform's home/appdata environment variable is unset.
#[cfg(target_os = "macos")]
pub fn user_preset_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join("Library/Audio/Presets/Vulpus Labs/VXN1"))
}

#[cfg(target_os = "windows")]
pub fn user_preset_dir() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(
        Path::new(&appdata)
            .join("Vulpus Labs")
            .join("VXN1")
            .join("Presets"),
    )
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn user_preset_dir() -> Option<PathBuf> {
    // `$XDG_DATA_HOME/VXN1/presets`, falling back to `~/.local/share/VXN1/presets`.
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return Some(Path::new(&xdg).join("VXN1").join("presets"));
        }
    }
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join(".local/share/VXN1/presets"))
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
/// lands outside it (`PermissionDenied`). Targets that don't exist yet
/// (Save-As, rename destination, new folder) fall back to canonicalising the
/// parent and rejoining the filename. Belt-and-braces: the UI never *should*
/// hand it a bad path, but the guard is one line at each entry point.
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

/// Convert a [`LoadError`] into an [`io::Error`] for the rename/tag paths,
/// which read-modify-write existing files. A parse error becomes
/// `InvalidData`; an IO error passes through.
fn load_err_to_io(e: LoadError) -> io::Error {
    match e {
        LoadError::Io(e) => e,
        LoadError::Parse(e) => io::Error::new(io::ErrorKind::InvalidData, e.to_string()),
    }
}

/// Serialize and save a [`Performance`] to the user directory root as
/// `<name>.toml`. Shim over [`save_performance_in`] with `folder = None`.
pub fn save_performance(perf: &Performance) -> io::Result<PathBuf> {
    save_performance_in(perf, None)
}

// ── PresetStore adapter ──────────────────────────────────────────────────────
//
// The controller talks to preset IO through `vxn_app::PresetStore`; this is
// the engine-side adapter. Stateless — every call goes straight to the existing
// module functions and the `factory()` bank. The conversions between engine
// types (`Meta`, `Performance`, `UserPreset`, `UserFolder`) and the slim
// vxn-app shapes happen here, so `vxn-app` stays free of preset-format code.

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

fn meta_to_app(m: &crate::preset::Meta) -> vxn_app::PresetMeta {
    vxn_app::PresetMeta {
        name: m.name.clone(),
        author: m.author.clone(),
        category: m.category.clone(),
        comment: m.comment.clone(),
    }
}

fn perf_to_load(perf: Performance, warnings: Vec<String>) -> Result<vxn_app::PresetLoad, String> {
    let mut blob = Vec::with_capacity(256);
    perf.state.write(&mut blob).map_err(|e| e.to_string())?;
    Ok(vxn_app::PresetLoad {
        meta: meta_to_app(&perf.meta),
        blob,
        warnings,
    })
}

impl vxn_app::PresetStore for EnginePresetStore {
    fn factory_len(&self) -> usize {
        crate::factory::factory().len()
    }

    fn factory_load(&self, index: usize) -> Result<vxn_app::PresetLoad, String> {
        let bank = crate::factory::factory();
        let fp = bank.get(index).ok_or("factory index out of range")?;
        // Factory presets carry a ready-made Performance — no warnings.
        perf_to_load(fp.preset.clone(), Vec::new())
    }

    fn factory_meta(&self, index: usize) -> Option<vxn_app::PresetMeta> {
        crate::factory::factory().get(index).map(|fp| vxn_app::PresetMeta {
            // The browser groups factory presets by their directory category
            // (the `FactoryPreset.category` field), not the optional
            // `[meta] category` in the TOML. Override here so the
            // controller-published corpus carries the same grouping the old
            // `build_browser(&factory(), …)` produced.
            name: fp.preset.meta.name.clone(),
            author: fp.preset.meta.author.clone(),
            category: Some(fp.category.clone()),
            comment: fp.preset.meta.comment.clone(),
        })
    }

    fn user_load(&self, path: &Path) -> Result<vxn_app::PresetLoad, String> {
        let (perf, warnings) = load_preset_file(path).map_err(|e| e.to_string())?;
        perf_to_load(perf, warnings)
    }

    fn user_save(
        &self,
        name: &str,
        folder: Option<&str>,
        meta: &vxn_app::PresetMeta,
        blob: &[u8],
    ) -> Result<PathBuf, String> {
        let state = crate::state::PluginState::read(&mut &blob[..]).map_err(|e| e.to_string())?;
        let perf = Performance {
            meta: crate::preset::Meta {
                name: name.to_string(),
                author: meta.author.clone(),
                category: meta.category.clone(),
                comment: meta.comment.clone(),
            },
            state,
        };
        save_performance_in(&perf, folder).map_err(|e| e.to_string())
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

    fn user_rename_folder(
        &self,
        old: &str,
        new: &str,
    ) -> Result<(PathBuf, String), String> {
        rename_user_folder(old, new).map_err(|e| e.to_string())
    }

    fn user_delete_folder(&self, name: &str) -> Result<(), String> {
        delete_user_folder(name).map_err(|e| e.to_string())
    }

    fn list_user_tree(&self) -> Vec<vxn_app::UserFolderEntry> {
        // User presets surface only their display name in the corpus; full
        // metadata (author/category/comment) lives in the file and is re-read
        // by `user_load` when the user actually loads the preset.
        list_user_tree()
            .unwrap_or_default()
            .into_iter()
            .map(|f| vxn_app::UserFolderEntry {
                name: f.name.clone(),
                presets: f
                    .presets
                    .into_iter()
                    .map(|p| vxn_app::UserPresetEntry {
                        path: p.path,
                        meta: vxn_app::PresetMeta {
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

/// Save a [`Performance`] under the user-root (`folder = None`) or into the
/// named subfolder (creating it if missing). The filename is derived from
/// `meta.name`; the on-disk format is unchanged (ADR 0006 §2).
pub fn save_performance_in(perf: &Performance, folder: Option<&str>) -> io::Result<PathBuf> {
    let base = ensure_user_dir()?;
    let dir = match folder {
        Some(name) => base.join(sanitize_name(name)),
        None => base,
    };
    let path = dir.join(preset_filename(&perf.meta.name));
    ensure_within_user_dir(&path)?;
    fs::create_dir_all(&dir)?;
    fs::write(&path, perf.to_toml_string())?;
    Ok(path)
}

/// A user preset on disk, for the browser's listing.
#[derive(Clone, Debug)]
pub struct UserPreset {
    pub path: PathBuf,
    /// Display name from `[meta] name`.
    pub name: String,
    /// `None` = root group ([`UNCATEGORIZED`]); `Some(name)` = subdirectory.
    pub folder: Option<String>,
}

/// One folder's worth of user presets for the browser's two-pane layout.
/// `name == None` is the virtual root group (loose files at the top of the
/// user dir); `name == Some(_)` is a real subdirectory.
#[derive(Clone, Debug)]
pub struct UserFolder {
    pub name: Option<String>,
    pub presets: Vec<UserPreset>,
}

/// Enumerate the user directory's `*.toml` presets, sorted by display name.
/// Files that don't parse are skipped (a stray/corrupt file shouldn't break the
/// browser). Returns an empty list if the directory doesn't exist yet.
///
/// Flat view across the root group only; for the folder-aware browser see
/// [`list_user_tree`].
pub fn list_user_presets() -> io::Result<Vec<UserPreset>> {
    let Some(dir) = user_preset_dir() else {
        return Ok(Vec::new());
    };
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        if let Some(p) = read_preset_at(&entry.path(), None) {
            out.push(p);
        }
    }
    out.sort_by_key(|p| p.name.to_lowercase());
    Ok(out)
}

/// Walk one level deep: root group first ([`UNCATEGORIZED`]), then each
/// subfolder alpha-sorted. Empty subfolders are kept (a freshly-created
/// folder is empty). Files that don't parse are skipped silently.
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
    let (preset, _warnings) = from_toml_str(&contents).ok()?;
    Some(UserPreset {
        path: path.to_path_buf(),
        name: preset.meta.name,
        folder,
    })
}

/// Create a new user subfolder with a unique name. If `suggested` (sanitised)
/// already names a folder it's suffixed: `"New Folder"`, `"New Folder 1"`,
/// `"New Folder 2"`, … against existing folders (case-insensitive).
/// Returns `(path, chosen_name)`.
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

/// Delete a user subfolder and everything in it (recursive). The browser's
/// confirmation gate (ADR 0006 §7) is the UI's responsibility; this call
/// commits.
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
/// `dest_folder = None`). The on-disk filename is preserved — only the parent
/// directory changes (ADR 0006 §6). Refuses to overwrite an existing file.
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
/// filename → remove the old (ADR 0006 §6). The parent directory is
/// unchanged. Refuses to overwrite an existing destination filename.
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
    let (mut perf, _w) = load_preset_file(path).map_err(load_err_to_io)?;
    perf.meta.name = new_name.to_string();
    fs::write(&new_path, perf.to_toml_string())?;
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

/// Read and parse a single preset file. The returned warnings (unknown keys /
/// bad enum labels that fell back to defaults) are non-fatal — surface them to
/// the user (0027) rather than discarding them.
pub fn load_preset_file(path: &Path) -> Result<(Performance, Vec<String>), LoadError> {
    let contents = fs::read_to_string(path).map_err(LoadError::Io)?;
    from_toml_str(&contents).map_err(LoadError::Parse)
}

/// Construct a [`Performance`] [`Meta`] with just a name (Save-As). A
/// convenience for the browser; author/category/tags can be filled later.
pub fn meta_named(name: &str) -> Meta {
    Meta {
        name: name.to_string(),
        ..Meta::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ParamValues;
    use crate::params::{KeyMode, Layer, PatchParam};
    use crate::state::PluginState;

    fn temp_dir() -> PathBuf {
        // Hermetic per-process scratch dir; never the real user preset dir.
        std::env::temp_dir().join(format!("vxn1-preset-io-test-{}", std::process::id()))
    }

    // Name-sanitisation rules (sanitize_name / preset_filename /
    // unique_folder_name) moved to `vxn-app::preset_names` (E019 / 0063) and are
    // tested there — shared with the web browser-storage store.

    #[test]
    fn write_then_load_round_trips_a_performance() {
        let dir = temp_dir();
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let mut params = ParamValues::default();
        params.layer_mut(Layer::Upper).set(PatchParam::Cutoff, 1234.0);
        let perf = Performance {
            meta: meta_named("Round Trip"),
            state: PluginState {
                params,
                key_mode: KeyMode::Split,
                split_point: 48,
            },
        };

        let path = dir.join(preset_filename(&perf.meta.name));
        fs::write(&path, perf.to_toml_string()).unwrap();
        assert!(path.ends_with("Round Trip.toml"));

        let (p, warnings) = load_preset_file(&path).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(p.state.key_mode, KeyMode::Split);
        assert_eq!(p.state.split_point, 48);
        assert_eq!(
            p.state.params.layer(Layer::Upper).get(PatchParam::Cutoff),
            1234.0
        );

        let listed = {
            let mut v = Vec::new();
            for entry in fs::read_dir(&dir).unwrap() {
                v.push(entry.unwrap().path());
            }
            v
        };
        assert!(listed.iter().any(|p| p == &path));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_error_on_missing_file() {
        let path = temp_dir().join("does-not-exist.toml");
        assert!(matches!(load_preset_file(&path), Err(LoadError::Io(_))));
    }

}
