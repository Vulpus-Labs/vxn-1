//! Build tasks for VXN1.
//!
//! Usage:
//!   cargo xtask bundle [--release] [--install] [--universal]
//!   cargo xtask web [--debug]
//!
//! `bundle` compiles the `vxn-clap` cdylib and wraps it into a `VXN1.clap`
//! plugin. On macOS that is a bundle directory (`Contents/MacOS/VXN1` +
//! `Info.plist`); on Linux/Windows the CLAP is just the shared library renamed
//! to `.clap`. `--install` copies it to the user CLAP directory. `--universal`
//! (macOS only) builds both `aarch64`/`x86_64` slices and `lipo`s them into a
//! single fat binary, so one bundle loads on Apple Silicon and Intel hosts.
//!
//! `web` (ticket 0041) compiles the wasm crate(s) for `wasm32-unknown-unknown`
//! (release + SIMD128 by default) and assembles a self-contained, servable
//! `target/web-dist/`: the `.wasm`, the E015 JS transport modules, the worklet,
//! and the page assets. `--debug` builds a debug wasm. The COOP/COEP dev server
//! that serves it is ticket 0045.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const PLUGIN_NAME: &str = "VXN1";
const BUNDLE_ID: &str = "labs.vulpus.vxn1";
const LIB_NAME: &str = "vxn_clap";

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let release = args.iter().any(|a| a == "--release");
    let install = args.iter().any(|a| a == "--install");
    let universal = args.iter().any(|a| a == "--universal");

    match cmd {
        "bundle" => {
            if let Err(e) = bundle(release, install, universal) {
                eprintln!("xtask: {e}");
                std::process::exit(1);
            }
        }
        "web" => {
            // `web` defaults to release (a real deploy ships release+SIMD);
            // `--debug` opts into a debug wasm.
            let debug = args.iter().any(|a| a == "--debug");
            if let Err(e) = web(!debug) {
                eprintln!("xtask: {e}");
                std::process::exit(1);
            }
        }
        _ => {
            eprintln!(
                "usage:\n  cargo xtask bundle [--release] [--install] [--universal]\n  cargo xtask web [--debug]"
            );
            std::process::exit(2);
        }
    }
}

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is .../vxn-1/xtask/. The flat workspace root sits two
    // levels up (E001 promoted the repo root to a single workspace).
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf()
}

fn bundle(release: bool, install: bool, universal: bool) -> Result<(), String> {
    let root = workspace_root();
    let profile = if release { "release" } else { "debug" };

    // 1. Compile the cdylib (a single fat dylib for a macOS universal build,
    //    otherwise the host-target shared library).
    let lib = if universal {
        if !cfg!(target_os = "macos") {
            return Err("--universal is macOS-only".into());
        }
        build_universal(&root, release)?
    } else {
        let mut build = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
        build
            .current_dir(&root)
            .args(["build", "--package", "vxn-clap"]);
        if release {
            build.arg("--release");
        }
        let status = build
            .status()
            .map_err(|e| format!("failed to run cargo: {e}"))?;
        if !status.success() {
            return Err("cargo build failed".into());
        }
        let lib = lib_path(&root.join("target").join(profile));
        if !lib.exists() {
            return Err(format!("built library not found at {}", lib.display()));
        }
        lib
    };

    // 2. Assemble the .clap bundle.
    let out_dir = root.join("target").join("bundled");
    fs::create_dir_all(&out_dir).map_err(io("create bundled dir"))?;
    let clap_path = out_dir.join(format!("{PLUGIN_NAME}.clap"));

    if cfg!(target_os = "macos") {
        build_macos_bundle(&clap_path, &lib)?;
    } else {
        // Linux/Windows: a CLAP is just the shared library with a .clap name.
        let _ = fs::remove_file(&clap_path);
        fs::copy(&lib, &clap_path).map_err(io("copy library"))?;
    }
    println!("bundled → {}", clap_path.display());

    // 3. Optionally install.
    if install {
        let dest_dir = install_dir()?;
        fs::create_dir_all(&dest_dir).map_err(io("create install dir"))?;
        let dest = dest_dir.join(format!("{PLUGIN_NAME}.clap"));
        copy_clap(&clap_path, &dest)?;
        println!("installed → {}", dest.display());
    }
    Ok(())
}

const WASM_PKG: &str = "vxn-wasm";
const WASM_ARTIFACT: &str = "vxn_wasm.wasm";

