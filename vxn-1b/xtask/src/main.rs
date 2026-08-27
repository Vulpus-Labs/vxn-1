//! Build tasks for VXN1b.
//!
//! Usage:
//!   cargo xtask bundle [--universal] [--format clap,vst3]
//!   cargo xtask install [--universal] [--format clap,vst3]
//!   cargo xtask uninstall [--format clap,vst3]
//!   cargo xtask web [--serve] [--port N]
//!   cargo xtask --help
//!
//! `bundle` compiles the `vxn1b-clap` cdylib and wraps it into a `vxn1b.clap`
//! plugin, staged in `target/bundled/`. On macOS that is a bundle directory
//! (`Contents/MacOS/vxn1b` + `Info.plist` + `PkgInfo`); on Linux/Windows the
//! CLAP is just the shared library renamed to `.clap`. The faceplate assets are
//! `include_str!`-embedded in the cdylib, so nothing else goes in the bundle.
//!
//! `--universal` (macOS only) builds both `aarch64`/`x86_64` slices and `lipo`s
//! them into one fat binary, so a single bundle loads on Apple Silicon and Intel
//! hosts.
//!
//! `--format` selects which artifact(s) to produce (comma-separated, default
//! `clap`): `clap` runs the path above verbatim; `vst3` builds the `vxn1b-clap`
//! *staticlib*, whole-archives it into a clap-wrapper VST3 module via the
//! `vxn-1b/wrapper` CMake project (0213), and stages `VXN1b.vst3` next to the
//! CLAP. Both can be requested together. The `vst3` path needs CMake and the
//! repo-root `vendor/` submodules; on macOS it builds a universal bundle with
//! `--universal`, on Windows an x86_64 MSVC build (run from a Developer
//! PowerShell so `cl.exe` is on PATH).
//!
//! The build is **always release** — there is no `--debug`. A plugin is only
//! ever loaded into a host, where a debug build is not useful. A stray
//! `--release` is still tolerated (unknown flags are ignored; only unknown
//! subcommands are an error), but it is no longer documented or passed by CI —
//! it existed to make the workflow line scan like vxn-1's and vxn-2's, which is
//! not a reason for a flag to exist (0311).
//!
//! Ported from `vxn-1/xtask` by 0213. Two things differ and are deliberate:
//! the CLAP artifact is lowercase `vxn1b.clap` (the name already installed on
//! users' machines) while the VST3 is `VXN1b.vst3` (the display name); and
//! `touch_factory` below has no vxn-1 counterpart. Watch the two-`.parent()`
//! workspace-root quirk (the repo root is one flat workspace).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use vxn_xtask_common::{
    Format, Product, Profile, Vst3, arg_value, io, parse_formats, run_formats,
};

/// This product, for the shared bundler (0317).
///
/// The names are load-bearing and not derivable from one another: the CLAP
/// artifact is lowercase `vxn1b.clap` — the name already installed on users'
/// machines — while `Vst3::name` is `VXN1b`, a FILENAME rather than a display
/// name. Renaming either would orphan every installed copy and re-scan as a new
/// plugin. `display_name` is hyphenated to match the faceplate banner and is
/// NOT an identifier. `lib_name` is the `vxn1b-clap` package name with `-` →
/// `_` (cargo's crate-name rule), coupled to `clap_package` by hand; a rename
/// cannot *silently* ship an empty bundle, because every build path checks the
/// artifact exists at that name and errors with the path if it does not.
///
/// `version` is THIS crate's `CARGO_PKG_VERSION`, and has to be passed rather
/// than read inside the shared crate: VXN1b versions independently of the
/// workspace 0.x line, and the Info.plist stamps it.
const PRODUCT: Product = Product {
    plugin_name: "vxn1b",
    bundle_name: "vxn1b.clap",
    bundle_id: "labs.vulpus.vxn1b",
    display_name: "VXN-1b",
    lib_name: "vxn1b_clap",
    clap_package: "vxn1b-clap",
    version: env!("CARGO_PKG_VERSION"),
    resources_dir: None,
    min_macos: "11.0.0",
    vst3: Some(Vst3 {
        name: "VXN1b",
        wrapper_dir: "vxn-1b/wrapper",
        build_dir_stem: "vxn1b-wrapper-release",
    }),
};

