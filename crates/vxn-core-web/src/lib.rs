//! Shared browser-glue JS for the VXN web ports (ticket 0284, epic E045).
//!
//! There is no Rust in the shipped path here — the crate exists to give the
//! shared `assets/*.mjs` a home in the workspace and, more usefully, to hold the
//! tests that keep them shared. VXN1b is the third browser port; before this
//! crate, six of the fourteen glue modules were duplicated between vxn-1 and
//! vxn-2, two of them byte-identical and the rest differing only in comments and
//! two pieces of configuration.
//!
//! What the tests below defend:
//!
//! 1. [`MODULES`] matches what is actually on disk, so a module can't be added
//!    to `assets/` and silently left out of the ports' bundles (or vice versa).
//! 2. No shared module hardcodes a **per-port configuration value**. The
//!    IndexedDB identity and the product name are caller-supplied precisely so
//!    that one copy can serve every synth; a literal creeping back in is the
//!    first step of re-forking, and it would be invisible until a user's presets
//!    landed in the wrong database.
//!
//! Prose *may* name a synth — `preset-storage.mjs` explains why vxn-1's database
//! is at v2 and vxn-2's at v1, which is exactly the kind of thing a reader needs.
//! The guard is about code, so it looks for the configuration values themselves.

/// The shared modules, by filename. Each port's `xtask web` copies these out of
/// `assets/` into its flat `dist/`, alongside its own eight.
pub const MODULES: [&str; 8] = [
    // IndexedDB primitive + the two write-behind owners layered on it.
    "preset-storage.mjs",
    "preset-persistence.mjs",
    "state-autosave.mjs",
    // Off-device patch transfer: `.toml` file + `#patch=` share link.
    "patch-io.mjs",
    // Browser input → event-ring producers.
    "midi-input.mjs",
    "keyboard-input.mjs",
    "piano-keyboard.mjs",
    // Web-only chrome: the render-load badge.
    "cpu-meter.mjs",
];

/// Source of one shared module, embedded at compile time. `None` for a name
/// that isn't in [`MODULES`].
///
/// Embedding rather than reading from disk keeps this usable from a test binary
/// run out of any working directory, and means a renamed asset fails the build
/// rather than an assertion.
pub fn module_source(name: &str) -> Option<&'static str> {
    Some(match name {
        "preset-storage.mjs" => include_str!("../assets/preset-storage.mjs"),
        "preset-persistence.mjs" => include_str!("../assets/preset-persistence.mjs"),
        "state-autosave.mjs" => include_str!("../assets/state-autosave.mjs"),
        "patch-io.mjs" => include_str!("../assets/patch-io.mjs"),
        "midi-input.mjs" => include_str!("../assets/midi-input.mjs"),
        "keyboard-input.mjs" => include_str!("../assets/keyboard-input.mjs"),
        "piano-keyboard.mjs" => include_str!("../assets/piano-keyboard.mjs"),
        "cpu-meter.mjs" => include_str!("../assets/cpu-meter.mjs"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `assets/` and [`MODULES`] must describe the same set. Catches both
    /// directions: a new shared module that no port bundles, and a stale entry
    /// whose file is gone.
    #[test]
    fn modules_list_matches_the_assets_directory() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/assets");
        let mut on_disk: Vec<String> = std::fs::read_dir(dir)
            .expect("assets/ readable")
            .map(|e| e.expect("dir entry").file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".mjs"))
            .collect();
        on_disk.sort();

        let mut declared: Vec<String> = MODULES.iter().map(|m| m.to_string()).collect();
        declared.sort();

        assert_eq!(declared, on_disk, "MODULES vs assets/ contents");
    }

    #[test]
    fn every_declared_module_has_a_source() {
        for m in MODULES {
            let src = module_source(m).unwrap_or_else(|| panic!("no source embedded for {m}"));
            assert!(!src.is_empty(), "{m} is empty");
        }
    }

    /// The anti-refork guard. These are the per-port configuration values that
    /// motivated the extraction: if one is hardcoded again, the module has
    /// quietly become vxn-1's (or vxn-2's) rather than everyone's, and the next
    /// port will fork it instead of configuring it.
    ///
    /// Deliberately matched case-insensitively and without word boundaries — the
    /// point is to catch the value in any spelling that could reach a running
    /// page, not to be precise about identifiers.
    #[test]
    fn no_shared_module_hardcodes_a_per_port_config_value() {
        // IndexedDB database names (`openPresetDB`'s `dbId.name`) and the
        // product patch names (`exportPatchFile`'s `product`).
        const FORBIDDEN: [&str; 6] = [
            "vxn1-presets",
            "vxn2-presets",
            "vxn1b-presets",
            "VXN1 Patch",
            "VXN2 Patch",
            "VXN1b Patch",
        ];
        for m in MODULES {
            let src = module_source(m).expect("declared module has a source");
            let lower = src.to_lowercase();
            for needle in FORBIDDEN {
                assert!(
                    !lower.contains(&needle.to_lowercase()),
                    "{m} hardcodes the per-port value {needle:?} — pass it in \
                     (dbId / product) instead of baking it into the shared copy",
                );
            }
        }
    }

    /// `openPresetDB` has no default database identity, and must not grow one:
    /// a default is how a port silently opens (and upgrades) another port's
    /// corpus. The contract is "reject rather than guess".
    #[test]
    fn open_preset_db_refuses_to_guess_a_database() {
        let src = module_source("preset-storage.mjs").expect("source");
        assert!(
            src.contains("export function openPresetDB(indexedDB = globalThis.indexedDB, db = null)"),
            "openPresetDB's db identity must stay un-defaulted",
        );
        assert!(
            src.contains("openPresetDB needs a { name, version } DB identity"),
            "openPresetDB must reject a missing identity",
        );
    }
}
