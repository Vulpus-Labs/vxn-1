//! Embedded factory-preset bank (E007 lineage, ADR 0005 §4).
//!
//! The factory presets live as a source tree under
//! `crates/vxn2-engine/presets/factory/<Category>/<name>.toml` and are baked
//! into the binary at compile time with [`include_dir!`]. No install step,
//! nothing to lose at runtime, identical across DAWs and OSes, and the
//! round-trip test below validates the whole bank at build time (a malformed
//! factory preset fails CI).
//!
//! The directory name is the **category** (the browser's grouping); the
//! file's `[meta] name` is its display name. The `PresetStore`
//! ([`crate::preset_io`]) consumes [`factory`] without ever touching the
//! filesystem.

use crate::preset::{Meta, from_toml_str};
use include_dir::{Dir, include_dir};

/// The embedded factory source tree. The TOML bytes are baked into the
/// `vxn2-engine` rlib at compile time; the bank is fully validated by the
/// round-trip test below, which links and reads every embedded file.
static FACTORY: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/presets/factory");

/// One embedded factory preset: its category (directory), display name, and
/// the raw TOML source (parsed on demand by [`crate::preset_io`]).
#[derive(Clone, Debug)]
pub struct FactoryPreset {
    /// Category = the immediate parent directory, e.g. `"Keys"`.
    pub category: String,
    /// Display name from `[meta] name`.
    pub name: String,
    /// Parsed metadata (name/author/category/comment from the file).
    pub meta: Meta,
    /// Raw TOML, re-parsed to a host-state blob on load.
    pub contents: &'static str,
}

/// Walk the embedded tree, yielding `(category, contents)` for every
/// `*.toml` file one level deep (`<category>/<file>.toml`). Shared by
/// [`factory`] and the bank's CI test so both see exactly the same files.
fn factory_files() -> Vec<(String, &'static str)> {
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
            out.push((category.clone(), contents));
        }
    }
    out
}

/// All embedded factory presets, sorted by category then display name. A file
/// that fails to parse is skipped (the CI test guarantees the shipped bank
/// never has one).
pub fn factory() -> Vec<FactoryPreset> {
    let mut out: Vec<FactoryPreset> = factory_files()
        .into_iter()
        .filter_map(|(category, contents)| {
            let (meta, _blob, _warnings) = from_toml_str(contents).ok()?;
            Some(FactoryPreset {
                category,
                name: meta.name.clone(),
                meta,
                contents,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        a.category
            .to_lowercase()
            .cmp(&b.category.to_lowercase())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out
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
        // schema and produces ZERO warnings (no unknown keys, no bad enum
        // labels, no type mismatches). A malformed factory preset fails here.
        for (category, contents) in factory_files() {
            match from_toml_str(contents) {
                Ok((meta, _blob, warnings)) => assert!(
                    warnings.is_empty(),
                    "factory preset `{}/{}` produced warnings: {warnings:?}",
                    category,
                    meta.name
                ),
                Err(e) => panic!("factory preset in `{category}` failed to parse: {e}"),
            }
        }
    }

    /// E008 0097 durable guard: every routed slot in every factory preset must
    /// be coherent (per the 0090 predicate) and point at a *consumed* dest. A
    /// future preset that reintroduces an incoherent route (finer source into a
    /// coarser dest, an LFO into its own rate, or a degenerate `voice-idx`
    /// collapse) — or that ever pointed at a now-removed dest — fails CI here.
    /// This is the keystone test: it keeps the matrix honest as presets grow.
    #[test]
    fn no_factory_preset_routes_incoherently() {
        use crate::matrix::{coherence, Coherence, DestId, SourceId};
        use crate::preset::read_preset;

        for (category, contents) in factory_files() {
            let (meta, _params, matrix, _warnings) =
                read_preset(contents).expect("factory preset parses");
            for (slot, row) in matrix.iter().enumerate() {
                if !row.active {
                    continue;
                }
                let src = SourceId::from_u8(row.source);
                let dst = DestId::from_u8(row.dest);
                let verdict = coherence(src, dst);
                assert_eq!(
                    verdict,
                    Coherence::Ok,
                    "factory preset `{}/{}` slot {slot} routes {src:?} → {dst:?} \
                     incoherently ({verdict:?}); repoint to a coherent pair",
                    category,
                    meta.name
                );
            }
        }
    }

    #[test]
    fn covers_multiple_categories() {
        let bank = factory();
        let categories: std::collections::BTreeSet<_> =
            bank.iter().map(|p| p.category.as_str()).collect();
        assert!(
            categories.len() >= 5,
            "expected presets across several categories, got {categories:?}"
        );
    }

    #[test]
    fn names_are_present() {
        for p in factory() {
            assert!(!p.name.trim().is_empty(), "factory preset with empty name in {}", p.category);
        }
    }
}
