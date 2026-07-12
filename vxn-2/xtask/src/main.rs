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
        "web" => {
            let serve = args.iter().any(|a| a == "--serve");
            let debug = args.iter().any(|a| a == "--debug");
            let port = args
                .iter()
                .position(|a| a == "--port")
                .and_then(|i| args.get(i + 1))
                .map(String::as_str);
            web(!debug, serve, port)
        }
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
  web         Build the browser bundle → target/web-dist/: both wasm modules
              (release + SIMD128 by default), the transport JS, the generated
              faceplate page, factory.bin, and a COOP/COEP _headers.
              Pass --debug for a debug wasm build.
              Pass --serve [--port N] to run the COOP/COEP dev server.
  --help      Show this message."
    );
}

// ── web bundle (ticket 0158) ────────────────────────────────────────────────

/// wasm crates + their `.wasm` artifact stems.
const WASM_PKG: &str = "vxn2-wasm";
const WASM_ARTIFACT: &str = "vxn2_wasm.wasm";
const CONTROLLER_PKG: &str = "vxn2-web-controller";
const CONTROLLER_ARTIFACT: &str = "vxn2_web_controller.wasm";

/// One command → a servable directory: both `.wasm` modules (release + SIMD128
/// by default), the transport JS + worklet, the generated faceplate page, the
/// baked factory bank, and a COOP/COEP `_headers`. `--serve` hands the bundle to
/// `serve-coep.mjs` with the headers `SharedArrayBuffer` needs.
fn web(release: bool, serve: bool, port: Option<&str>) -> Result<(), String> {
    let root = workspace_root();
    let profile = if release { "release" } else { "debug" };

    // 1. Compile BOTH wasm crates for wasm32-unknown-unknown: the engine (runs in
    //    the worklet) and the main-thread controller.
    let wasm = build_wasm(&root, WASM_PKG, WASM_ARTIFACT, release, profile)?;
    let controller_wasm = build_wasm(&root, CONTROLLER_PKG, CONTROLLER_ARTIFACT, release, profile)?;

    // 2. Assemble target/web-dist/ from scratch (a clean, portable copy).
    let dist = root.join("target").join("web-dist");
    let _ = fs::remove_dir_all(&dist);
    fs::create_dir_all(&dist).map_err(|e| format!("create web-dist: {e}"))?;

    // 2a. Both wasm modules.
    fs::copy(&wasm, dist.join(WASM_ARTIFACT)).map_err(|e| format!("copy engine wasm: {e}"))?;
    fs::copy(&controller_wasm, dist.join(CONTROLLER_ARTIFACT))
        .map_err(|e| format!("copy controller wasm: {e}"))?;

    // 2b. The production transport modules + worklet. Curated by hand: the
    //     *.test.mjs suites stay out of the shipped bundle. Preset / input
    //     modules land with 0159 / 0160.
    let web_src = root.join("vxn-2/crates/vxn2-wasm/web");
    const MODULES: [&str; 11] = [
        "event-ring.mjs",
        "event-codec.mjs",
        "param-store.mjs",
        "audio-host.mjs",
        "host-runner.mjs",
        "vxn2-processor.js",
        "coordinator.mjs",
        "controller.mjs",
        // Browser input adapters (0160): Web MIDI + computer keyboard → ring.
        "midi-input.mjs",
        "keyboard-input.mjs",
        "faceplate-bridge.mjs",
    ];
    for m in MODULES {
        let from = web_src.join(m);
        if !from.exists() {
            return Err(format!("missing web module {}", from.display()));
        }
        fs::copy(&from, dist.join(m)).map_err(|e| format!("copy web module {m}: {e}"))?;
    }

    // 2c. The faceplate page (generated by vxn2-ui-web's `gen-web-page` bin, so
    //     the JSON-shaping stays single-sourced and xtask carries no wry dep).
    let page = run_capture(
        &root,
        &["run", "--quiet", "-p", "vxn2-ui-web", "--bin", "gen-web-page"],
        "gen-web-page",
    )?;
    fs::write(dist.join("index.html"), &page).map_err(|e| format!("write index.html: {e}"))?;

    // 2c'. The baked factory bank (bake-factory bin → factory.bin). Consumed by
    //      the browser factory loader in 0159; baked now so the bundle is
    //      complete.
    let factory = run_capture(
        &root,
        &[
            "run", "--quiet", "--release", "-p", "vxn2-engine", "--bin", "bake-factory",
        ],
        "bake-factory",
    )?;
    fs::write(dist.join("factory.bin"), &factory).map_err(|e| format!("write factory.bin: {e}"))?;

    // 2d. A Netlify/Cloudflare-style `_headers` so dropping dist/ onto a static
    //     host carries the isolation headers SAB needs, no extra config.
    fs::write(dist.join("_headers"), WEB_DIST_HEADERS).map_err(|e| format!("write _headers: {e}"))?;

    println!("web bundle → {}", dist.display());

    if serve {
        return serve_dist(&root, &dist, port);
    }
    println!(
        "  note: SharedArrayBuffer needs cross-origin isolation — serve with \
         COOP/COEP (`cargo xtask web --serve`)"
    );
    Ok(())
}