/// The build is always release — see the module docs.
const PROFILE: Profile = Profile::Release;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let universal = args.iter().any(|a| a == "--universal");

    let formats = match parse_formats(&args) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("xtask: {e}");
            std::process::exit(2);
        }
    };

    let result = match cmd {
        "bundle" => run_formats(&formats, |fmt| bundle(fmt, universal, false)),
        "install" => run_formats(&formats, |fmt| bundle(fmt, universal, true)),
        "uninstall" => run_formats(&formats, |fmt| PRODUCT.uninstall(fmt)),
        "web" => {
            let serve = args.iter().any(|a| a == "--serve");
            let port = arg_value(&args, "--port");
            web(serve, port.as_deref())
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
        "cargo xtask <subcommand> [--universal] [--format clap,vst3]

Subcommands:
  bundle      Build {pkg} (release) and stage the artifact(s) in target/bundled/.
  install     Bundle, then copy to the user CLAP/VST3 directories.
  uninstall   Remove the installed artifact(s) if present.
  web         Build the browser bundle into target/web-dist-vxn1b/: both wasm modules,
              the transport JS + worklet, the generated faceplate page, and a
              COOP/COEP _headers. Pass --serve [--port N] for the dev server.
  --help      Show this message.

Flags:
  --universal  macOS only: build arm64 + x86_64 and lipo into one fat binary.
  --format     Comma-separated artifacts to produce (default: clap).
               `vst3` needs CMake and `git submodule update --init --recursive`.",
        pkg = PRODUCT.clap_package,
    );
}

/// The workspace root. Two `.parent()` calls: `CARGO_MANIFEST_DIR` is
/// `.../vxn-1b/xtask/` and the repo root is one flat workspace.
fn workspace_root() -> PathBuf {
    vxn_xtask_common::workspace_root(env!("CARGO_MANIFEST_DIR"))
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
    let path = workspace_root().join("vxn-1b/crates/vxn1b-engine/src/factory.rs");
    let src = fs::read(&path).map_err(io("read factory.rs"))?;
    fs::write(&path, src).map_err(io("touch factory.rs"))?;
    Ok(())
}

