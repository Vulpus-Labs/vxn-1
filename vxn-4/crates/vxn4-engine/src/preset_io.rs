//! User-preset filesystem IO + the [`vxn_core_app::PresetStore`] adapter (0385).
//!
//! [`crate::preset`] is deliberately pure — it maps a [`Patch`] to text and back
//! and touches nothing else. This module is the other half: it resolves the
//! per-OS **user** preset directory and provides the file ops a browser needs —
//! load/save a preset, enumerate one level of subfolders, create / rename /
//! delete a folder, and rename / delete / move a user preset — then adapts them
//! to the shared controller's [`PresetStore`] trait.
//!
//! Ported in shape from `vxn1b-engine/src/preset_io.rs`, adapted to vxn-4's
//! `(Meta, Patch, Macros)` triple rather than vxn-1b's `(Meta, PluginState)`
//! pair. The layout, the one-level folder depth and the guard are vxn-1b's on
//! purpose: the browser UI (0389) is then a port rather than a design.
//!
//! All **main/UI-thread** work. Nothing here is reachable from
//! [`crate::Engine::process`] — the audio thread neither opens files nor parses
//! TOML, and the only way a preset reaches it is as a snapshot through
//! [`crate::shared::SharedParams`].
//!
//! ## The factory bank needs no IO
//!
//! The seven patches are baked into the binary ([`crate::patch`](mod@crate::patch)) and stay that
//! way: they are `const`-shaped Rust, there is no install step that could put
//! them on disk, and a bank that cannot go missing is one fewer failure mode.
//! The store presents them and the user's files as **one corpus**, indexed on
//! the factory side and path-addressed on the user side. Factory presets are
//! read-only and there is no code path here that writes one — a "save" over a
//! factory patch is an ordinary [`PresetStore::user_save`] under the same name,
//! which lands in the user directory and shadows nothing.
//!
//! ## Path containment
//!
//! A preset name arrives from user input — eventually from a webview — and
//! becomes a path component. So every mutating call canonicalises its target and
//! refuses anything landing outside the user directory ([`ensure_within`]), and
//! every name that becomes a path segment goes through [`sanitize_name`] first.
//! The two are belt and braces on purpose: sanitisation is what keeps a name
//! from *meaning* anything to the path parser, and the guard is what catches the
//! case where a path reached us some other way.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use vxn_core_app::{PresetLoad, PresetMeta, PresetStore, UserFolderEntry, UserPresetEntry};

use crate::patch::{N_PATCHES, Patch, patch, patch_names};
use crate::preset::{Macros, Meta, Preset, PresetError, read_preset, write_preset};

// ── name sanitisation ───────────────────────────────────────────────────────
//
// A local copy of vxn-1b's rules rather than a dependency on them: the two
// synths share a format but not a bank, and a filename that drifted between the
// backends would show up as a preset that saves under one name and lists under
// another.

/// Sanitise a display name into a filesystem-safe stem — alphanumerics, space,
/// `-` and `_` survive; everything else becomes `_`. Empty after trimming →
/// `"Untitled"`.
///
/// Mapping rather than stripping is deliberate: `../etc/passwd` becomes
/// `___etc_passwd`, which is a visibly odd single path segment rather than a
/// silently shortened one that might collide with a real preset.
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

/// Preset filename derived from a display name (`<sanitised>.toml`).
pub fn preset_filename(name: &str) -> String {
    format!("{}.toml", sanitize_name(name))
}

/// Pick a folder name that does not collide (**case-insensitively**) with any in
/// `existing_ci`, suffixing ` 1`, ` 2`, … until one is free.
///
/// Case-insensitive because macOS and Windows filesystems are: a case-only
/// clash is a failed `create_dir` on the user's machine and a passing test on a
/// case-sensitive CI box.
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

// ── the user directory ──────────────────────────────────────────────────────

/// The per-OS directory VXN4 reads and writes user presets in.
///
/// A leaf of its own (`.../VXN4`) so the four synths' banks never see each
/// other's files: they share an envelope but not a parameter table, and a VXN1b
/// preset opened here would be a page of warnings.
///
/// `None` only when the platform's home/appdata environment variable is unset,
/// which is a headless-CI shape rather than anything a user will hit.
#[cfg(target_os = "macos")]
pub fn user_preset_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join("Library/Audio/Presets/Vulpus Labs/VXN4"))
}

#[cfg(target_os = "windows")]
pub fn user_preset_dir() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(
        Path::new(&appdata)
            .join("Vulpus Labs")
            .join("VXN4")
            .join("Presets"),
    )
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn user_preset_dir() -> Option<PathBuf> {
    // `$XDG_DATA_HOME/VXN4/presets`, falling back to `~/.local/share/VXN4/presets`.
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return Some(Path::new(&xdg).join("VXN4").join("presets"));
        }
    }
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join(".local/share/VXN4/presets"))
}

fn no_dir_err() -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        "no user preset directory for this platform",
    )
}

