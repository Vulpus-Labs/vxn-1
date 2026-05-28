//! User-preset filesystem IO (E007 / 0026, ADR 0005 §5).
//!
//! Resolves the per-OS **user** preset directory, saves a [`Patch`] /
//! [`Performance`] as a `<name>.toml`, enumerates the directory for the browser,
//! and loads a single file. All **main/UI-thread** work: the audio thread never
//! touches the filesystem or serde (ADR 0005 §6, ticket 0026 §Threading).
//!
//! The factory bank is embedded, not on disk ([`crate::factory`]); this module
//! is only the writable user side. Save-As name validation is intentionally
//! minimal here and polished in the browser (0027).

use crate::preset::{Meta, Patch, Performance, Preset, PresetError, from_toml_str};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

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

/// Minimal filename derivation: keep alphanumerics, space, `-` and `_`; map
/// anything else (path separators, `.`, etc.) to `_`. Empty → `"Untitled"`.
fn preset_filename(name: &str) -> String {
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
    let base = if trimmed.is_empty() {
        "Untitled"
    } else {
        trimmed
    };
    format!("{base}.toml")
}

fn write_preset(dir: &Path, name: &str, toml: &str) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join(preset_filename(name));
    fs::write(&path, toml)?;
    Ok(path)
}

/// Serialize and save a [`Patch`] to the user directory as `<name>.toml`.
/// Returns the written path.
pub fn save_patch(patch: &Patch) -> io::Result<PathBuf> {
    let dir = user_preset_dir().ok_or_else(no_dir_err)?;
    write_preset(&dir, &patch.meta.name, &patch.to_toml_string())
}

/// Serialize and save a [`Performance`] to the user directory as `<name>.toml`.
/// Returns the written path.
pub fn save_performance(perf: &Performance) -> io::Result<PathBuf> {
    let dir = user_preset_dir().ok_or_else(no_dir_err)?;
    write_preset(&dir, &perf.meta.name, &perf.to_toml_string())
}

/// A user preset on disk, for the browser's listing.
#[derive(Clone, Debug)]
pub struct UserPreset {
    pub path: PathBuf,
    /// Display name from `[meta] name`.
    pub name: String,
    /// `"patch"` or `"performance"`.
    pub kind: &'static str,
}

/// Enumerate the user directory's `*.toml` presets, sorted by display name.
/// Files that don't parse are skipped (a stray/corrupt file shouldn't break the
/// browser). Returns an empty list if the directory doesn't exist yet.
pub fn list_user_presets() -> io::Result<Vec<UserPreset>> {
    let Some(dir) = user_preset_dir() else {
        return Ok(Vec::new());
    };
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        if let Ok(contents) = fs::read_to_string(&path) {
            if let Ok((preset, _warnings)) = from_toml_str(&contents) {
                out.push(UserPreset {
                    name: preset.meta().name.clone(),
                    kind: preset.kind_str(),
                    path,
                });
            }
        }
    }
    out.sort_by_key(|p| p.name.to_lowercase());
    Ok(out)
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
pub fn load_preset_file(path: &Path) -> Result<(Preset, Vec<String>), LoadError> {
    let contents = fs::read_to_string(path).map_err(LoadError::Io)?;
    from_toml_str(&contents).map_err(LoadError::Parse)
}

/// Construct a [`Patch`]/[`Performance`] [`Meta`] with just a name (Save-As). A
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
    use crate::params::{KeyMode, Layer, PatchParam};
    use crate::state::PluginState;
    use crate::ParamValues;

    fn temp_dir() -> PathBuf {
        // Hermetic per-process scratch dir; never the real user preset dir.
        std::env::temp_dir().join(format!("vxn1-preset-io-test-{}", std::process::id()))
    }

    #[test]
    fn filename_is_sanitized() {
        assert_eq!(preset_filename("Mini Bass"), "Mini Bass.toml");
        assert_eq!(preset_filename("a/b:c*?"), "a_b_c__.toml");
        assert_eq!(preset_filename("   "), "Untitled.toml");
    }

    #[test]
    fn write_then_load_round_trips_a_performance() {
        let dir = temp_dir();
        let _ = fs::remove_dir_all(&dir);

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

        let path = write_preset(&dir, &perf.meta.name, &perf.to_toml_string()).unwrap();
        assert!(path.ends_with("Round Trip.toml"));

        let (loaded, warnings) = load_preset_file(&path).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let Preset::Performance(p) = loaded else {
            panic!("expected a performance");
        };
        assert_eq!(p.state.key_mode, KeyMode::Split);
        assert_eq!(p.state.split_point, 48);
        assert_eq!(
            p.state.params.layer(Layer::Upper).get(PatchParam::Cutoff),
            1234.0
        );

        // Enumeration finds it.
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