/// Build one format and stage it in `target/bundled/`; optionally install it.
///
/// `touch_factory` runs for BOTH legs: the CLAP and the VST3 wrap the same
/// engine, so a stale preset bank would ship in whichever one was asked for.
fn bundle(fmt: Format, universal: bool, install: bool) -> Result<(), String> {
    let root = workspace_root();
    touch_factory()?;
    let out = match fmt {
        Format::Clap => PRODUCT.bundle_clap(&root, PROFILE, universal)?,
        Format::Vst3 => PRODUCT.bundle_vst3(&root, PROFILE, universal)?,
    };
    println!("bundled → {}", out.display());
    if install {
        PRODUCT.install_artifact(fmt, &out)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The acceptance criterion of 0317: this product's root must be the
    /// directory holding the workspace `Cargo.toml`. Two `.parent()` calls are
    /// right for the flat layout and were wrong once
    /// ([[vxn2-xtask-flat-workspace]]), so each product asserts its own rather
    /// than trusting the shared helper.
    #[test]
    fn workspace_root_holds_the_workspace_manifest() {
        let root = workspace_root();
        assert!(
            root.join("Cargo.toml").is_file(),
            "{} has no Cargo.toml",
            root.display()
        );
        assert!(
            root.join("vxn-1b/crates/vxn1b-clap/Cargo.toml").is_file(),
            "{} is not the repo root — vxn-1b is not under it",
            root.display()
        );
    }

    /// `deploy.sh` passes no `--format`; that must stay a CLAP-only build.
    #[test]
    fn absent_format_defaults_to_clap() {
        let f = parse_formats(&["bundle".to_string()]).unwrap();
        assert_eq!(f, vec![Format::Clap]);
    }

    /// The bundle stamps VXN1b's OWN version, not the shared xtask crate's —
    /// this product rides a separate 0.0.x line.
    #[test]
    fn the_plist_stamps_this_crates_version_and_identity() {
        let plist = PRODUCT.info_plist();
        assert!(plist.contains(&format!("<string>{}</string>", env!("CARGO_PKG_VERSION"))));
        assert!(plist.contains(PRODUCT.bundle_id));
        assert!(plist.contains(&format!(
            "<key>CFBundleExecutable</key><string>{}</string>",
            PRODUCT.plugin_name
        )));
    }

    /// The two artifact names differ in case on purpose, and both are already
    /// installed on users' machines under those exact spellings.
    #[test]
    fn the_clap_and_vst3_names_are_not_derived_from_each_other() {
        assert_eq!(PRODUCT.bundle_name, "vxn1b.clap");
        assert_eq!(PRODUCT.vst3.expect("vxn-1b ships a VST3").name, "VXN1b");
    }
}

// ── Browser bundle (ticket 0292, epic E045) ─────────────────────────────────

/// Engine wasm: renders in the AudioWorklet.
const WASM_PKG: &str = "vxn1b-wasm";
const WASM_ARTIFACT: &str = "vxn1b_wasm.wasm";
/// Controller wasm: the main-thread model (ticket 0290).
const CONTROLLER_PKG: &str = "vxn1b-web-controller";
const CONTROLLER_ARTIFACT: &str = "vxn1b_web_controller.wasm";

/// The production browser modules, curated by hand so the `*.test.mjs` suites
/// never reach the bundle. Everything the page loads resolves within this list —
/// verified by [`web`] failing on a missing file rather than shipping a `dist/`
/// that 404s at runtime.
///
/// The SHARED modules live in `crates/vxn-core-web/assets` and are listed
/// separately ([`CORE_MODULES`]) because they come from a different source root;
/// `dist/` is flat, so both land side by side and the browser's `./x.mjs`
/// specifiers resolve either way.
const WEB_MODULES: [&str; 9] = [
    "event-ring.mjs",
    "event-codec.mjs",
    "param-store.mjs",
    "telemetry.mjs",
    "audio-host.mjs",
    "host-runner.mjs",
    "coordinator.mjs",
    "controller.mjs",
    "faceplate-bridge.mjs",
];

/// The AudioWorklet processor. Plain `.js`, not `.mjs`: `addModule()` loads it
/// as a classic script into the worklet scope.
const WEB_WORKLET: &str = "vxn1b-processor.js";

/// Shared browser modules, from `crates/vxn-core-web/assets` rather than this
/// port's `web/` (ticket 0284 — the "don't fork a third time" extraction).
///
/// Everything VXN1b actually imports: the two input adapters (0294) and the four
/// persistence modules (0293). Nothing speculative — a file no page loads is
/// dead weight on every visit, and the closure test below fails the moment a
/// reference outruns this list.
const CORE_MODULES: [&str; 8] = [
    "midi-input.mjs",
    "keyboard-input.mjs",
    // The on-screen piano (0322). VXN1b's faceplate has no playable keys, so in
    // a browser this is the only way to sound a note without MIDI hardware or
    // the QWERTY mapping.
    "piano-keyboard.mjs",
    "cpu-meter.mjs",
    "preset-storage.mjs",
    "preset-persistence.mjs",
    "state-autosave.mjs",
    "patch-io.mjs",
];

/// Netlify / Cloudflare-Pages `_headers`: COOP/COEP (+CORP) on every path, so a
/// static host serves the page cross-origin isolated and `SharedArrayBuffer` is
/// constructible. Without these the whole transport is unavailable and the page
/// cannot boot at all.
const WEB_DIST_HEADERS: &str = "/*\n  \
     Cross-Origin-Opener-Policy: same-origin\n  \
     Cross-Origin-Embedder-Policy: require-corp\n  \
     Cross-Origin-Resource-Policy: same-origin\n";

/// One command → a servable directory: both `.wasm` modules (release +
/// SIMD128), the transport JS + worklet, and the generated faceplate page.
///
/// No `factory.bin`: unlike vxn-1 and vxn-2, VXN1b's factory bank is embedded in
/// the controller wasm (`include_dir!`, ticket 0290) and its corpus is published
/// during `vxnc_new()`, so there is no asset to bake and no boot fetch to fail.
fn web(serve: bool, port: Option<&str>) -> Result<(), String> {
    let root = workspace_root();

    // 1. Both wasm crates, for wasm32-unknown-unknown.
    let engine = build_wasm(&root, WASM_PKG, WASM_ARTIFACT)?;
    let controller = build_wasm(&root, CONTROLLER_PKG, CONTROLLER_ARTIFACT)?;

    // 2. Assemble from scratch, so a removed module cannot linger in the bundle.
    let dist = root.join("target").join("web-dist-vxn1b");
    let _ = fs::remove_dir_all(&dist);
    fs::create_dir_all(&dist).map_err(|e| format!("create web-dist: {e}"))?;

    fs::copy(&engine, dist.join(WASM_ARTIFACT)).map_err(|e| format!("copy engine wasm: {e}"))?;
    fs::copy(&controller, dist.join(CONTROLLER_ARTIFACT))
        .map_err(|e| format!("copy controller wasm: {e}"))?;

    let web_src = root.join("vxn-1b/crates/vxn1b-wasm/web");
    for m in WEB_MODULES.iter().chain(std::iter::once(&WEB_WORKLET)) {
        let from = web_src.join(m);
        if !from.exists() {
            return Err(format!("missing web module {}", from.display()));
        }
        fs::copy(&from, dist.join(m)).map_err(|e| format!("copy web module {m}: {e}"))?;
    }
    let core_src = root.join("crates/vxn-core-web/assets");
    for m in CORE_MODULES {
        let from = core_src.join(m);
        if !from.exists() {
            return Err(format!("missing shared web module {}", from.display()));
        }
        fs::copy(&from, dist.join(m)).map_err(|e| format!("copy shared web module {m}: {e}"))?;
    }

    // 3. The faceplate page, GENERATED rather than copied: `gen-web-page` runs
    //    the same splice the plugin's editor does, so the param-descriptor JSON
    //    is byte-identical and xtask needs no wry dependency.
    let page = run_capture(
        &root,
        &["run", "--quiet", "-p", "vxn1b-ui-web", "--bin", "gen-web-page"],
        "gen-web-page",
    )?;
    fs::write(dist.join("index.html"), &page).map_err(|e| format!("write index.html: {e}"))?;

    fs::write(dist.join("_headers"), WEB_DIST_HEADERS)
        .map_err(|e| format!("write _headers: {e}"))?;

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

/// Build one wasm crate for `wasm32-unknown-unknown` (always release: a debug
/// wasm is too slow to render in real time) and return the artifact path.
fn build_wasm(root: &Path, package: &str, artifact: &str) -> Result<PathBuf, String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut build = Command::new(&cargo);
    build.current_dir(root).args([
        "build",
        "--package",
        package,
        "--target",
        "wasm32-unknown-unknown",
        "--release",
    ]);
    // SIMD128 is appended, never assigned: clobbering a caller's RUSTFLAGS would
    // silently drop whatever they set (a target-cpu, a lint level, a linker arg).
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
    let path = root
        .join("target")
        .join("wasm32-unknown-unknown")
        .join("release")
        .join(artifact);
    if !path.exists() {
        return Err(format!("expected wasm artifact at {}", path.display()));
    }
    Ok(path)
}

/// Run a cargo command and capture stdout — used for the generator bins whose
/// output IS the artifact.
fn run_capture(root: &Path, args: &[&str], what: &str) -> Result<Vec<u8>, String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(&cargo)
        .current_dir(root)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run {what}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{what} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out.stdout)
}

