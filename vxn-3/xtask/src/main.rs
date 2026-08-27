//! Build tasks for VXN3.
//!
//! Usage:
//!   cargo xtask bundle      # build + assemble target/bundled/vxn3.clap
//!   cargo xtask install     # bundle + copy to ~/Library/Audio/Plug-Ins/CLAP
//!   cargo xtask uninstall   # remove installed bundle
//!   cargo xtask --help
//!
//! macOS only, CLAP only. vxn-3 has no wrapper CMake project, so `Product::vst3`
//! is `None` and a `--format vst3` gets a reason rather than a stack of CMake
//! errors; Windows bundling is a follow-up.
//!
//! The bundle is just the dylib + Info.plist + PkgInfo — vxn-3's faceplate
//! assets are `include_str!`-embedded in the cdylib, so there is no
//! `Contents/Resources/` staging.
//!
//! The bundler itself is `vxn-xtask-common` (0317), shared with vxn-1b and
//! vxn-2. What is left here is the product descriptor and the argument shell.

use std::env;
use std::path::PathBuf;

use vxn_xtask_common::{Format, Product, Profile};

/// This product, for the shared bundler.
///
/// `vst3: None` is the load-bearing field: vxn-3 is CLAP-only, and saying so
/// here is what turns `--format vst3` into a sentence instead of a CMake
/// failure. `min_macos` stays 10.13.0 — vxn-1b declares 11.0.0 and unifying
/// them silently would change what a host believes about this plugin.
const PRODUCT: Product = Product {
    plugin_name: "vxn3",
    bundle_name: "vxn3.clap",
    bundle_id: "labs.vulpus.vxn3",
    display_name: "VXN3",
    lib_name: "vxn3_clap",
    clap_package: "vxn3-clap",
    version: env!("CARGO_PKG_VERSION"),
    min_macos: "10.13.0",
    vst3: None,
};

const PROFILE: Profile = Profile::Release;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("");

    let result = match cmd {
        "bundle" => bundle(false),
        "install" => bundle(true),
        "uninstall" => PRODUCT.uninstall(Format::Clap),
        "--help" | "-h" | "help" => {
            print_help();
            return;
        }
        "" => {
            print_help();
            std::process::exit(2);
        }
        other => {
            eprintln!("xtask: unknown subcommand `{other}`");
            print_help();
            std::process::exit(2);
        }
    };

    if let Err(e) = result {
        eprintln!("xtask: {e}");
        std::process::exit(1);
    }
}

fn print_help() {
    println!(
        "cargo xtask <subcommand>

Subcommands:
  bundle      Build {pkg} (release) and assemble target/bundled/{bundle}.
  install     Bundle, then copy to ~/Library/Audio/Plug-Ins/CLAP. macOS only.
  uninstall   Remove the installed bundle if present. macOS only.
  --help      Show this message.",
        pkg = PRODUCT.clap_package,
        bundle = PRODUCT.bundle_name,
    );
}

/// The workspace root. Two `.parent()` calls: `CARGO_MANIFEST_DIR` is
/// `.../vxn-3/xtask/` and the repo root is one flat workspace.
fn workspace_root() -> PathBuf {
    vxn_xtask_common::workspace_root(env!("CARGO_MANIFEST_DIR"))
}

/// Build the CLAP and stage it; optionally install it.
///
/// Always re-bundles rather than gating on mtime. Cargo's own freshness check
/// makes the build a no-op when nothing changed, and the copy after it is
/// cheap — an mtime gate here only ever fails to fire.
fn bundle(install: bool) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("bundle currently only supports macOS".into());
    }
    let out = PRODUCT.bundle_clap(&workspace_root(), PROFILE, false)?;
    println!("bundled → {}", out.display());
    if install {
        PRODUCT.install_artifact(Format::Clap, &out)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0317's per-product criterion: two `.parent()` calls must land on the
    /// directory holding the workspace manifest.
    #[test]
    fn workspace_root_holds_the_workspace_manifest() {
        let root = workspace_root();
        assert!(root.join("Cargo.toml").is_file(), "{} has no Cargo.toml", root.display());
        assert!(
            root.join("vxn-3/crates/vxn3-clap/Cargo.toml").is_file(),
            "{} is not the repo root",
            root.display()
        );
    }

    /// CLAP-only is a property of this product, and the error has to say so —
    /// there is no wrapper project for a VST3 build to find.
    #[test]
    fn there_is_no_vst3() {
        assert!(PRODUCT.vst3.is_none());
        let e = PRODUCT.install_path(Format::Vst3).unwrap_err();
        assert!(e.contains("CLAP-only"), "got: {e}");
    }

    #[test]
    fn the_plist_keeps_this_products_macos_floor() {
        let plist = PRODUCT.info_plist();
        assert!(plist.contains("<string>10.13.0</string>"), "not vxn-1b's 11.0.0");
        assert!(plist.contains("<key>CFBundleIdentifier</key><string>labs.vulpus.vxn3</string>"));
    }
}