/// Compile one wasm crate for `wasm32-unknown-unknown` (release + SIMD128 by
/// default) and return the path to its `.wasm` artifact.
fn build_wasm(
    root: &Path,
    package: &str,
    artifact: &str,
    release: bool,
    profile: &str,
) -> Result<PathBuf, String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut build = Command::new(&cargo);
    build
        .current_dir(root)
        .args(["build", "--package", package, "--target", "wasm32-unknown-unknown"]);
    if release {
        build.arg("--release");
    }
    // SIMD128: append so a caller's RUSTFLAGS isn't clobbered.
    let existing = env::var("RUSTFLAGS").unwrap_or_default();
    let rustflags = if existing.trim().is_empty() {
        "-C target-feature=+simd128".to_string()
    } else {
        format!("{existing} -C target-feature=+simd128")
    };
    build.env("RUSTFLAGS", rustflags);
    let status = build
        .status()
        .map_err(|e| format!("failed to run cargo for {package}: {e}"))?;
    if !status.success() {
        return Err(format!("wasm build failed for {package}"));
    }
    let wasm = root
        .join("target/wasm32-unknown-unknown")
        .join(profile)
        .join(artifact);
    if !wasm.exists() {
        return Err(format!("built wasm not found at {}", wasm.display()));
    }
    Ok(wasm)
}

/// Run a `cargo` subcommand and capture its stdout as bytes (used for the
/// gen-web-page + bake-factory subprocesses).
fn run_capture(root: &Path, args: &[&str], label: &str) -> Result<Vec<u8>, String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(&cargo)
        .current_dir(root)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run {label}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{label} failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(out.stdout)
}

/// Netlify/Cloudflare-Pages `_headers`: COOP/COEP (+CORP) on every path so the
/// served document is cross-origin isolated and SAB is constructible.
const WEB_DIST_HEADERS: &str = "/*\n  \
     Cross-Origin-Opener-Policy: same-origin\n  \
     Cross-Origin-Embedder-Policy: require-corp\n  \
     Cross-Origin-Resource-Policy: same-origin\n";

/// Serve the built bundle with COOP/COEP via `serve-coep.mjs`. Requires `node`.
/// Blocks until killed.
fn serve_dist(root: &Path, dist: &Path, port: Option<&str>) -> Result<(), String> {
    let server = root.join("vxn-2/crates/vxn2-wasm/serve-coep.mjs");
    if !server.exists() {
        return Err(format!("serve-coep.mjs not found at {}", server.display()));
    }
    let port = port.unwrap_or("8080");
    let status = Command::new("node")
        .current_dir(root)
        .arg(&server)
        .arg(port)
        .arg(dist)
        .status()
        .map_err(|e| format!("failed to run node (is it on PATH?): {e}"))?;
    if !status.success() {
        return Err("serve-coep.mjs exited with an error".into());
    }
    Ok(())
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
