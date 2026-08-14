//! Build tasks for VXN1b.
//!
//! Usage:
//!   cargo xtask bundle      # build + assemble target/release/vxn1b.clap
//!   cargo xtask install     # bundle (if stale) + copy to ~/Library/Audio/Plug-Ins/CLAP
//!   cargo xtask uninstall   # remove installed bundle
//!   cargo xtask --help
//!
//! macOS only. Linux/Windows bundling (and the VST3 wrapper path) are follow-ups.
//!
//! Forked from `vxn-3/xtask` — the leanest of the family: vxn-1b's faceplate
//! assets will be `include_str!`-embedded in the cdylib, so the bundle is just
//! the dylib + Info.plist + PkgInfo. Watch the two-`.parent()` workspace-root
//! quirk (the repo root is one flat workspace).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const PLUGIN_NAME: &str = "vxn1b";
const BUNDLE_NAME: &str = "vxn1b.clap";
const BUNDLE_ID: &str = "labs.vulpus.vxn1b";
const DISPLAY_NAME: &str = "VXN1b";
const LIB_FILE: &str = "libvxn1b_clap.dylib";
const CLAP_PACKAGE: &str = "vxn1b-clap";

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("");

    let result = match cmd {
        "bundle" => bundle().map(|p| println!("bundled → {}", p.display())),
        "install" => install(),
        "uninstall" => uninstall(),
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
  bundle      Build {CLAP_PACKAGE} (release) and assemble target/release/{BUNDLE_NAME}.
  install     Bundle if stale, then copy to ~/Library/Audio/Plug-Ins/CLAP/{BUNDLE_NAME}.
  uninstall   Remove ~/Library/Audio/Plug-Ins/CLAP/{BUNDLE_NAME} if present.
  --help      Show this message."
    );
}

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is .../vxn-1b/xtask/. The flat workspace root sits two
    // levels up (the repo root is a single workspace).
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf()
}

fn release_dir() -> PathBuf {
    workspace_root().join("target").join("release")
}

fn bundle_path() -> PathBuf {
    release_dir().join(BUNDLE_NAME)
}

fn dylib_path() -> PathBuf {
    release_dir().join(LIB_FILE)
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

/// Force `factory.rs` to look modified so cargo rebuilds the embedded preset
/// bank (0212).
///
/// `include_dir!` bakes the TOML tree in at compile time but emits **no**
/// `rerun-if-changed` for it, so editing or adding a preset leaves the previous
/// bank in the rlib and the change silently doesn't ship. Rewriting the file's
/// own bytes bumps its mtime, which cargo *does* track. Costs one recompile of
/// one crate per bundle; the alternative is a bank that lies.
fn touch_factory() -> Result<(), String> {
    let path = workspace_root()
        .join("vxn-1b/crates/vxn1b-engine/src/factory.rs");
    let src = fs::read(&path).map_err(io("read factory.rs"))?;
    fs::write(&path, src).map_err(io("touch factory.rs"))?;
    Ok(())
}

fn bundle() -> Result<PathBuf, String> {
    if !cfg!(target_os = "macos") {
        return Err("bundle currently only supports macOS".into());
    }

    touch_factory()?;

    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(&cargo)
        .current_dir(workspace_root())
        .args(["build", "--release", "-p", CLAP_PACKAGE])
        .status()
        .map_err(|e| format!("failed to run cargo: {e}"))?;
    if !status.success() {
        return Err(format!("`cargo build --release -p {CLAP_PACKAGE}` failed"));
    }

    let dylib = dylib_path();
    if !dylib.exists() {
        return Err(format!(
            "expected dylib not found at {} (cross-compile target?)",
            dylib.display()
        ));
    }

    let bundle = bundle_path();
    let _ = fs::remove_dir_all(&bundle);
    let macos_dir = bundle.join("Contents").join("MacOS");
    fs::create_dir_all(&macos_dir).map_err(io("create Contents/MacOS"))?;
    fs::copy(&dylib, macos_dir.join(PLUGIN_NAME)).map_err(io("copy dylib into bundle"))?;
    fs::write(bundle.join("Contents").join("Info.plist"), info_plist())
        .map_err(io("write Info.plist"))?;
    fs::write(bundle.join("Contents").join("PkgInfo"), "BNDL????")
        .map_err(io("write PkgInfo"))?;

    Ok(bundle)
}

fn install() -> Result<(), String> {
    let dest = install_dest()?;
    let src = bundle_path();

    // Always re-bundle. Cargo's freshness check makes the build a no-op when
    // nothing changed; the bundle copy after it is cheap.
    bundle()?;

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