/// Build the wasm and assemble a self-contained `target/web-dist/` (ticket 0041).
///
/// One command → a servable directory: the engine `.wasm` (release + SIMD128 by
/// default), the E015 transport JS modules, the production worklet, and a page.
/// The COOP/COEP server that serves it is ticket 0045; the AudioContext boot
/// that drives it is 0042.
fn web(release: bool) -> Result<(), String> {
    let root = workspace_root();
    let profile = if release { "release" } else { "debug" };

    // 1. Compile the engine wasm crate for wasm32-unknown-unknown.
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut build = Command::new(&cargo);
    build.current_dir(&root).args([
        "build",
        "--package",
        WASM_PKG,
        "--target",
        "wasm32-unknown-unknown",
    ]);
    if release {
        build.arg("--release");
    }
    // SIMD128: perf measurement is E020, but the flag belongs in the pipeline.
    // Append so we don't clobber a caller's RUSTFLAGS.
    let existing = env::var("RUSTFLAGS").unwrap_or_default();
    let rustflags = if existing.trim().is_empty() {
        "-C target-feature=+simd128".to_string()
    } else {
        format!("{existing} -C target-feature=+simd128")
    };
    build.env("RUSTFLAGS", rustflags);
    let status = build
        .status()
        .map_err(|e| format!("failed to run cargo: {e}"))?;
    if !status.success() {
        return Err("wasm build failed".into());
    }

    let wasm = root
        .join("target/wasm32-unknown-unknown")
        .join(profile)
        .join(WASM_ARTIFACT);
    if !wasm.exists() {
        return Err(format!("built wasm not found at {}", wasm.display()));
    }

    // 2. Assemble target/web-dist/ from scratch (a clean, portable copy).
    let dist = root.join("target").join("web-dist");
    let _ = fs::remove_dir_all(&dist);
    fs::create_dir_all(&dist).map_err(io("create web-dist"))?;

    // 2a. The engine wasm.
    fs::copy(&wasm, dist.join(WASM_ARTIFACT)).map_err(io("copy wasm"))?;

    // 2b. The E015 production transport modules + worklet. Curated by hand: the
    //     *.test.mjs suites, the Node harnesses, and the 0034/0035 spike
    //     processors stay out of the shipped bundle. The production worklet
    //     (`vxn-processor-0038.js`, runner-based) takes dist's stable name.
    let web_src = root.join("vxn-1/crates/vxn-wasm/web");
    const MODULES: [(&str, &str); 6] = [
        ("event-ring.mjs", "event-ring.mjs"),
        ("event-codec.mjs", "event-codec.mjs"),
        ("param-store.mjs", "param-store.mjs"),
        ("audio-host.mjs", "audio-host.mjs"),
        ("host-runner.mjs", "host-runner.mjs"),
        ("vxn-processor-0038.js", "vxn-processor.js"),
    ];
    for (src, dest) in MODULES {
        let from = web_src.join(src);
        if !from.exists() {
            return Err(format!("missing web module {}", from.display()));
        }
        fs::copy(&from, dist.join(dest)).map_err(io("copy web module"))?;
    }

    // 2c. A generated page. The real AudioContext/coordinator boot is ticket
    //     0042; until then this documents the bundle + the isolation
    //     requirement so a stray open() doesn't look broken.
    fs::write(dist.join("index.html"), web_index_html()).map_err(io("write index.html"))?;

    println!("web bundle → {}", dist.display());
    println!(
        "  note: SharedArrayBuffer needs cross-origin isolation — serve with \
         COOP/COEP (ticket 0045)"
    );
    Ok(())
}

fn web_index_html() -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>VXN1 web (v{version})</title>
    <style>
      body {{ font: 16px system-ui; max-width: 40rem; margin: 4rem auto; padding: 0 1rem; }}
      code {{ background: #f0f0f0; padding: 0.1rem 0.3rem; border-radius: 3px; }}
      #iso {{ font-weight: 600; }}
    </style>
  </head>
  <body>
    <h1>VXN1 → WASM</h1>
    <p>Build bundle from <code>cargo xtask web</code> (ticket 0041). The
      AudioContext boot (ticket 0042) and faceplate (E018) land on top of these
      assets.</p>
    <p>cross-origin isolated: <span id="iso">checking…</span></p>
    <script type="module">
      const el = document.getElementById("iso");
      el.textContent = self.crossOriginIsolated ? "yes — SharedArrayBuffer available"
        : "no — serve with COOP/COEP (ticket 0045)";
      el.style.color = self.crossOriginIsolated ? "green" : "crimson";
    </script>
  </body>
</html>
"#,
        version = env!("CARGO_PKG_VERSION"),
    )
}

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
            .args(["build", "--package", "vxn-clap", "--target", triple]);
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

fn build_macos_bundle(clap_path: &Path, lib: &Path) -> Result<(), String> {
    let _ = fs::remove_dir_all(clap_path);
    let macos_dir = clap_path.join("Contents").join("MacOS");
    fs::create_dir_all(&macos_dir).map_err(io("create Contents/MacOS"))?;
    fs::copy(lib, macos_dir.join(PLUGIN_NAME)).map_err(io("copy library into bundle"))?;
    fs::write(clap_path.join("Contents").join("Info.plist"), info_plist())
        .map_err(io("write Info.plist"))?;
    fs::write(clap_path.join("Contents").join("PkgInfo"), "BNDL????")
        .map_err(io("write PkgInfo"))?;
    Ok(())
}

fn copy_clap(src: &Path, dest: &Path) -> Result<(), String> {
    if src.is_dir() {
        let _ = fs::remove_dir_all(dest);
        copy_dir_recursive(src, dest)
    } else {
        let _ = fs::remove_file(dest);
        fs::copy(src, dest).map(|_| ()).map_err(io("install copy"))
    }
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

fn install_dir() -> Result<PathBuf, String> {
    if cfg!(target_os = "macos") {
        let home = env::var("HOME").map_err(|_| "HOME not set".to_string())?;
        Ok(PathBuf::from(home).join("Library/Audio/Plug-Ins/CLAP"))
    } else if cfg!(target_os = "windows") {
        let local = env::var("LOCALAPPDATA").map_err(|_| "LOCALAPPDATA not set".to_string())?;
        Ok(PathBuf::from(local).join("Programs/Common/CLAP"))
    } else {
        let home = env::var("HOME").map_err(|_| "HOME not set".to_string())?;
        Ok(PathBuf::from(home).join(".clap"))
    }
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
    <key>CFBundleName</key><string>{PLUGIN_NAME}</string>
    <key>CFBundlePackageType</key><string>BNDL</string>
    <key>CFBundleVersion</key><string>{version}</string>
    <key>CFBundleShortVersionString</key><string>{version}</string>
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
