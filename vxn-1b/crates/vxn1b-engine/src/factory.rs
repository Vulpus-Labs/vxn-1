//! Embedded factory-preset bank (0212, E038).
//!
//! The presets live as a source tree under
//! `crates/vxn1b-engine/presets/factory/<Category>/<name>.toml` and are baked
//! into the binary with [`include_dir!`]. No install step, nothing to lose at
//! runtime, identical across DAWs and OSes, and the tests below validate the
//! whole bank at build time — a malformed factory preset fails CI.
//!
//! Structure follows vxn-2's bank ([`vxn2-engine/src/factory.rs`]): the
//! directory name is the **category** and the file's `[meta] name` is the
//! display name. [`crate::preset_io::EnginePresetStore`] consumes [`factory`]
//! without touching the filesystem.
//!
//! Two things carry a category and they are not the same field. The browser
//! GROUPS on `[meta] category` out of the TOML; the directory name lands in
//! [`FactoryPreset::category`] and drives only the bank's SORT order. Nothing in
//! the loader forces them to agree, so a file moved between directories without
//! its `[meta]` being updated would sort under one heading and group under
//! another. [`tests::the_directory_is_the_meta_category`] is what makes that
//! fail loudly instead.
//!
//! **Editing the bank does not trigger a rebuild.** `include_dir!` emits no
//! `rerun-if-changed`, so adding or changing a TOML leaves a stale bank baked
//! into the rlib — touch this file before an install
//! ([[vxn2-include-dir-no-rerun]]). `vxn-1b/xtask` does that for you.

use include_dir::{Dir, include_dir};

use crate::preset::{Meta, read_preset};
use crate::state::PluginState;

/// The embedded factory source tree, baked into the `vxn1b-engine` rlib.
static FACTORY: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/presets/factory");

/// One embedded factory preset.
#[derive(Clone, Debug)]
pub struct FactoryPreset {
    /// Category = the immediate parent directory, e.g. `"Bass"`. Drives the
    /// bank's sort order; the browser groups on `meta.category`, which a test
    /// pins to this (see the module docs).
    pub category: String,
    /// Display name from `[meta] name`.
    pub name: String,
    /// Parsed metadata.
    pub meta: Meta,
    /// Raw TOML, re-parsed to a [`PluginState`] on load.
    pub contents: &'static str,
}

