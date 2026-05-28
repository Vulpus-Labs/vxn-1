//! Embedded factory-preset bank (E007 / 0025, ADR 0005 §4).
//!
//! The factory presets live as a source tree under
//! `crates/vxn-engine/presets/factory/<Category>/<name>.toml` and are baked into
//! the binary at compile time with [`include_dir!`]. No install step, nothing to
//! lose at runtime, identical across DAWs and OSes, and the round-trip test below
//! validates the whole bank at build time (a malformed factory preset fails CI).
//!
//! The directory name is the **category** (the browser's grouping); the file's
//! [`Meta::name`](crate::preset::Meta) is its display name. The browser (0027)
//! consumes [`factory()`] without ever touching the filesystem.

use crate::preset::{Preset, from_toml_str};
use include_dir::{Dir, include_dir};

/// The embedded factory source tree.
///
/// The TOML bytes are baked into the `vxn-engine` rlib at compile time. In the
/// final `cdylib`, thin-LTO will dead-code-eliminate this static until something
/// reachable from an exported symbol calls [`factory()`] — that happens when the
/// browser (0027) reads the bank from the editor. (Compare the `#[used]`
/// `_PARAM_COUNT` anchor in `vxn-clap`.) The bank is fully validated regardless
/// by the round-trip test below, which links and reads every embedded file.
static FACTORY: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/presets/factory");

/// One embedded factory preset: its relative path, category (directory), display
/// name, and the parsed [`Preset`].
#[derive(Clone, Debug)]
pub struct FactoryPreset {
    /// Path relative to the factory root, e.g. `"Bass/Mini Bass.toml"`.
    pub path: String,
    /// Category = the immediate parent directory, e.g. `"Bass"`.
    pub category: String,
    /// Display name from `[meta] name`.
    pub name: String,
    pub preset: Preset,
}

/// Walk the embedded tree, yielding `(relative_path, category, contents)` for
/// every `*.toml` file one level deep (`<category>/<file>.toml`). Shared by
/// [`factory()`] and the bank's CI test so both see exactly the same files.
fn factory_files() -> Vec<(String, String, &'static str)> {
    let mut out = Vec::new();
    for category_dir in FACTORY.dirs() {
        let category = category_dir
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        for file in category_dir.files() {
            if file.path().extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let Some(contents) = file.contents_utf8() else {
                continue;
            };
            let path = file.path().to_string_lossy().into_owned();
            out.push((path, category.clone(), contents));
        }
    }
    out
}

/// All embedded factory presets, parsed. A file that fails to parse is skipped
/// (the CI test guarantees the shipped bank never has one); parse warnings are
/// likewise dropped here — the test asserts the bank produces none.
pub fn factory() -> Vec<FactoryPreset> {
    factory_files()
        .into_iter()
        .filter_map(|(path, category, contents)| {
            let (preset, _warnings) = from_toml_str(contents).ok()?;
            let name = preset.meta().name.clone();
            Some(FactoryPreset {
                path,
                category,
                name,
                preset,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bank_is_non_empty() {
        assert!(!factory_files().is_empty(), "no factory presets embedded");
    }

    #[test]
    fn every_factory_preset_parses_cleanly() {
        // The shippable contract: each embedded file parses with the current
        // schema/kind and produces ZERO warnings (no unknown keys, no bad enum
        // labels, no type mismatches). A malformed factory preset fails here.
        for (path, _category, contents) in factory_files() {
            match from_toml_str(contents) {
                Ok((_preset, warnings)) => {
                    assert!(
                        warnings.is_empty(),
                        "factory preset `{path}` produced warnings: {warnings:?}"
                    );
                }
                Err(e) => panic!("factory preset `{path}` failed to parse: {e}"),
            }
        }
    }

    #[test]
    fn covers_multiple_categories_and_a_performance() {
        let bank = factory();
        let categories: std::collections::BTreeSet<_> =
            bank.iter().map(|p| p.category.as_str()).collect();
        assert!(
            categories.len() >= 3,
            "expected presets across several categories, got {categories:?}"
        );
        assert!(
            bank.iter()
                .any(|p| matches!(p.preset, Preset::Performance(_))),
            "starter bank should include at least one performance"
        );
    }
}
