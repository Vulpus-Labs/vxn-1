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
/// lands outside it.
///
/// The base is resolved here and the check itself is [`ensure_within`], which
/// takes it as an argument: the guard is the security-shaped part of this
/// module, and testing it must not depend on — or write to — the developer's
/// real preset directory (0320).
fn ensure_within_user_dir(target: &Path) -> io::Result<()> {
    ensure_within(&ensure_user_dir()?, target)
}

/// Refuse any `target` that does not land inside `base` once both are
/// canonicalised.
///
/// Targets that don't exist yet (Save-As, rename dest, new folder) fall back to
/// canonicalising the *parent* and rejoining the filename — a path cannot be
/// canonicalised before it exists, and refusing every not-yet-created path
/// would refuse every save.
///
/// Canonicalising is what makes this a real guard rather than a string check:
/// it resolves `..` segments and follows symlinks, so neither
/// `<base>/../../etc/passwd` nor a symlink planted inside the tree can escape.
fn ensure_within(base: &Path, target: &Path) -> io::Result<()> {
    let canon_base = fs::canonicalize(base).unwrap_or_else(|_| base.to_path_buf());
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
    create_user_folder_in(&ensure_user_dir()?, suggested)
}

/// [`create_user_folder`] against an explicit base — the form the tests drive,
/// so they exercise the shipping logic rather than a stand-in (0320).
fn create_user_folder_in(base: &Path, suggested: &str) -> io::Result<(PathBuf, String)> {
    let stem = sanitize_name(suggested);
    let existing = existing_folder_names_ci(base)?;
    let name = unique_folder_name(&stem, &existing);
    let path = base.join(&name);
    ensure_within(base, &path)?;
    fs::create_dir(&path)?;
    Ok((path, name))
}

/// Rename an existing user subfolder. Refuses to overwrite an existing
/// destination. Returns `(new_path, sanitised_new_name)`.
pub fn rename_user_folder(old: &str, new: &str) -> io::Result<(PathBuf, String)> {
    rename_user_folder_in(&ensure_user_dir()?, old, new)
}

/// [`rename_user_folder`] against an explicit base.
fn rename_user_folder_in(base: &Path, old: &str, new: &str) -> io::Result<(PathBuf, String)> {
    let old_path = base.join(sanitize_name(old));
    let new_name = sanitize_name(new);
    let new_path = base.join(&new_name);
    ensure_within(base, &old_path)?;
    ensure_within(base, &new_path)?;
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
    delete_user_folder_in(&ensure_user_dir()?, name)
}

/// [`delete_user_folder`] against an explicit base.
fn delete_user_folder_in(base: &Path, name: &str) -> io::Result<()> {
    let path = base.join(sanitize_name(name));
    ensure_within(base, &path)?;
    fs::remove_dir_all(&path)
}

/// Delete a user preset file. Refuses paths outside the user directory.
pub fn delete_user_preset(path: &Path) -> io::Result<()> {
    delete_user_preset_in(&ensure_user_dir()?, path)
}

/// [`delete_user_preset`] against an explicit base.
fn delete_user_preset_in(base: &Path, path: &Path) -> io::Result<()> {
    ensure_within(base, path)?;
    fs::remove_file(path)
}

/// Move a user preset into the named subfolder (or back to the root with
/// `dest_folder = None`). The on-disk filename is preserved.
pub fn move_user_preset(path: &Path, dest_folder: Option<&str>) -> io::Result<PathBuf> {
    move_user_preset_in(&ensure_user_dir()?, path, dest_folder)
}

/// [`move_user_preset`] against an explicit base.
fn move_user_preset_in(base: &Path, path: &Path, dest_folder: Option<&str>) -> io::Result<PathBuf> {
    ensure_within(base, path)?;
    let dest_dir = match dest_folder {
        Some(name) => base.join(sanitize_name(name)),
        None => base.to_path_buf(),
    };
    let filename = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "preset has no filename"))?;
    let new_path = dest_dir.join(filename);
    ensure_within(base, &new_path)?;
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
    rename_user_preset_in(&ensure_user_dir()?, path, new_name)
}