/// Walk the embedded tree, yielding `(category, contents)` for every `*.toml`
/// one level deep. Shared by [`factory`] and the tests, so both see exactly the
/// same files.
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
/// that fails to parse is skipped — the tests guarantee the shipped bank never
/// has one, so this is belt-and-braces rather than a real path.
pub fn factory() -> Vec<FactoryPreset> {
    let mut out: Vec<FactoryPreset> = factory_files()
        .into_iter()
        .filter_map(|(category, contents)| {
            let (meta, _state, _warnings) = read_preset(contents).ok()?;
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

/// Parse one embedded preset by its index in [`factory`].
pub fn load(index: usize) -> Option<(Meta, PluginState, Vec<String>)> {
    let bank = factory();
    let p = bank.get(index)?;
    read_preset(p.contents).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matrix::{DestId, SourceId};

    #[test]
    fn bank_is_non_empty() {
        assert!(!factory_files().is_empty(), "no factory presets embedded");
    }

    /// The directory a preset lives in and the `[meta] category` it declares
    /// must be the same string. They feed different things — the directory
    /// sorts the bank, `meta.category` is what the browser groups on — so a
    /// file moved without its `[meta]` updated would sort under one heading and
    /// appear under another, with nothing failing.
    #[test]
    fn the_directory_is_the_meta_category() {
        for (dir, contents) in factory_files() {
            let (meta, _state, _warnings) =
                read_preset(contents).unwrap_or_else(|e| panic!("`{dir}` failed to parse: {e:?}"));
            let declared = meta.category.as_deref().unwrap_or_else(|| {
                panic!("factory preset `{dir}/{}` declares no [meta] category", meta.name)
            });
            assert_eq!(
                declared, dir,
                "factory preset `{}` sits in `{dir}/` but declares category `{declared}` — \
                 the browser would group it under one and sort it under the other",
                meta.name
            );
        }
    }

    /// The shippable contract: every file parses under the current schema and
    /// produces **zero** warnings — no unknown keys, no bad enum labels, no type
    /// mismatches. A param renamed out from under the bank fails here.
    #[test]
    fn every_factory_preset_parses_without_warnings() {
        for (category, contents) in factory_files() {
            match read_preset(contents) {
                Ok((meta, _state, warnings)) => assert!(
                    warnings.is_empty(),
                    "factory preset `{category}/{}` warned: {warnings:?}",
                    meta.name
                ),
                Err(e) => panic!("factory preset in `{category}` failed to parse: {e:?}"),
            }
        }
    }

    /// Round-trip: re-writing a loaded preset and reading it back must land on
    /// the same state. Guards the sparse-TOML codec against a param whose
    /// default drifts away from what the file omits.
    #[test]
    fn every_factory_preset_round_trips() {
        use crate::preset::write_preset;
        for (category, contents) in factory_files() {
            let (meta, state, _) = read_preset(contents).expect("parses");
            let rewritten = write_preset(&meta, &state).expect("writes");
            let (_, back, warnings) = read_preset(&rewritten).expect("re-parses");
            assert!(warnings.is_empty(), "`{category}/{}` warned on re-read", meta.name);
            for layer in 0..2 {
                assert_eq!(
                    state.layers[layer].matrix, back.layers[layer].matrix,
                    "`{category}/{}` layer {layer} topology drifted",
                    meta.name
                );
                for i in 0..crate::params::ParamId::COUNT {
                    let Some(id) = crate::params::ParamId::from_index(i) else { continue };
                    let (a, b) = (
                        state.layers[layer].params.get(id),
                        back.layers[layer].params.get(id),
                    );
                    assert!(
                        (a - b).abs() < 1e-6,
                        "`{category}/{}` layer {layer} {} drifted {a} → {b}",
                        meta.name,
                        id.desc().name
                    );
                }
            }
            assert_eq!(state.key, back.key, "`{category}/{}` keyboard record drifted", meta.name);
        }
    }

    /// Every routed slot must point at a real source *and* a real dest. A slot
    /// half-wired (source set, dest `none`) is silently inert, which is the
    /// least obvious way for a factory preset to be broken.
    #[test]
    fn no_factory_preset_has_a_half_wired_slot() {
        for (category, contents) in factory_files() {
            let (meta, state, _) = read_preset(contents).expect("parses");
            for (layer, ls) in state.layers.iter().enumerate() {
                for (i, slot) in ls.matrix.slots.iter().enumerate() {
                    let src = slot.source != SourceId::None;
                    let dst = slot.dest != DestId::None;
                    assert_eq!(
                        src, dst,
                        "`{category}/{}` layer {layer} slot {i} is half-wired \
                         ({:?} → {:?})",
                        meta.name, slot.source, slot.dest
                    );
                }
            }
        }
    }

    /// Every preset needs a VCA: without a route into `Amp` the patch is either
    /// silent or stuck open. Cheap guard against a bank edit that clears slot 0.
    ///
    /// `is_active`, not just the dest: a preset persists switched-off routes on
    /// purpose, and the Amp scan skips them (0333), so a patch whose only Amp
    /// route is parked renders silent while a dest-only test says it is fine.
    #[test]
    fn every_factory_preset_drives_the_amp() {
        for (category, contents) in factory_files() {
            let (meta, state, _) = read_preset(contents).expect("parses");
            assert!(
                state.layers[0]
                    .matrix
                    .slots
                    .iter()
                    .any(|s| s.dest == DestId::Amp && s.is_active()),
                "`{category}/{}` layer 1 has no live route into Amp",
                meta.name
            );
        }
    }

    /// The two demos the ticket names, asserted structurally rather than by ear:
    /// the wheel-gated vibrato really is *scaled* by the wheel (not merely an
    /// LFO → pitch route), and the MPE patch really reads aftertouch.
    #[test]
    fn the_named_demos_route_what_they_claim() {
        let bank = factory();
        let by_name = |n: &str| {
            bank.iter()
                .find(|p| p.name == n)
                .unwrap_or_else(|| panic!("factory bank is missing `{n}`"))
        };

        let (_, wheel, _) = read_preset(by_name("Wheel Vibrato Lead").contents).unwrap();
        let vib = wheel.layers[0]
            .matrix
            .slots
            .iter()
            .find(|s| s.source == SourceId::Lfo1 && s.dest == DestId::Pitch)
            .expect("wheel demo routes LFO 1 → pitch");
        assert_eq!(
            vib.scale_src,
            SourceId::ModWheel,
            "the vibrato slot must be scaled by the mod wheel, or it is not gated"
        );
        assert_eq!(
            wheel.layers[0]
                .matrix
                .slots
                .iter()
                .filter(|s| s.dest == DestId::Pitch)
                .count(),
            1,
            "a second, ungated pitch route would defeat the gate"
        );

        let (_, mpe, _) = read_preset(by_name("Pressure Pad").contents).unwrap();
        assert!(
            mpe.layers[0]
                .matrix
                .slots
                .iter()
                .any(|s| s.source == SourceId::Aftertouch),
            "the MPE demo must route aftertouch"
        );
    }

    /// The split and dual demos must actually enable Layer 2 — a two-layer demo
    /// saved in Single mode is just a one-layer preset with dead state.
    #[test]
    fn the_two_layer_demos_enable_layer_2() {
        let bank = factory();
        for name in ["Split Bass and Lead", "Dual Locked Sweep"] {
            let p = bank.iter().find(|p| p.name == name).expect("demo present");
            let (_, st, _) = read_preset(p.contents).unwrap();
            assert!(st.key.layer2_on, "`{name}` must enable Layer 2");
        }
        let split = bank.iter().find(|p| p.name == "Split Bass and Lead").unwrap();
        let (_, st, _) = read_preset(split.contents).unwrap();
        assert!(st.key.split_enabled, "the split demo must be split");
    }
}
