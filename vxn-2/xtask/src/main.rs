//! Build tasks for VXN2.
//!
//! Usage:
//!   cargo xtask bundle [--release]    # build + assemble target/bundled/VXN2.clap
//!   cargo xtask install               # bundle (release) + copy to user CLAP dir (macOS)
//!   cargo xtask uninstall             # remove installed bundle (macOS)
//!   cargo xtask level-presets [--apply] [--lufs <db>] [--headroom <db>]
//!   cargo xtask --help
//!
//! `bundle` builds the `vxn2-clap` cdylib and assembles `target/bundled/VXN2.clap`.
//! On macOS: a bundle directory (Contents/MacOS/ + Info.plist + PkgInfo + Resources).
//! On Windows/Linux: the shared library renamed to `VXN2.clap`.
//! Dev-asset staging (Contents/Resources) is macOS-only; Windows/Linux builds read
//! from the `include_str!` embed — hot-reload on those platforms is a follow-up.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const PLUGIN_NAME: &str = "vxn2";
const BUNDLE_NAME: &str = "VXN2.clap";
const BUNDLE_ID: &str = "labs.vulpus.vxn2";
const DISPLAY_NAME: &str = "VXN2";
/// Cargo lib stem: the `vxn2-clap` package name with `-` → `_`.
const LIB_NAME: &str = "vxn2_clap";
const CLAP_PACKAGE: &str = "vxn2-clap";

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let release = args.iter().any(|a| a == "--release");
    let universal = args.iter().any(|a| a == "--universal");

    let result = match cmd {
        "bundle" => bundle(release, universal).map(|p| println!("bundled → {}", p.display())),
        "install" => install(),
        "uninstall" => uninstall(),
        "level-presets" => level_presets(&args[1..]),
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
  bundle      Build {CLAP_PACKAGE} and assemble target/bundled/{BUNDLE_NAME}.
              macOS: a bundle dir (Contents/MacOS/ + Info.plist + Resources).
              Windows/Linux: the shared library renamed to {BUNDLE_NAME}.
              Pass --release to build in release mode.
              Pass --universal (macOS only) to lipo arm64+x86_64 into one fat binary.
  install     Bundle (release) + copy to user CLAP directory. macOS only.
  uninstall   Remove ~/Library/Audio/Plug-Ins/CLAP/{BUNDLE_NAME}. macOS only.
  level-presets  Render every factory preset (held C-major triad over C4),
                 measure LUFS/peak, and rebalance each `master-volume`.
                 Dry run by default; pass `--apply` to rewrite the TOMLs.
                 Extra flags forwarded: --lufs <db> --headroom <db>.
  --help      Show this message."
    );
}

fn level_presets(rest: &[String]) -> Result<(), String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut cmd = Command::new(&cargo);
    cmd.current_dir(workspace_root()).args([
        "run",
        "--release",
        "-p",
        "vxn2-engine",
        "--example",
        "level_presets",
        "--",
    ]);
    cmd.args(rest);
    let status = cmd
        .status()
        .map_err(|e| format!("failed to launch cargo: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("level-presets failed".into())
    }
}

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is .../vxn-1/vxn-2/xtask/. The flat workspace
    // root sits two levels up (E001 promoted the repo root to a single
    // workspace). The target/ dir + asset paths key off this.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf()
}

fn bundled_dir() -> PathBuf {
    workspace_root().join("target").join("bundled")
}

fn bundle_path() -> PathBuf {
    bundled_dir().join(BUNDLE_NAME)
}

/// Path to the vxn2-clap shared library under a profile dir.
/// Mirrors the helper in vxn-1/xtask so the cross-platform lib naming is
/// handled in one place rather than scattered through bundle().
fn lib_path(profile_dir: &Path) -> PathBuf {
    let (prefix, ext) = if cfg!(target_os = "windows") {
        ("", "dll")
    } else if cfg!(target_os = "macos") {
        ("lib", "dylib")
    } else {
        ("lib", "so")
    };
    profile_dir.join(format!("{prefix}{LIB_NAME}.{ext}"))
}

fn install_dest() -> Result<PathBuf, String> {
    if !cfg!(target_os = "macos") {
        return Err("install/uninstall only supported on macOS".into());
    }
    let home = env::var("HOME").map_err(|_| "HOME not set".to_string())?;
    Ok(PathBuf::from(home)
        .join("Library/Audio/Plug-Ins/CLAP")
        .join(BUNDLE_NAME))
}