/// Resolve the user preset directory and create it. Idempotent, and called at
/// the head of every op rather than once at startup: the user can delete the
/// folder between two saves, and a browser that then reports "no such directory"
/// is a worse answer than one that puts it back.
pub fn ensure_user_dir() -> io::Result<PathBuf> {
    let dir = user_preset_dir().ok_or_else(no_dir_err)?;
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Refuse any `target` that does not land inside `base` once both are
/// canonicalised.
///
/// Canonicalising is what makes this a guard rather than a string check: it
/// collapses `..` segments **and follows symlinks**, so neither
/// `<base>/../../etc/passwd` nor a link planted inside the tree is a way out. A
/// `starts_with` on the raw path would pass both.
///
/// The subtlety is that a path cannot be canonicalised before it exists, and
/// most of the calls here are about a file that is *about* to exist — Save-As, a
/// rename destination, a new folder in a folder. This is the usual place the
/// check is got wrong in both directions: canonicalise-or-bail refuses every
/// save, and skip-the-check-if-it-does-not-exist refuses nothing. See
/// [`resolve_for_check`] for how the not-yet-existing tail is handled.
fn ensure_within(base: &Path, target: &Path) -> io::Result<()> {
    let canon_base = fs::canonicalize(base).unwrap_or_else(|_| base.to_path_buf());
    if !resolve_for_check(target).starts_with(&canon_base) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "preset path outside user directory",
        ));
    }
    Ok(())
}

/// Canonicalise as much of `target` as exists, then re-attach the part that does
/// not, resolving `..` **lexically** as it goes.
///
/// Canonicalising only the immediate parent — vxn-1b's version of this — is one
/// level short in two ways. A save into a folder that has not been created yet
/// has *two* missing components, not one; and more quietly, it assumes the base
/// itself canonicalises to a prefix of the raw parent, which on macOS is false
/// the moment the base is under `/var` (a symlink to `/private/var`) — every
/// save into a new subfolder of a temp directory is then refused. So this walks
/// up to the nearest ancestor that does exist instead.
///
/// Resolving `..` lexically on the way back down is what keeps that safe. The
/// canonical prefix contains no symlinks, and the re-attached components do not
/// exist, so they cannot be symlinks either — which means a lexical `pop` is
/// exactly what the filesystem would do. Popping past the base is allowed to
/// happen and left for [`ensure_within`] to refuse; swallowing it here would
/// turn `<base>/new/../../outside` into something that passes.
fn resolve_for_check(target: &Path) -> PathBuf {
    use std::path::Component;

    // Peeled component-wise rather than with `parent()`, because `parent()` of a
    // path ending in `..` is not the directory above it and walking with it
    // would leave a literal `..` in the result for the prefix check to be fooled
    // by. Every absolute path bottoms out at the root, which always exists, so
    // the loop finds a prefix.
    let comps: Vec<Component> = target.components().collect();
    for split in (1..=comps.len()).rev() {
        let head: PathBuf = comps[..split].iter().collect();
        let Ok(mut out) = fs::canonicalize(&head) else {
            continue;
        };
        for c in &comps[split..] {
            match c {
                Component::ParentDir => {
                    out.pop();
                }
                Component::CurDir => {}
                other => out.push(other.as_os_str()),
            }
        }
        return out;
    }
    // Nothing on the path resolves at all — a relative path against a working
    // directory we cannot see. Hand back what we were given; the base is
    // absolute, so the prefix check refuses it.
    target.to_path_buf()
}

// ── file ops ────────────────────────────────────────────────────────────────
//
// Each op is a public function that resolves the real user directory and an
// inner `*_in(base, ..)` that takes it as an argument. The split exists for the
// tests: the guard is the security-shaped part of this module, and exercising it
// must neither depend on nor write to the developer's own preset directory. The
// tests therefore drive the shipping logic against a `tempdir` rather than a
// stand-in.

/// A user preset on disk, as the browser lists it.
#[derive(Clone, Debug)]
pub struct UserPreset {
    pub path: PathBuf,
    pub name: String,
    /// `None` = the user-dir root; `Some(name)` = a subfolder.
    pub folder: Option<String>,
}

/// One folder's worth of user presets. `name == None` is the virtual root.
#[derive(Clone, Debug)]
pub struct UserFolder {
    pub name: Option<String>,
    pub presets: Vec<UserPreset>,
}

/// Save a preset under the user root (`folder = None`) or into the named
/// subfolder, creating it if missing. The filename derives from `meta.name`.
pub fn save_preset_in(
    meta: &Meta,
    patch: &Patch,
    macros: &Macros,
    folder: Option<&str>,
) -> io::Result<PathBuf> {
    save_preset_into(&ensure_user_dir()?, meta, patch, macros, folder)
}

/// [`save_preset_in`] against an explicit base.
fn save_preset_into(
    base: &Path,
    meta: &Meta,
    patch: &Patch,
    macros: &Macros,
    folder: Option<&str>,
) -> io::Result<PathBuf> {
    let dir = match folder {
        Some(name) => base.join(sanitize_name(name)),
        None => base.to_path_buf(),
    };
    let path = dir.join(preset_filename(&meta.name));
    ensure_within(base, &path)?;
    fs::create_dir_all(&dir)?;
    // Serialise before touching the disk, so a codec failure leaves no
    // half-written file where a preset used to be.
    let text = write_preset(meta, patch, macros)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(&path, text)?;
    Ok(path)
}

