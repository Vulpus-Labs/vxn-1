//! Preset store trait (ADR 0007 §1, §3) — the IO surface the controller calls
//! into for factory + user-dir operations. `vxn-engine` provides the concrete
//! impl backed by `preset_io`; tests use a tempdir-backed or in-memory fake.

use std::path::{Path, PathBuf};

use crate::domain::PresetMeta;

/// A preset payload, ready to apply to the model.
///
/// `blob` is the same format the model accepts via
/// `ParamModel::restore_from_bytes` — the controller bridges file load to
/// model restore through this byte channel without knowing the schema.
#[derive(Clone, Debug)]
pub struct PresetLoad {
    pub meta: PresetMeta,
    pub blob: Vec<u8>,
    pub warnings: Vec<String>,
}

/// One on-disk user preset in the corpus listing.
#[derive(Clone, Debug)]
pub struct UserPresetEntry {
    pub path: PathBuf,
    pub meta: PresetMeta,
    /// `None` = lives at the user-dir root; `Some(_)` = subfolder name.
    pub folder: Option<String>,
}

/// One folder slot in the corpus listing. `name == None` is the virtual root.
#[derive(Clone, Debug)]
pub struct UserFolderEntry {
    pub name: Option<String>,
    pub presets: Vec<UserPresetEntry>,
}

/// The preset corpus, as the browser sees it. Factory bank is read-only and
/// indexed; user side is mutable and lives on disk.
#[derive(Clone, Debug, Default)]
pub struct PresetCorpus {
    pub factory: Vec<PresetMeta>,
    pub user: Vec<UserFolderEntry>,
}

/// Preset persistence: factory bank reads + user-dir IO. Held by the
/// controller; injected by the host shell.
pub trait PresetStore: Send + 'static {
    fn factory_len(&self) -> usize;
    fn factory_load(&self, index: usize) -> Result<PresetLoad, String>;
    fn factory_meta(&self, index: usize) -> Option<PresetMeta>;

    fn user_load(&self, path: &Path) -> Result<PresetLoad, String>;

    /// Save the model's snapshot + meta to the user dir. `folder = None`
    /// writes to the root. Returns the path written.
    fn user_save(
        &self,
        name: &str,
        folder: Option<&str>,
        meta: &PresetMeta,
        blob: &[u8],
    ) -> Result<PathBuf, String>;

    fn user_delete(&self, path: &Path) -> Result<(), String>;
    fn user_rename(&self, path: &Path, new_name: &str) -> Result<PathBuf, String>;
    fn user_move(&self, path: &Path, dest_folder: Option<&str>) -> Result<PathBuf, String>;
    fn user_create_folder(&self, suggested: &str) -> Result<(PathBuf, String), String>;

    /// Rename a user subfolder. Returns the new path and the chosen (sanitised)
    /// name.
    fn user_rename_folder(
        &self,
        old: &str,
        new: &str,
    ) -> Result<(PathBuf, String), String>;
    fn user_delete_folder(&self, name: &str) -> Result<(), String>;

    fn list_user_tree(&self) -> Vec<UserFolderEntry>;
}
