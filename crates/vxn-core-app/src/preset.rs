//! Preset corpus model + persistence trait.
//!
//! The factory bank is read-only and indexed; the user side is mutable
//! and lives on disk. The synth supplies a concrete [`PresetStore`]
//! impl; the controller drives the IO surface and republishes the
//! shared snapshot ([`crate::CorpusHandle`]) after every disk-mutating
//! op.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Slim preset metadata the controller hands to the view.
///
/// The serde-derived file shape (vxn-1's `LayerBlock` / `GlobalBlock`,
/// vxn-2's eventual schema) lives next to each synth's format. This is
/// the view-facing projection.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PresetMeta {
    pub name: String,
    pub author: Option<String>,
    pub category: Option<String>,
    pub comment: Option<String>,
}

/// A preset payload, ready to apply to the model.
///
/// `blob` is the same format the model accepts via
/// [`crate::ParamModel::restore_from_bytes`] — the controller bridges
/// file load to model restore through this byte channel without knowing
/// the schema.
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

/// One folder slot in the corpus listing. `name == None` is the virtual
/// root.
#[derive(Clone, Debug)]
pub struct UserFolderEntry {
    pub name: Option<String>,
    pub presets: Vec<UserPresetEntry>,
}

/// The preset corpus, as the browser sees it.
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

    /// Rename a user subfolder. Returns the new path and the chosen
    /// (sanitised) name.
    fn user_rename_folder(&self, old: &str, new: &str) -> Result<(PathBuf, String), String>;
    fn user_delete_folder(&self, name: &str) -> Result<(), String>;

    fn list_user_tree(&self) -> Vec<UserFolderEntry>;
}

/// Serialise a [`PresetCorpus`] for the JS browser panel. Factory
/// presets are grouped by `meta.category` (presets without a category
/// fall into `uncategorised_label`); user folders preserve their
/// `Option<String>` shape so the page can show the root group first
/// then sorted named folders. Within each group, presets are
/// alpha-sorted by name (case-insensitive) — same order the prev/next
/// walker uses.
///
/// Lives here (model layer) rather than in `vxn-core-ui-web` so both the
/// desktop wry editor and the wasm web controller (which deps only
/// `vxn-app`, never wry — ADR 0009) build a byte-identical payload.
pub fn corpus_snapshot_json(corpus: &PresetCorpus, uncategorised_label: &str) -> String {
    use serde_json::{Value, json};

    let mut factory_groups: HashMap<String, Vec<(usize, &str)>> = HashMap::new();
    for (i, m) in corpus.factory.iter().enumerate() {
        let cat = m
            .category
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(uncategorised_label)
            .to_string();
        factory_groups
            .entry(cat)
            .or_default()
            .push((i, m.name.as_str()));
    }
    let mut factory: Vec<(String, Vec<(usize, &str)>)> = factory_groups.into_iter().collect();
    factory.sort_by_cached_key(|a| a.0.to_lowercase());
    for g in factory.iter_mut() {
        g.1.sort_by_cached_key(|a| a.1.to_lowercase());
    }
    let factory_v: Vec<Value> = factory
        .into_iter()
        .map(|(category, presets)| {
            let entries: Vec<Value> = presets
                .into_iter()
                .map(|(idx, name)| json!({"name": name, "index": idx}))
                .collect();
            json!({"category": category, "presets": entries})
        })
        .collect();

    let mut user = corpus.user.clone();
    user.sort_by(|a, b| match (&a.name, &b.name) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, _) => std::cmp::Ordering::Less,
        (_, None) => std::cmp::Ordering::Greater,
        (Some(x), Some(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
    });
    let user_v: Vec<Value> = user
        .into_iter()
        .map(|f| {
            let mut presets = f.presets;
            presets.sort_by_cached_key(|a| a.meta.name.to_lowercase());
            let entries: Vec<Value> = presets
                .into_iter()
                .map(|p| {
                    json!({"name": p.meta.name, "path": p.path.display().to_string()})
                })
                .collect();
            json!({"name": f.name, "presets": entries})
        })
        .collect();
    json!({"factory": factory_v, "user": user_v}).to_string()
}