/// Walk one level deep: the root group first, then each subfolder alpha-sorted.
///
/// Empty subfolders are kept — the user made them, and a folder that vanishes
/// when you empty it is a surprise. Files that do not parse are skipped
/// silently: a stray `.toml` in the preset folder is not an error worth
/// interrupting a browser refresh for, and a file that *is* a preset and fails
/// to parse reports its warnings when it is opened.
pub fn list_user_tree() -> io::Result<Vec<UserFolder>> {
    let Some(base) = user_preset_dir() else {
        return Ok(Vec::new());
    };
    list_user_tree_in(&base)
}

/// [`list_user_tree`] against an explicit base.
fn list_user_tree_in(base: &Path) -> io::Result<Vec<UserFolder>> {
    if !base.exists() {
        return Ok(Vec::new());
    }

    let mut root_presets = Vec::new();
    let mut subfolders: Vec<(String, Vec<UserPreset>)> = Vec::new();

    for entry in fs::read_dir(base)? {
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
                // One level only (E052). A nested directory is simply not
                // descended into, not an error and not flattened.
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

/// The listing entry for one file, or `None` if it is not a preset. The name
/// shown is the file's `[meta]` name, not its stem: the two can diverge (a user
/// renames the file in Finder) and the embedded one is what the author meant.
fn read_preset_at(path: &Path, folder: Option<String>) -> Option<UserPreset> {
    if path.extension().and_then(|e| e.to_str()) != Some("toml") {
        return None;
    }
    let contents = fs::read_to_string(path).ok()?;
    let preset = read_preset(&contents).ok()?;
    Some(UserPreset {
        path: path.to_path_buf(),
        name: preset.meta.name,
        folder,
    })
}

/// Create a new user subfolder with a non-colliding name. Returns
/// `(path, chosen_name)` — the chosen name can differ from the suggestion, both
/// by sanitisation and by the collision suffix, and the caller needs it to
/// select the new folder in the browser.
pub fn create_user_folder(suggested: &str) -> io::Result<(PathBuf, String)> {
    create_user_folder_in(&ensure_user_dir()?, suggested)
}

/// [`create_user_folder`] against an explicit base.
fn create_user_folder_in(base: &Path, suggested: &str) -> io::Result<(PathBuf, String)> {
    let stem = sanitize_name(suggested);
    let existing = existing_folder_names_ci(base)?;
    let name = unique_folder_name(&stem, &existing);
    let path = base.join(&name);
    ensure_within(base, &path)?;
    fs::create_dir(&path)?;
    Ok((path, name))
}

/// Rename a user subfolder. Refuses to clobber an existing destination — a
/// rename that silently merged two folders would lose the collisions.
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

/// Delete a user subfolder and everything in it.
pub fn delete_user_folder(name: &str) -> io::Result<()> {
    delete_user_folder_in(&ensure_user_dir()?, name)
}

/// [`delete_user_folder`] against an explicit base.
fn delete_user_folder_in(base: &Path, name: &str) -> io::Result<()> {
    let path = base.join(sanitize_name(name));
    ensure_within(base, &path)?;
    // The guard runs before the recursive delete, which is the whole point:
    // this is the one call in the module that could take a directory tree with
    // it if the path resolved somewhere unexpected.
    fs::remove_dir_all(&path)
}

/// Delete a user preset file.
pub fn delete_user_preset(path: &Path) -> io::Result<()> {
    delete_user_preset_in(&ensure_user_dir()?, path)
}

/// [`delete_user_preset`] against an explicit base.
fn delete_user_preset_in(base: &Path, path: &Path) -> io::Result<()> {
    ensure_within(base, path)?;
    fs::remove_file(path)
}

/// Move a user preset into the named subfolder, or back to the root with
/// `dest_folder = None`. The on-disk filename is preserved — a move is not a
/// rename, and rewriting the file would change its `[meta]` mtime for nothing.
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
    // Source and destination are separate arguments and each gets the guard:
    // a legal source does not make an arbitrary destination legal.
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

/// Rename a user preset: load, rewrite `meta.name`, write under the new
/// filename, remove the old. The parent directory is unchanged.
///
/// A rename is a load-and-rewrite rather than an `fs::rename` because the
/// display name lives *inside* the file as well as in its stem, and a browser
/// that renamed only the stem would list the preset under its new name and open
/// it under its old one.
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
    let preset = load_preset_file(path).map_err(load_err_to_io)?;
    // Warnings on a rename are worth saying out loud: the rewrite is about to
    // make the substitutions permanent, and this is the only moment the user
    // could still have the original file.
    if !preset.warnings.is_empty() {
        eprintln!(
            "vxn4: rename_user_preset({}): {} parse warning(s): {}",
            path.display(),
            preset.warnings.len(),
            preset.warnings.join("; ")
        );
    }
    let meta = Meta {
        name: new_name.to_string(),
        ..preset.meta
    };
    let text = write_preset(&meta, &preset.patch, &preset.macros)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
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

/// Why a preset file failed to load. The two halves are worth keeping apart:
/// an IO error is about the file, a parse error is about its contents, and the
/// browser says different things about them.
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

/// Read and parse a single preset file.
pub fn load_preset_file(path: &Path) -> Result<Preset, LoadError> {
    let contents = fs::read_to_string(path).map_err(LoadError::Io)?;
    read_preset(&contents).map_err(LoadError::Parse)
}

// ── the controller's byte channel ───────────────────────────────────────────

/// Encode a preset for [`PresetLoad::blob`] — vxn-4's is the preset file's own
/// UTF-8 text.
///
/// The controller's blob is opaque by design ([`vxn_core_app::ParamModel`]):
/// it only has to be something the model accepts, and the store and the model
/// have to agree on what that is. Reusing the preset text means the store needs
/// no second encoding at all, the factory and user sides produce identical bytes
/// for identical sounds, and the format's round-trip property is the one
/// [`crate::preset`]'s tests already hold. The `clap.state` blob is a separate
/// question with a separate answer (0384) and is deliberately not entangled
/// with this one — a preset has to survive being read by a different build,
/// where a project file only has to survive being read back by this one.
pub fn encode_blob(meta: &Meta, patch: &Patch, macros: &Macros) -> Result<Vec<u8>, String> {
    write_preset(meta, patch, macros).map(String::into_bytes)
}

/// Decode a [`PresetLoad::blob`] back into a preset. The inverse of
/// [`encode_blob`], used by `user_save` to turn the model's snapshot into
/// something it can write.
pub fn decode_blob(blob: &[u8]) -> Result<Preset, String> {
    let text = std::str::from_utf8(blob).map_err(|e| format!("preset blob is not UTF-8: {e}"))?;
    read_preset(text).map_err(|e| e.to_string())
}

// ── the PresetStore adapter ─────────────────────────────────────────────────

/// What the browser shows above the user's own presets. The factory bank has no
/// categories of its own — that is out of scope for E052, and seven patches do
/// not want subdividing — but the corpus groups the factory side *by* category,
/// so one honest label beats seven presets filed under "Uncategorised".
const FACTORY_CATEGORY: &str = "Factory";

/// The engine's [`PresetStore`]: the baked bank on the factory side, the user
/// directory on the other, presented to the controller as one corpus.
///
/// Stateless in the shipping configuration. `root` exists so the tests can point
/// the whole trait surface at a temp directory — the alternative is either
/// testing a stand-in that is not the shipping code, or writing into the
/// developer's real preset folder, and neither is acceptable for the half of
/// this module that is a security guard.
pub struct EnginePresetStore {
    root: Option<PathBuf>,
}

impl EnginePresetStore {
    /// A store over the per-OS user preset directory.
    pub fn new() -> Self {
        Self { root: None }
    }

    /// A store rooted at an explicit directory. Test-facing; the plugin uses
    /// [`Self::new`].
    pub fn rooted_at(root: impl Into<PathBuf>) -> Self {
        Self {
            root: Some(root.into()),
        }
    }

    /// Resolve and create this store's base directory.
    fn base(&self) -> io::Result<PathBuf> {
        match &self.root {
            Some(root) => {
                fs::create_dir_all(root)?;
                Ok(root.clone())
            }
            None => ensure_user_dir(),
        }
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

fn to_load(preset: Preset) -> Result<PresetLoad, String> {
    let blob = encode_blob(&preset.meta, &preset.patch, &preset.macros)?;
    Ok(PresetLoad {
        meta: meta_to_app(&preset.meta),
        blob,
        warnings: preset.warnings,
    })
}

/// The metadata a baked patch presents. Author and category are the bank's, not
/// the patch's: [`Patch`] carries a `&'static str` name and nothing else, and
/// inventing per-patch fields to satisfy a browser column would put display
/// strings into a struct that has to cross to the audio thread (0382).
fn factory_meta_at(index: usize) -> Option<Meta> {
    let names = patch_names();
    names.get(index).map(|name| Meta {
        name: (*name).to_string(),
        author: Some("Vulpus Labs".to_string()),
        category: Some(FACTORY_CATEGORY.to_string()),
        comment: None,
    })
}

impl PresetStore for EnginePresetStore {
    fn factory_len(&self) -> usize {
        N_PATCHES
    }

    fn factory_load(&self, index: usize) -> Result<PresetLoad, String> {
        // The bounds check is this function's, not `patch`'s: `patch` wraps its
        // index modulo the bank, which is the right behaviour for a host param
        // that must always denote *some* patch and the wrong one for a browser,
        // where a stale index should say so rather than open patch 0.
        let meta = factory_meta_at(index).ok_or_else(|| format!("no factory preset {index}"))?;
        // The baked bank carries no macro display record — labels arrive with
        // the faceplate (0388) — so a factory preset's knobs are unassigned.
        to_load(Preset {
            meta,
            patch: patch(index),
            macros: Macros::default(),
            warnings: Vec::new(),
        })
    }

    fn factory_meta(&self, index: usize) -> Option<PresetMeta> {
        factory_meta_at(index).map(|m| meta_to_app(&m))
    }

    fn user_load(&self, path: &Path) -> Result<PresetLoad, String> {
        let preset = load_preset_file(path).map_err(|e| e.to_string())?;
        to_load(preset)
    }

    fn user_save(
        &self,
        name: &str,
        folder: Option<&str>,
        meta: &PresetMeta,
        blob: &[u8],
    ) -> Result<PathBuf, String> {
        // This is also the factory "save": the store has no write path into the
        // bank, so overwriting a factory patch is simply a user preset under the
        // same name.
        let preset = decode_blob(blob)?;
        let file_meta = Meta {
            name: name.to_string(),
            author: meta.author.clone(),
            category: meta.category.clone(),
            comment: meta.comment.clone(),
        };
        let base = self.base().map_err(|e| e.to_string())?;
        save_preset_into(&base, &file_meta, &preset.patch, &preset.macros, folder)
            .map_err(|e| e.to_string())
    }

    fn user_delete(&self, path: &Path) -> Result<(), String> {
        let base = self.base().map_err(|e| e.to_string())?;
        delete_user_preset_in(&base, path).map_err(|e| e.to_string())
    }

    fn user_rename(&self, path: &Path, new_name: &str) -> Result<PathBuf, String> {
        let base = self.base().map_err(|e| e.to_string())?;
        rename_user_preset_in(&base, path, new_name).map_err(|e| e.to_string())
    }

    fn user_move(&self, path: &Path, dest_folder: Option<&str>) -> Result<PathBuf, String> {
        let base = self.base().map_err(|e| e.to_string())?;
        move_user_preset_in(&base, path, dest_folder).map_err(|e| e.to_string())
    }

    fn user_create_folder(&self, suggested: &str) -> Result<(PathBuf, String), String> {
        let base = self.base().map_err(|e| e.to_string())?;
        create_user_folder_in(&base, suggested).map_err(|e| e.to_string())
    }

    fn user_rename_folder(&self, old: &str, new: &str) -> Result<(PathBuf, String), String> {
        let base = self.base().map_err(|e| e.to_string())?;
        rename_user_folder_in(&base, old, new).map_err(|e| e.to_string())
    }

    fn user_delete_folder(&self, name: &str) -> Result<(), String> {
        let base = self.base().map_err(|e| e.to_string())?;
        delete_user_folder_in(&base, name).map_err(|e| e.to_string())
    }

    fn list_user_tree(&self) -> Vec<UserFolderEntry> {
        // A listing failure is an empty browser, not an error dialog: this runs
        // on every refresh and the user has no action to take about a `read_dir`
        // that failed.
        let base = match self.base() {
            Ok(b) => b,
            Err(_) => return Vec::new(),
        };
        list_user_tree_in(&base)
            .unwrap_or_default()
            .into_iter()
            .map(|f| UserFolderEntry {
                name: f.name,
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

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    use crate::matrix::Matrix;
    use crate::params::{desc, patch_ids, value_of};
    use crate::preset::MacroSpec;

    fn tmp() -> TempDir {
        TempDir::new().expect("tempdir")
    }

    fn meta(name: &str) -> Meta {
        Meta {
            name: name.to_string(),
            ..Meta::default()
        }
    }

    /// Every patch field compared **bitwise**, as [`crate::preset`]'s own tests
    /// do: `assert_eq!` on `f32` lets a `-0.0` for a `0.0` through and the
    /// render is not indifferent to that.
    fn assert_same_patch(a: &Patch, b: &Patch, what: &str) {
        for id in patch_ids() {
            let (x, y) = (value_of(a, id).unwrap(), value_of(b, id).unwrap());
            assert_eq!(
                x.to_bits(),
                y.to_bits(),
                "{what}: {} was {x}, came back {y}",
                desc(id).unwrap().name
            );
        }
        assert_same_topology(&a.matrix, &b.matrix, what);
    }

    fn assert_same_topology(a: &Matrix, b: &Matrix, what: &str) {
        for (i, (x, y)) in a.slots.iter().zip(b.slots.iter()).enumerate() {
            assert_eq!(x, y, "{what}: slot {i} did not round-trip");
        }
    }

    fn labelled_macros() -> Macros {
        let mut m = Macros::default();
        m[0] = MacroSpec {
            label: "Detune".to_string(),
            min: 0.0,
            max: 60.0,
            unit: " ct".to_string(),
        };
        m[4] = MacroSpec {
            label: "Damp".to_string(),
            ..MacroSpec::default()
        };
        m
    }

    // ── the guard ───────────────────────────────────────────────────────────

    /// A guard that returns `Ok` for a good path proves very little; these are
    /// the paths it exists to refuse.
    #[test]
    fn the_escape_guard_refuses_traversal_and_absolute_paths() {
        let dir = tmp();
        let base = dir.path();

        // Inside is fine — including a file that does not exist yet, which is
        // the Save-As case the parent-canonicalise branch exists for.
        assert!(ensure_within(base, &base.join("Glass Tine.toml")).is_ok());
        fs::create_dir(base.join("Keys")).unwrap();
        assert!(ensure_within(base, &base.join("Keys")).is_ok());
        assert!(ensure_within(base, &base.join("Keys/Tine.toml")).is_ok());

        // `..`, bare and buried mid-path. Canonicalisation is what collapses
        // these; a `starts_with` on the raw string would pass the third.
        for escape in [
            base.join("../outside.toml"),
            base.join("../../outside.toml"),
            base.join("Keys/../../outside.toml"),
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

    /// The not-yet-existing tail is where this guard is easy to get wrong, so it
    /// gets its own test rather than riding on the save path's.
    #[test]
    fn the_escape_guard_resolves_a_tail_that_does_not_exist_yet() {
        let dir = tmp();
        let base = dir.path();

        // Two missing components, not one: saving into a folder the save itself
        // will create. Canonicalising only the immediate parent refuses this.
        assert!(ensure_within(base, &base.join("NewFolder/New.toml")).is_ok());
        assert!(ensure_within(base, &base.join("a/b/c/d.toml")).is_ok());

        // `..` inside the missing tail is resolved rather than left in the path
        // for `starts_with` to skim over — this is the escape the component-wise
        // walk exists for.
        for escape in [
            base.join("NewFolder/../../outside.toml"),
            base.join("a/b/../../../outside.toml"),
        ] {
            let err = ensure_within(base, &escape)
                .expect_err(&format!("{} escaped the base", escape.display()));
            assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        }
        // ...and a `..` that stays inside is still inside.
        assert!(ensure_within(base, &base.join("NewFolder/../New.toml")).is_ok());
    }

    /// Symlinks are why the guard canonicalises rather than string-matches: a
    /// link planted *inside* the tree must not become a way out of it.
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
        // Including a file that does not exist yet under the link — the
        // parent-canonicalise branch has to follow the link too, or "save as"
        // becomes the way out that "delete" is not.
        assert!(ensure_within(base, &base.join("escape/new.toml")).is_err());
    }

    #[test]
    fn sanitize_name_keeps_the_safe_set_and_never_returns_empty() {
        assert_eq!(sanitize_name("Glass Tine"), "Glass Tine");
        assert_eq!(sanitize_name("Lead-2_alt"), "Lead-2_alt");
        // Separators and traversal characters are the ones that matter: this is
        // what stands between a browser-supplied name and a path.
        assert_eq!(sanitize_name("../etc/passwd"), "___etc_passwd");
        assert_eq!(sanitize_name("a/b\\c"), "a_b_c");
        assert_eq!(sanitize_name("nul\0byte"), "nul_byte");
        // Trimmed, and never empty — an empty filename is not a filename.
        assert_eq!(sanitize_name("  padded  "), "padded");
        assert_eq!(sanitize_name(""), "Untitled");
        assert_eq!(sanitize_name("   "), "Untitled");
        // Not "Untitled": every separator maps to `_`, so this is non-empty and
        // still one safe segment. Only an empty-after-trimming name falls back.
        assert_eq!(sanitize_name("///"), "___");
        // Non-ASCII alphanumerics survive; they are not path-dangerous.
        assert_eq!(sanitize_name("Café"), "Café");
    }

    #[test]
    fn unique_folder_name_counts_up_case_insensitively() {
        assert_eq!(unique_folder_name("Keys", &[]), "Keys");
        assert_eq!(unique_folder_name("Keys", &["keys".into()]), "Keys 1");
        assert_eq!(
            unique_folder_name("Keys", &["keys".into(), "keys 1".into()]),
            "Keys 2"
        );
        // A gap is filled, not skipped past.
        assert_eq!(
            unique_folder_name("Keys", &["keys".into(), "keys 2".into()]),
            "Keys 1"
        );
        assert_eq!(unique_folder_name("Keys", &["pads".into()]), "Keys");
    }

    // ── the file ops ────────────────────────────────────────────────────────

    #[test]
    fn folder_operations_round_trip_on_a_real_tree() {
        let dir = tmp();
        let base = dir.path();

        let (path, name) = create_user_folder_in(base, "Keys").unwrap();
        assert_eq!(name, "Keys");
        assert!(path.is_dir());

        // The same name again uniquifies rather than colliding or erroring.
        let (_, second) = create_user_folder_in(base, "Keys").unwrap();
        assert_eq!(second, "Keys 1");

        // A traversal-shaped name becomes a plain folder inside the base, not a
        // path that is followed.
        let (esc_path, esc_name) = create_user_folder_in(base, "../escape").unwrap();
        assert_eq!(esc_name, "___escape");
        assert_eq!(esc_path.parent().unwrap(), base);

        let (renamed, new_name) = rename_user_folder_in(base, "Keys", "Pads").unwrap();
        assert_eq!(new_name, "Pads");
        assert!(renamed.is_dir());
        assert!(!base.join("Keys").exists());

        // ...and refuses to clobber an existing destination.
        let err = rename_user_folder_in(base, "Keys 1", "Pads").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);

        delete_user_folder_in(base, "Pads").unwrap();
        assert!(!renamed.exists());
    }

    #[test]
    fn preset_operations_round_trip_on_a_real_tree() {
        let dir = tmp();
        let base = dir.path();
        let p = patch(1); // epiano
        let root_file =
            save_preset_into(base, &meta("Glass Tine"), &p, &Macros::default(), None).unwrap();
        assert_eq!(root_file.parent().unwrap(), base);

        // Rename rewrites the file *and* the embedded meta name, then removes
        // the old file. Reading the name back is what proves the second half.
        let renamed = rename_user_preset_in(base, &root_file, "Thin Tine").unwrap();
        assert_eq!(renamed.file_name().unwrap(), "Thin Tine.toml");
        assert!(!root_file.exists());
        assert_eq!(load_preset_file(&renamed).unwrap().meta.name, "Thin Tine");

        // Move into a folder, then back to the root.
        create_user_folder_in(base, "Keys").unwrap();
        let moved = move_user_preset_in(base, &renamed, Some("Keys")).unwrap();
        assert_eq!(moved.parent().unwrap(), base.join("Keys"));
        assert!(!renamed.exists());
        let back = move_user_preset_in(base, &moved, None).unwrap();
        assert_eq!(back.parent().unwrap(), base);

        delete_user_preset_in(base, &back).unwrap();
        assert!(!back.exists());
    }

    /// The guard is not merely present on these paths — it stops them, and it
    /// stops them *before* the destructive call.
    #[test]
    fn the_preset_operations_refuse_a_path_outside_the_base() {
        let dir = tmp();
        let outside = tmp();
        let base = dir.path();
        let intruder = outside.path().join("elsewhere.toml");
        fs::write(&intruder, "x").unwrap();

        for kind in [
            delete_user_preset_in(base, &intruder)
                .map(|_| ())
                .unwrap_err()
                .kind(),
            move_user_preset_in(base, &intruder, None)
                .map(|_| ())
                .unwrap_err()
                .kind(),
            rename_user_preset_in(base, &intruder, "Nope")
                .map(|_| ())
                .unwrap_err()
                .kind(),
        ] {
            assert_eq!(kind, io::ErrorKind::PermissionDenied);
        }
        // ...and the file is still there: refused, not deleted.
        assert!(intruder.exists());
    }

    /// A symlinked folder inside the tree is the interesting delete case: the
    /// name sanitises clean, the path exists, and `remove_dir_all` through it
    /// would take a directory the user never put in the bank.
    #[cfg(unix)]
    #[test]
    fn deleting_a_symlinked_folder_is_refused() {
        let dir = tmp();
        let outside = tmp();
        let base = dir.path();
        fs::write(outside.path().join("keepme.toml"), "x").unwrap();
        std::os::unix::fs::symlink(outside.path(), base.join("escape")).unwrap();

        let err = delete_user_folder_in(base, "escape").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(outside.path().join("keepme.toml").exists());
    }

    #[test]
    fn the_listing_walks_one_level_and_sorts() {
        let dir = tmp();
        let base = dir.path();
        let p = patch(0);
        for (name, folder) in [
            ("Zither", None),
            ("Anvil", None),
            ("Tine", Some("Keys")),
            ("Wire", Some("Keys")),
        ] {
            save_preset_into(base, &meta(name), &p, &Macros::default(), folder).unwrap();
        }
        create_user_folder_in(base, "Empty").unwrap();
        // Neither of these is a preset and neither may appear.
        fs::write(base.join("notes.txt"), "hello").unwrap();
        fs::create_dir_all(base.join("Keys/Deeper")).unwrap();
        save_preset_into(
            &base.join("Keys"),
            &meta("Buried"),
            &p,
            &Macros::default(),
            Some("Deeper"),
        )
        .unwrap();

        let tree = list_user_tree_in(base).unwrap();
        assert_eq!(tree[0].name, None, "the root group comes first");
        assert_eq!(
            tree[0].presets.iter().map(|p| &p.name).collect::<Vec<_>>(),
            ["Anvil", "Zither"]
        );
        let names: Vec<_> = tree[1..].iter().map(|f| f.name.clone().unwrap()).collect();
        assert_eq!(names, ["Empty", "Keys"], "folders sort, empties survive");
        assert_eq!(
            tree[2].presets.iter().map(|p| &p.name).collect::<Vec<_>>(),
            ["Tine", "Wire"],
            "the second level is not descended into"
        );
        assert_eq!(tree[2].presets[0].folder.as_deref(), Some("Keys"));
    }

    // ── the store ───────────────────────────────────────────────────────────

    /// The browser reaches the bank through `PresetStore`, not through
    /// [`crate::patch`](mod@crate::patch) directly, so exercise that seam: every entry must
    /// enumerate, expose meta, and decode to a blob that is the patch it names.
    #[test]
    fn the_factory_bank_loads_through_the_store_without_touching_the_disk() {
        let store = EnginePresetStore::new();
        assert_eq!(store.factory_len(), N_PATCHES);

        for i in 0..store.factory_len() {
            let m = store.factory_meta(i).expect("meta for every index");
            assert_eq!(m.name, patch_names()[i]);
            assert_eq!(m.category.as_deref(), Some(FACTORY_CATEGORY));

            let load = store
                .factory_load(i)
                .unwrap_or_else(|e| panic!("factory preset `{}` failed to load: {e}", m.name));
            assert!(
                load.warnings.is_empty(),
                "`{}` warned: {:?}",
                m.name,
                load.warnings
            );
            assert_eq!(load.meta, m);
            // The blob is the patch, field for field — the store must not be
            // quietly handing the browser patch 0 seven times.
            let back = decode_blob(&load.blob).unwrap();
            assert!(back.warnings.is_empty(), "{:?}", back.warnings);
            assert_same_patch(&patch(i), &back.patch, m.name.as_str());
        }
    }

    /// `patch` wraps its index modulo the bank; the store must not. A stale
    /// browser index has to say so rather than open a different sound.
    #[test]
    fn a_factory_index_past_the_end_is_an_error_not_a_wrap() {
        let store = EnginePresetStore::new();
        assert!(store.factory_load(N_PATCHES).is_err());
        assert!(store.factory_meta(N_PATCHES).is_none());
        assert!(store.factory_load(usize::MAX).is_err());
    }

    /// E052's acceptance for this ticket: a preset written here loads back
    /// identically — through a folder rename and a move as well as straight.
    #[test]
    fn a_saved_preset_loads_back_identically_through_a_rename_and_a_move() {
        let dir = tmp();
        let store = EnginePresetStore::rooted_at(dir.path());

        // Start from a factory patch with a matrix worth losing, plus macro
        // labels, so a codec that dropped either would be caught here.
        let p = patch(4); // web — every route live
        let macros = labelled_macros();
        let blob = encode_blob(&meta("Web Thing"), &p, &macros).unwrap();
        let file_meta = PresetMeta {
            name: "Web Thing".to_string(),
            author: Some("df".to_string()),
            category: Some("pads".to_string()),
            comment: Some("measured at -6 dBFS".to_string()),
        };

        store.user_create_folder("Keys").unwrap();
        let path = store
            .user_save("Web Thing", Some("Keys"), &file_meta, &blob)
            .unwrap();

        let check = |path: &Path, what: &str| {
            let load = store.user_load(path).unwrap();
            assert!(load.warnings.is_empty(), "{what}: {:?}", load.warnings);
            assert_eq!(load.meta, file_meta, "{what}: meta");
            let back = decode_blob(&load.blob).unwrap();
            assert_same_patch(&p, &back.patch, what);
            assert_eq!(back.macros, macros, "{what}: macro labels");
        };
        check(&path, "as written");

        // A folder rename moves the file without rewriting it...
        let (folder, _) = store.user_rename_folder("Keys", "Pads").unwrap();
        let after_rename = folder.join(path.file_name().unwrap());
        check(&after_rename, "after a folder rename");

        // ...and a move back to the root does the same.
        let at_root = store.user_move(&after_rename, None).unwrap();
        assert_eq!(at_root.parent().unwrap(), dir.path());
        check(&at_root, "after a move to the root");
    }

    /// The whole point of the corpus: the seven baked patches and the user's
    /// files arrive through one object, and only one of the two halves is
    /// writable.
    #[test]
    fn the_store_presents_factory_and_user_entries_as_one_corpus() {
        let dir = tmp();
        let store = EnginePresetStore::rooted_at(dir.path());
        let empty = store.list_user_tree();
        assert_eq!(empty.len(), 1, "the root group exists with nothing in it");
        assert!(empty[0].presets.is_empty());

        let blob = store.factory_load(1).unwrap().blob;
        // "Save over a factory preset" is an ordinary user save under the same
        // name. The bank is untouched and the two entries coexist.
        let saved = store
            .user_save(patch_names()[1], None, &PresetMeta::default(), &blob)
            .unwrap();
        assert!(saved.starts_with(dir.path()));
        assert_eq!(store.factory_len(), N_PATCHES);
        assert_eq!(store.factory_meta(1).unwrap().name, patch_names()[1]);

        let tree = store.list_user_tree();
        assert_eq!(tree.len(), 1, "one root group, no folders");
        assert_eq!(tree[0].name, None);
        assert_eq!(tree[0].presets.len(), 1);
        assert_eq!(tree[0].presets[0].meta.name, patch_names()[1]);
        assert_eq!(tree[0].presets[0].folder, None);
        assert_eq!(tree[0].presets[0].path, saved);
    }

    /// Names from a browser are strings, not paths, on every mutating call.
    #[test]
    fn the_store_refuses_a_user_path_outside_its_root() {
        let dir = tmp();
        let outside = tmp();
        let store = EnginePresetStore::rooted_at(dir.path());
        let intruder = outside.path().join("elsewhere.toml");
        fs::write(&intruder, "x").unwrap();

        assert!(store.user_delete(&intruder).is_err());
        assert!(store.user_move(&intruder, None).is_err());
        assert!(store.user_rename(&intruder, "Nope").is_err());
        assert!(intruder.exists());

        // A traversal-shaped *name* lands inside the root, sanitised.
        let (folder, name) = store.user_create_folder("../../escape").unwrap();
        assert_eq!(name, "______escape");
        assert_eq!(folder.parent().unwrap(), dir.path());
        let blob = store.factory_load(0).unwrap().blob;
        let saved = store
            .user_save(
                "../../pwned",
                Some("../../escape"),
                &PresetMeta::default(),
                &blob,
            )
            .unwrap();
        assert_eq!(saved.parent().unwrap(), folder);
        assert!(!outside.path().join("pwned.toml").exists());
    }

    /// The controller hands `user_save` whatever the model produced. A blob it
    /// cannot read is an error, never a file full of defaults.
    #[test]
    fn a_blob_that_is_not_a_preset_fails_the_save_rather_than_writing_defaults() {
        let dir = tmp();
        let store = EnginePresetStore::rooted_at(dir.path());
        for blob in [
            &b"\xff\xfe not utf-8"[..],
            &b"schema = 99\n[meta]\nname = \"X\"\n"[..],
        ] {
            assert!(
                store
                    .user_save("Bad", None, &PresetMeta::default(), blob)
                    .is_err()
            );
        }
        assert!(store.list_user_tree()[0].presets.is_empty());
    }
}
