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

use vxn_xtask_common::{
    Format, Product, Profile, Vst3, parse_formats, run_formats,
};

/// This product, for the shared bundler (0317).
///
/// `bundle_name` is capitalised (`VXN2.clap`) where vxn-1b's is not — both are
/// the names already installed on users' machines, and neither can be derived
/// from the other. `min_macos` stays 10.13.0: vxn-1b declares 11.0.0, and
/// unifying them here would change what a host believes about this plugin.
const PRODUCT: Product = Product {
    plugin_name: "vxn2",
    bundle_name: "VXN2.clap",
    bundle_id: "labs.vulpus.vxn2",
    display_name: "VXN2",
    lib_name: "vxn2_clap",
    clap_package: "vxn2-clap",
    version: env!("CARGO_PKG_VERSION"),
    resources_dir: Some("vxn-2/crates/vxn2-ui-web/assets"),
    min_macos: "10.13.0",
    vst3: Some(Vst3 {
        name: "VXN2",
        wrapper_dir: "vxn-2/wrapper",
        build_dir_stem: "vxn2-wrapper-release",
    }),
};

/// The workspace root. Two `.parent()` calls: `CARGO_MANIFEST_DIR` is
/// `.../vxn-2/xtask/` and the repo root is one flat workspace.
fn workspace_root() -> PathBuf {
    vxn_xtask_common::workspace_root(env!("CARGO_MANIFEST_DIR"))
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let release = args.iter().any(|a| a == "--release");
    let universal = args.iter().any(|a| a == "--universal");
    let do_install = args.iter().any(|a| a == "--install");

    let profile = if release { Profile::Release } else { Profile::Debug };
    let formats = match parse_formats(&args) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("xtask: {e}");
            std::process::exit(2);
        }
    };

    let result = match cmd {
        // `--format clap,vst3` (0170) selects which artifact(s) to act on;
        // absent → clap. 0317 gave `install` / `uninstall` the same dispatch
        // `bundle` already had, so all three take the flag.
        "bundle" => run_formats(&formats, |fmt| bundle(fmt, profile, universal, do_install)),
        "install" => run_formats(&formats, |fmt| {
            bundle(fmt, Profile::Release, universal, true)
        }),
        "uninstall" => run_formats(&formats, |fmt| PRODUCT.uninstall(fmt)),
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
  bundle      Build {pkg} and assemble target/bundled/{bundle}.
              macOS: a bundle dir (Contents/MacOS/ + Info.plist + Resources).
              Windows/Linux: the shared library renamed to {bundle}.
              Pass --release to build in release mode.
              Pass --universal (macOS only) to lipo arm64+x86_64 into one fat binary.
              Pass --format clap,vst3 (default clap) to also emit VXN2.vst3 via
              the vxn-2/wrapper clap-wrapper project (macOS + Windows only).
              Pass --install to copy each built artifact to its user plugin dir.
  install     Bundle (release) + copy to the user plugin directory. Takes
              --format like bundle, and works on every platform the format
              does — it was macOS + CLAP only before 0317.
  uninstall   Remove the installed artifact(s) if present. Takes --format.
  level-presets  Render every factory preset (held C-major triad over C4),
                 measure LUFS/peak, and rebalance each `master-volume`.
                 Dry run by default; pass `--apply` to rewrite the TOMLs.
                 Extra flags forwarded: --lufs <db> --headroom <db>.
  web         Build the browser bundle → target/web-dist/: both wasm modules
              (release + SIMD128 by default), the transport JS, the generated
              faceplate page, factory.bin, and a COOP/COEP _headers.
              Pass --debug for a debug wasm build.
              Pass --serve [--port N] to run the COOP/COEP dev server.
  --help      Show this message.",
        pkg = PRODUCT.clap_package,
        bundle = PRODUCT.bundle_name,
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
    //     *.test.mjs suites stay out of the shipped bundle. Input modules land
    //     with 0160.
    //     Six of them are SHARED with the other web ports (ticket 0284) and come
    //     from `crates/vxn-core-web/assets`, not this port's `web/`; `dist/` is
    //     flat, so both roots land side by side and the browser's `./x.mjs`
    //     specifiers resolve either way. `CORE_MODULES` is the shared list.
    let web_src = root.join("vxn-2/crates/vxn2-wasm/web");
    let core_src = root.join("crates/vxn-core-web/assets");
    const CORE_MODULES: [&str; 8] = [
        // Browser persistence: IndexedDB user presets, full-state autosave, and
        // patch export/import + share-link.
        "preset-storage.mjs",
        "preset-persistence.mjs",
        "state-autosave.mjs",
        "patch-io.mjs",
        // Browser input adapters: Web MIDI + computer keyboard → ring.
        "midi-input.mjs",
        "keyboard-input.mjs",
        // The on-screen piano (ticket 0322), hoisted out of faceplate-bridge.mjs
        // when VXN1b needed one. The bridge dynamic-imports it, so the bundle
        // MUST carry it or the keyboard silently fails to appear.
        "piano-keyboard.mjs",
        "cpu-meter.mjs",
    ];
    const MODULES: [&str; 9] = [
        "event-ring.mjs",
        "event-codec.mjs",
        "param-store.mjs",
        "audio-host.mjs",
        "host-runner.mjs",
        "vxn2-processor.js",
        "coordinator.mjs",
        "controller.mjs",
        "faceplate-bridge.mjs",
    ];
    for m in MODULES {
        let from = web_src.join(m);
        if !from.exists() {
            return Err(format!("missing web module {}", from.display()));
        }
        fs::copy(&from, dist.join(m)).map_err(|e| format!("copy web module {m}: {e}"))?;
    }
    for m in CORE_MODULES {
        let from = core_src.join(m);
        if !from.exists() {
            return Err(format!("missing shared web module {}", from.display()));
        }
        fs::copy(&from, dist.join(m)).map_err(|e| format!("copy shared web module {m}: {e}"))?;
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

/// Build one format and stage it in `target/bundled/`; optionally install it.
fn bundle(
    fmt: Format,
    profile: Profile,
    universal: bool,
    install: bool,
) -> Result<(), String> {
    let root = workspace_root();
    let out = match fmt {
        Format::Clap => PRODUCT.bundle_clap(&root, profile, universal)?,
        Format::Vst3 => PRODUCT.bundle_vst3(&root, profile, universal)?,
    };
    println!("bundled → {}", out.display());
    if install {
        PRODUCT.install_artifact(fmt, &out)?;
    }
    Ok(())
}

// ── VST3 build path (0170; ports vxn-1 E010) ────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// 0317's per-product criterion: two `.parent()` calls must land on the
    /// directory holding the workspace manifest ([[vxn2-xtask-flat-workspace]]
    /// is this exact mistake, made here).
    #[test]
    fn workspace_root_holds_the_workspace_manifest() {
        let root = workspace_root();
        assert!(root.join("Cargo.toml").is_file(), "{} has no Cargo.toml", root.display());
        assert!(
            root.join("vxn-2/crates/vxn2-clap/Cargo.toml").is_file(),
            "{} is not the repo root",
            root.display()
        );
    }

    /// The CLAP bundle is capitalised here and lowercase in vxn-1b. Both are
    /// the names already on users' machines; neither is derivable.
    #[test]
    fn the_bundle_names_are_this_products_own() {
        assert_eq!(PRODUCT.bundle_name, "VXN2.clap");
        assert_eq!(PRODUCT.vst3.expect("vxn-2 ships a VST3").name, "VXN2");
    }

    /// vxn-2 declares an older floor than vxn-1b, and the shared bundler must
    /// not have quietly raised it.
    #[test]
    fn the_plist_keeps_this_products_macos_floor() {
        let plist = PRODUCT.info_plist();
        assert!(plist.contains("<string>10.13.0</string>"), "not vxn-1b's 11.0.0");
        assert!(plist.contains("<key>CFBundleIdentifier</key><string>labs.vulpus.vxn2</string>"));
    }

    /// vxn-2 keeps a real debug path — 0311 removed vxn-1b's no-op `--release`,
    /// but here the flag selects a profile that is actually built.
    #[test]
    fn both_profiles_name_a_cargo_directory() {
        assert_eq!(Profile::Release.dir(), "release");
        assert_eq!(Profile::Debug.dir(), "debug");
    }
}