/// Serve the built bundle with COOP/COEP via `serve-coep.mjs`. Requires `node`.
fn serve_dist(root: &Path, dist: &Path, port: Option<&str>) -> Result<(), String> {
    let server = root.join("vxn-1b/crates/vxn1b-wasm/serve-coep.mjs");
    if !server.exists() {
        return Err(format!("serve-coep.mjs not found at {}", server.display()));
    }
    // Argument order is the script's: `serve-coep.mjs [port] [dir]`. Passing
    // them the other way round silently serves the wrong directory on port NaN.
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

#[cfg(test)]
mod web_tests {
    use super::*;

    /// Every module the bundle ships, plus the worklet.
    fn bundled() -> Vec<&'static str> {
        WEB_MODULES
            .iter()
            .copied()
            .chain([WEB_WORKLET])
            .chain(CORE_MODULES)
            .collect()
    }

    /// Where a bundled module comes from — the two source roots `dist/`
    /// flattens together.
    fn src_of(m: &str) -> PathBuf {
        let root = workspace_root();
        if CORE_MODULES.contains(&m) {
            root.join("crates/vxn-core-web/assets").join(m)
        } else {
            root.join("vxn-1b/crates/vxn1b-wasm/web").join(m)
        }
    }

    #[test]
    fn every_bundled_module_exists() {
        for m in bundled() {
            let p = src_of(m);
            assert!(p.exists(), "web module {m} is listed but missing at {}", p.display());
        }
    }

    /// The bundle must be closed under its own references: anything a shipped
    /// module reaches for by relative path has to be shipped too, or the page
    /// 404s at runtime in a browser and nowhere else. This is the check that
    /// catches a new `import` landing without a matching copy-list entry — the
    /// failure mode that is invisible until someone opens the page.
    #[test]
    fn the_bundle_is_closed_under_its_own_references() {
        let names: Vec<&str> = bundled();
        let mut found = 0usize;
        for m in &names {
            let src = fs::read_to_string(src_of(m))
                .unwrap_or_else(|e| panic!("read {m}: {e}"));
            for referenced in relative_refs(&src) {
                found += 1;
                assert!(
                    names.contains(&referenced.as_str()),
                    "{m} references \"./{referenced}\", which the bundle does not ship — \
                     add it to WEB_MODULES or drop the reference",
                );
            }
        }
        // Guard against passing vacuously: if `relative_refs` ever stops
        // matching (a quoting change, a switch to import maps), the loop above
        // would assert nothing at all and look green. The transport modules
        // import each other heavily, so the real count is well above this.
        assert!(
            found >= 6,
            "only {found} relative references found across {} modules — the scan is broken, \
             not the bundle",
            names.len(),
        );
    }

    /// Pull every `"./thing.mjs"` / `"./thing.js"` literal out of a module —
    /// static imports, dynamic `import()`, and the worklet's `addModule` URL all
    /// use the same spelling, so one scan covers them.
    fn relative_refs(src: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = src;
        while let Some(i) = rest.find("\"./") {
            rest = &rest[i + 3..];
            if let Some(end) = rest.find('"') {
                let name = &rest[..end];
                if name.ends_with(".mjs") || name.ends_with(".js") {
                    out.push(name.to_string());
                }
                rest = &rest[end..];
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// The bundle deliberately ships no test files: they would be dead weight on
    /// every page load and would pull `node:test` into a browser context.
    #[test]
    fn no_test_files_are_bundled() {
        for m in bundled() {
            assert!(!m.contains(".test."), "{m} is a test file and must not ship");
        }
    }

    /// vxn-1 and vxn-2 bake a `factory.bin`; VXN1b embeds its bank in the
    /// controller wasm (0290), so the bundle must not grow one back by copy-paste
    /// from either port's xtask.
    #[test]
    fn no_factory_asset_is_expected() {
        assert!(
            !bundled().iter().any(|m| m.contains("factory")),
            "VXN1b's factory bank is embedded in the controller wasm — no asset to bundle",
        );
    }

    /// The isolation headers are the whole reason the transport works: without
    /// both, `SharedArrayBuffer` is not constructible and the page cannot boot.
    #[test]
    fn the_headers_carry_both_isolation_directives() {
        assert!(WEB_DIST_HEADERS.contains("Cross-Origin-Opener-Policy: same-origin"));
        assert!(WEB_DIST_HEADERS.contains("Cross-Origin-Embedder-Policy: require-corp"));
        assert!(WEB_DIST_HEADERS.starts_with("/*\n"), "headers must apply to every path");
    }
}