fn bundle(release: bool, universal: bool) -> Result<PathBuf, String> {
    if universal && !cfg!(target_os = "macos") {
        return Err("--universal is macOS-only".into());
    }
    let profile = if release { "release" } else { "debug" };
    let root = workspace_root();

    let lib = if universal {
        build_universal(&root, release)?
    } else {
        let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let mut build = Command::new(&cargo);
        build
            .current_dir(&root)
            .args(["build", "-p", CLAP_PACKAGE]);
        if release {
            build.arg("--release");
        }
        let status = build
            .status()
            .map_err(|e| format!("failed to run cargo: {e}"))?;
        if !status.success() {
            return Err(format!("`cargo build -p {CLAP_PACKAGE}` failed"));
        }
        let l = lib_path(&root.join("target").join(profile));
        if !l.exists() {
            return Err(format!(
                "expected library not found at {} (cross-compile target?)",
                l.display()
            ));
        }
        l
    };

    let bundle = bundle_path();
    fs::create_dir_all(&bundled_dir()).map_err(io("create bundled dir"))?;

    if cfg!(target_os = "macos") {
        let _ = fs::remove_dir_all(&bundle);
        let macos_dir = bundle.join("Contents").join("MacOS");
        fs::create_dir_all(&macos_dir).map_err(io("create Contents/MacOS"))?;
        fs::copy(&lib, macos_dir.join(PLUGIN_NAME)).map_err(io("copy dylib into bundle"))?;
        fs::write(bundle.join("Contents").join("Info.plist"), info_plist())
            .map_err(io("write Info.plist"))?;
        fs::write(bundle.join("Contents").join("PkgInfo"), "BNDL????")
            .map_err(io("write PkgInfo"))?;

        // Stage the HTML faceplate assets into Contents/Resources/ so a
        // developer can iterate on CSS/JS without rebuilding the cdylib:
        // with VXN2_DEV_ASSETS=1 set in the host's environment, the editor
        // reads from the bundle path instead of its `include_str!` embed.
        // Production users never set the env var and run from the embed.
        // Windows/Linux dev hot-reload is a follow-up (E013 out-of-scope).
        let assets_src = workspace_root()
            .join("vxn-2")
            .join("crates")
            .join("vxn2-ui-web")
            .join("assets");
        if !assets_src.is_dir() {
            return Err(format!(
                "expected ui-web assets at {}, but the directory is missing",
                assets_src.display()
            ));
        }
        let resources_dir = bundle.join("Contents").join("Resources");
        copy_dir_recursive(&assets_src, &resources_dir)?;
    } else {
        // Windows/Linux: a CLAP is just the shared library with a .clap name.
        let _ = fs::remove_file(&bundle);
        fs::copy(&lib, &bundle).map_err(io("copy library"))?;
    }

    Ok(bundle)
}

fn install() -> Result<(), String> {
    let dest = install_dest()?;
    let src = bundle_path();

    // Always re-bundle in release mode. Cargo's own freshness check makes the
    // build a no-op when nothing has changed; the bundle copy after it is cheap.
    bundle(true, false)?;

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(io("create install parent"))?;
    }
    let _ = fs::remove_dir_all(&dest);
    copy_dir_recursive(&src, &dest)?;
    println!("installed → {}", dest.display());
    Ok(())
}

fn uninstall() -> Result<(), String> {
    let dest = install_dest()?;
    if dest.exists() {
        fs::remove_dir_all(&dest).map_err(io("remove install"))?;
        println!("uninstalled → {}", dest.display());
    } else {
        println!("nothing to uninstall at {}", dest.display());
    }
    Ok(())
}

/// Build both macOS slices and `lipo` them into one fat dylib; returns its path.
/// Each target's dylib lands under `target/<triple>/<profile>/`; the combined
/// binary is written to `target/universal/<profile>/`.
fn build_universal(root: &Path, release: bool) -> Result<PathBuf, String> {
    const TRIPLES: [&str; 2] = ["aarch64-apple-darwin", "x86_64-apple-darwin"];
    let profile = if release { "release" } else { "debug" };
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());

    let mut slices = Vec::new();
    for triple in TRIPLES {
        let mut build = Command::new(&cargo);
        build
            .current_dir(root)
            .args(["build", "-p", CLAP_PACKAGE, "--target", triple]);
        if release {
            build.arg("--release");
        }
        let status = build
            .status()
            .map_err(|e| format!("failed to run cargo for {triple}: {e}"))?;
        if !status.success() {
            return Err(format!("cargo build failed for {triple}"));
        }
        let lib = lib_path(&root.join("target").join(triple).join(profile));
        if !lib.exists() {
            return Err(format!("{triple} library not found at {}", lib.display()));
        }
        slices.push(lib);
    }

    let out_dir = root.join("target").join("universal").join(profile);
    fs::create_dir_all(&out_dir).map_err(io("create universal dir"))?;
    let out = out_dir.join(format!("lib{LIB_NAME}.dylib"));
    let status = Command::new("lipo")
        .arg("-create")
        .args(&slices)
        .arg("-output")
        .arg(&out)
        .status()
        .map_err(|e| format!("failed to run lipo: {e}"))?;
    if !status.success() {
        return Err("lipo failed".into());
    }
    Ok(out)
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(io("mkdir"))?;
    for entry in fs::read_dir(src).map_err(io("read_dir"))? {
        let entry = entry.map_err(io("dir entry"))?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            fs::copy(&from, &to).map_err(io("copy file"))?;
        }
    }
    Ok(())
}

fn info_plist() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key><string>English</string>
    <key>CFBundleExecutable</key><string>{PLUGIN_NAME}</string>
    <key>CFBundleIdentifier</key><string>{BUNDLE_ID}</string>
    <key>CFBundleName</key><string>{DISPLAY_NAME}</string>
    <key>CFBundlePackageType</key><string>BNDL</string>
    <key>CFBundleVersion</key><string>{version}</string>
    <key>CFBundleShortVersionString</key><string>{version}</string>
    <key>CFBundleSupportedPlatforms</key>
    <array><string>MacOSX</string></array>
    <key>LSMinimumSystemVersion</key><string>10.13.0</string>
</dict>
</plist>
"#,
        version = env!("CARGO_PKG_VERSION"),
    )
}

fn io(ctx: &'static str) -> impl Fn(std::io::Error) -> String {
    move |e| format!("{ctx}: {e}")
}