/// [`rename_user_preset`] against an explicit base.
fn rename_user_preset_in(base: &Path, path: &Path, new_name: &str) -> io::Result<PathBuf> {
    ensure_within(base, path)?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "preset has no parent"))?;
    let new_path = parent.join(preset_filename(new_name));
    ensure_within(base, &new_path)?;
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

    // ── Filesystem half (0320) ──────────────────────────────────────────────
    //
    // Until 0320 nothing here touched the filesystem at all: the two tests
    // above cover the embedded factory bank and stop. Everything below — the
    // path-escape guard especially — is reused by `vxn1b-web-controller`'s user
    // store, where the names arrive from a browser.
    //
    // Each op has a `*_in(base, ..)` inner that the public function delegates
    // to, so these drive the shipping logic against a `tempdir` rather than a
    // stand-in, and never touch the developer's real preset directory.

    use std::fs;
    use tempfile::TempDir;

    fn tmp() -> TempDir {
        TempDir::new().expect("tempdir")
    }

    /// A guard returning `Ok` for a good path proves very little; these are the
    /// paths it exists to refuse.
    #[test]
    fn the_escape_guard_refuses_traversal_and_absolute_paths() {
        let dir = tmp();
        let base = dir.path();

        // Inside is fine — including a not-yet-existing file, which is the
        // Save-As case the parent-canonicalise branch exists for.
        assert!(ensure_within(base, &base.join("Lead.toml")).is_ok());
        fs::create_dir(base.join("Bass")).unwrap();
        assert!(ensure_within(base, &base.join("Bass")).is_ok());
        assert!(ensure_within(base, &base.join("Bass/Sub.toml")).is_ok());

        // `..` traversal, both bare and buried mid-path. Canonicalisation is
        // what collapses these — a `starts_with` on the raw string would pass
        // the second one.
        for escape in [
            base.join("../outside.toml"),
            base.join("../../outside.toml"),
            base.join("Bass/../../outside.toml"),
        ] {
            let err = ensure_within(base, &escape)
                .expect_err(&format!("{} escaped the base", escape.display()));
            assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        }

        // An absolute path elsewhere.
        let other = tmp();
        let err = ensure_within(base, &other.path().join("elsewhere.toml"))
            .expect_err("an absolute path outside the base was allowed");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(ensure_within(base, Path::new("/etc/passwd")).is_err());
    }

    /// Symlinks are the reason the guard canonicalises rather than string-
    /// matches: a link planted *inside* the tree must not become a way out.
    #[cfg(unix)]
    #[test]
    fn the_escape_guard_follows_symlinks_out_of_the_tree() {
        let dir = tmp();
        let outside = tmp();
        let base = dir.path();
        fs::write(outside.path().join("secret.toml"), "x").unwrap();
        std::os::unix::fs::symlink(outside.path(), base.join("escape")).unwrap();

        // The link itself resolves outside, and so does anything under it.
        assert!(ensure_within(base, &base.join("escape")).is_err());
        assert!(ensure_within(base, &base.join("escape/secret.toml")).is_err());
    }

    #[test]
    fn sanitize_name_keeps_the_safe_set_and_never_returns_empty() {
        assert_eq!(sanitize_name("Fat Bass"), "Fat Bass");
        assert_eq!(sanitize_name("Lead-2_alt"), "Lead-2_alt");
        // Separators and traversal characters are the ones that matter here:
        // this is what stands between a browser-supplied name and a path.
        assert_eq!(sanitize_name("../etc/passwd"), "___etc_passwd");
        assert_eq!(sanitize_name("a/b\\c"), "a_b_c");
        assert_eq!(sanitize_name("nul\0byte"), "nul_byte");
        // Trimmed, and never empty — an empty filename is not a filename.
        assert_eq!(sanitize_name("  padded  "), "padded");
        assert_eq!(sanitize_name(""), "Untitled");
        assert_eq!(sanitize_name("   "), "Untitled");
        // Not "Untitled": every separator maps to `_`, so this is non-empty and
        // still a single safe path segment. Only a name that is empty *after*
        // trimming falls back.
        assert_eq!(sanitize_name("///"), "___");
        // Non-ASCII alphanumerics are kept: they are not path-dangerous.
        assert_eq!(sanitize_name("Café"), "Café");
    }

    #[test]
    fn unique_folder_name_counts_up_case_insensitively() {
        assert_eq!(unique_folder_name("Leads", &[]), "Leads");
        // The existing list is lowercased by `existing_folder_names_ci`, and the
        // comparison must be case-insensitive or a case-only clash slips
        // through on a case-insensitive filesystem.
        assert_eq!(unique_folder_name("Leads", &["leads".into()]), "Leads 1");
        assert_eq!(
            unique_folder_name("Leads", &["leads".into(), "leads 1".into()]),
            "Leads 2"
        );
        // A gap is filled, not skipped past.
        assert_eq!(
            unique_folder_name("Leads", &["leads".into(), "leads 2".into()]),
            "Leads 1"
        );
        assert_eq!(unique_folder_name("Leads", &["pads".into()]), "Leads");
    }

    #[test]
    fn folder_operations_round_trip_on_a_real_tree() {
        let dir = tmp();
        let base = dir.path();

        let (path, name) = create_user_folder_in(base, "Leads").unwrap();
        assert_eq!(name, "Leads");
        assert!(path.is_dir());

        // Same name again uniquifies rather than colliding or erroring.
        let (_, second) = create_user_folder_in(base, "Leads").unwrap();
        assert_eq!(second, "Leads 1");

        // A traversal-shaped name is sanitised into a plain folder inside the
        // base, not followed.
        let (esc_path, esc_name) = create_user_folder_in(base, "../escape").unwrap();
        assert_eq!(esc_name, "___escape");
        assert_eq!(esc_path.parent().unwrap(), base);

        let (renamed, new_name) = rename_user_folder_in(base, "Leads", "Pads").unwrap();
        assert_eq!(new_name, "Pads");
        assert!(renamed.is_dir());
        assert!(!base.join("Leads").exists());

        // Refuses to clobber an existing destination.
        let err = rename_user_folder_in(base, "Leads 1", "Pads").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);

        delete_user_folder_in(base, "Pads").unwrap();
        assert!(!renamed.exists());
    }

    #[test]
    fn preset_operations_round_trip_on_a_real_tree() {
        let dir = tmp();
        let base = dir.path();
        let meta = Meta {
            name: "Fat Bass".into(),
            ..Default::default()
        };
        let state = PluginState::factory_default();
        let text = write_preset(&meta, &state).unwrap();
        let root_file = base.join(preset_filename("Fat Bass"));
        fs::write(&root_file, &text).unwrap();

        // Rename rewrites the file AND the embedded meta name, and removes the
        // old file — the round-trip is what proves the second half.
        let renamed = rename_user_preset_in(base, &root_file, "Thin Bass").unwrap();
        assert_eq!(renamed.file_name().unwrap(), "Thin Bass.toml");
        assert!(!root_file.exists());
        let (m, _, _) = load_preset_file(&renamed).unwrap();
        assert_eq!(m.name, "Thin Bass");

        // Move into a folder, then back to the root.
        create_user_folder_in(base, "Bass").unwrap();
        let moved = move_user_preset_in(base, &renamed, Some("Bass")).unwrap();
        assert_eq!(moved.parent().unwrap(), base.join("Bass"));
        assert!(!renamed.exists());
        let back = move_user_preset_in(base, &moved, None).unwrap();
        assert_eq!(back.parent().unwrap(), base);

        delete_user_preset_in(base, &back).unwrap();
        assert!(!back.exists());
    }

    /// The guard is not merely present in these paths — it stops them.
    #[test]
    fn the_preset_operations_refuse_a_path_outside_the_base() {
        let dir = tmp();
        let outside = tmp();
        let base = dir.path();
        let intruder = outside.path().join("elsewhere.toml");
        fs::write(&intruder, "x").unwrap();

        for kind in [
            delete_user_preset_in(base, &intruder).map(|_| ()).unwrap_err().kind(),
            move_user_preset_in(base, &intruder, None).map(|_| ()).unwrap_err().kind(),
            rename_user_preset_in(base, &intruder, "Nope").map(|_| ()).unwrap_err().kind(),
        ] {
            assert_eq!(kind, io::ErrorKind::PermissionDenied);
        }
        // And the file is still there — refused, not deleted.
        assert!(intruder.exists());
    }
}
