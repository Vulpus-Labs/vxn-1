//! Build tasks for VXN2.
//!
//! Usage:
//!   cargo xtask bundle      # build + assemble target/release/vxn2.clap
//!   cargo xtask install     # bundle (if stale) + copy to ~/Library/Audio/Plug-Ins/CLAP
//!   cargo xtask uninstall   # remove installed bundle
//!   cargo xtask --help
//!
//! macOS only. Linux/Windows bundling is a follow-up.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const PLUGIN_NAME: &str = "vxn2";
const BUNDLE_NAME: &str = "vxn2.clap";
const BUNDLE_ID: &str = "labs.vulpus.vxn2";
const DISPLAY_NAME: &str = "VXN2";
const LIB_FILE: &str = "libvxn2_clap.dylib";
const CLAP_PACKAGE: &str = "vxn2-clap";

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
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
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

fn bundle() -> Result<PathBuf, String> {
    if !cfg!(target_os = "macos") {
        return Err("bundle currently only supports macOS".into());
    }

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

    // Stage the HTML faceplate assets into Contents/Resources/ so a
    // developer can iterate on CSS / JS without rebuilding the cdylib:
    // with VXN2_DEV_ASSETS=1 set in the host's environment, the editor
    // reads from the bundle path instead of its `include_str!` embed.
    // Production users never set the env var and run from the embed.
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

    Ok(bundle)
}

fn install() -> Result<(), String> {
    let dest = install_dest()?;
    let src = bundle_path();

    let needs_build = match (mtime(&src), mtime(&dylib_path())) {
        (None, _) => true,
        (Some(_), None) => true,
        (Some(b), Some(d)) => b < d,
    };
    if needs_build {
        bundle()?;
    }

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

fn mtime(path: &Path) -> Option<std::time::SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
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
