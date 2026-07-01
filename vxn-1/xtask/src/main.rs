//! Build tasks for VXN1.
//!
//! Usage:
//!   cargo xtask bundle [--release] [--install] [--universal] [--format clap,vst3]
//!   cargo xtask web [--debug] [--serve] [--port N]
//!
//! `bundle` compiles the `vxn-clap` cdylib and wraps it into a `VXN1.clap`
//! plugin. On macOS that is a bundle directory (`Contents/MacOS/VXN1` +
//! `Info.plist`); on Linux/Windows the CLAP is just the shared library renamed
//! to `.clap`. `--install` copies it to the user CLAP directory. `--universal`
//! (macOS only) builds both `aarch64`/`x86_64` slices and `lipo`s them into a
//! single fat binary, so one bundle loads on Apple Silicon and Intel hosts.
//!
//! `--format` selects which artifact(s) to produce (comma-separated, default
//! `clap`): `clap` runs the path above verbatim; `vst3` (E010 / tickets
//! 0011+0012) builds the `vxn-clap` *staticlib*, whole-archives it into a
//! clap-wrapper VST3 module via the `vxn-1/wrapper` CMake project, and stages
//! `VXN1.vst3` next to the CLAP. Both can be requested together. The `vst3` path
//! needs CMake and the `vendor/` submodules; on macOS it builds a universal
//! bundle (`--universal`), on Windows an x86_64 MSVC build (run from a Developer
//! PowerShell so `cl.exe` is on PATH).
//!
//! `web` (ticket 0041) compiles the wasm crate(s) for `wasm32-unknown-unknown`
//! (release + SIMD128 by default) and assembles a self-contained, servable
//! `target/web-dist/`: the `.wasm`, the E015 JS transport modules, the worklet,
//! and the page assets. `--debug` builds a debug wasm. `--serve` (ticket 0045)
//! then serves the bundle with the COOP/COEP cross-origin-isolation headers
//! `SharedArrayBuffer` requires, via `serve-coep.mjs` (default port 8080,
//! `--port N` overrides). Production hosting is documented in the crate's
//! `WEB-HOSTING.md`; the bundle also drops a Netlify `_headers` file so a
//! static-host deploy carries the same two headers.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const PLUGIN_NAME: &str = "VXN1";
const BUNDLE_ID: &str = "labs.vulpus.vxn1";
/// The cdylib's file stem: the `vxn-clap` package name with `-` → `_` (cargo's
/// crate-name rule). Coupled to the `--package vxn-clap` build below by hand
/// (0019); deriving it from the manifest at runtime isn't worth the parse for a
/// build tool. A rename can't *silently* ship an empty bundle: `build_universal`
/// checks the produced dylib exists at this name and errors with the path if it
/// doesn't — update both this constant and the `--package` arg together.
const LIB_NAME: &str = "vxn_clap";

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let release = args.iter().any(|a| a == "--release");
    let install = args.iter().any(|a| a == "--install");
    let universal = args.iter().any(|a| a == "--universal");

    match cmd {
        "bundle" => {
            let formats = match parse_formats(&args) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("xtask: {e}");
                    std::process::exit(2);
                }
            };
            for fmt in formats {
                let res = match fmt {
                    Format::Clap => bundle(release, install, universal),
                    Format::Vst3 => bundle_vst3(release, install, universal),
                };
                if let Err(e) = res {
                    eprintln!("xtask: {e}");
                    std::process::exit(1);
                }
            }
        }
        "standalone" => {
            if let Err(e) = standalone(release, install, universal) {
                eprintln!("xtask: {e}");
                std::process::exit(1);
            }
        }
        "web" => {
            // `web` defaults to release (a real deploy ships release+SIMD);
            // `--debug` opts into a debug wasm. `--serve` then serves the bundle
            // with COOP/COEP; `--port N` overrides the default 8080.
            let debug = args.iter().any(|a| a == "--debug");
            let serve = args.iter().any(|a| a == "--serve");
            let port = arg_value(&args, "--port");
            if let Err(e) = web(!debug, serve, port.as_deref()) {
                eprintln!("xtask: {e}");
                std::process::exit(1);
            }
        }
        _ => {
            eprintln!(
                "usage:\n  cargo xtask bundle [--release] [--install] [--universal] [--format clap,vst3]\n  cargo xtask standalone [--release] [--install] [--universal]\n  cargo xtask web [--debug] [--serve] [--port N]"
            );
            std::process::exit(2);
        }
    }
}

/// Value of a `--flag value` pair (e.g. `--port 9000` → `Some("9000")`).
fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Output formats `bundle` can emit, selected by `--format` (E010 / 0011).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Clap,
    Vst3,
}

/// Parse `--format a,b,c` into a deduped, order-preserving format list. Absent
/// or value-less → `[Clap]` (the pre-0011 default — no behaviour change for
/// callers who never pass the flag). Unknown tokens are a hard error.
fn parse_formats(args: &[String]) -> Result<Vec<Format>, String> {
    let Some(raw) = arg_value(args, "--format") else {
        return Ok(vec![Format::Clap]);
    };
    let mut out: Vec<Format> = Vec::new();
    for tok in raw.split(',') {
        let tok = tok.trim();
        if tok.is_empty() {
            continue;
        }
        let fmt = match tok {
            "clap" => Format::Clap,
            "vst3" => Format::Vst3,
            other => {
                return Err(format!(
                    "unknown --format '{other}' (expected comma-separated: clap, vst3)"
                ));
            }
        };
        if !out.contains(&fmt) {
            out.push(fmt);
        }
    }
    if out.is_empty() {
        return Ok(vec![Format::Clap]);
    }
    Ok(out)
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

/// Build `VXN1.vst3` by wrapping the `vxn-clap` staticlib through clap-wrapper
/// (E010 / ticket 0011). The engine, params, controller and faceplate are the
/// same source as the CLAP; VST3 is purely a distribution artifact (ADR 0008).
///
/// Flow: build the staticlib slice(s) → configure + build the `vxn-1/wrapper`
/// CMake project (whole-archives the archive into a VST3 MODULE) → copy the
/// staged `VXN1.vst3` bundle to `target/bundled/`, and on `--install` to the
/// user VST3 directory. macOS (universal) and Windows (x86_64 MSVC, ticket
/// 0012); the wrapper CMake (0010) handles both platforms' bundle layout.
fn bundle_vst3(release: bool, install: bool, universal: bool) -> Result<(), String> {
    if !(cfg!(target_os = "macos") || cfg!(target_os = "windows")) {
        return Err("--format vst3 is supported on macOS and Windows only".into());
    }
    if universal && !cfg!(target_os = "macos") {
        return Err("--universal is macOS-only (omit it on Windows; the build is x86_64)".into());
    }
    let root = workspace_root();
    let profile = if release { "release" } else { "debug" };

    // Preflight: fail early with actionable hints rather than letting CMake or
    // the linker fail opaquely deep in the build.
    ensure_cmake()?;
    ensure_msvc()?;
    ensure_submodules(&root)?;

    // 1. Build the staticlib. `cargo build --package vxn-clap` emits the .a
    //    alongside the cdylib (crate-type cdylib+rlib+staticlib, ticket 0008),
    //    so this also produces the dylib but we only consume the archive. For a
    //    universal build we lipo the two thin archives into one fat .a — the
    //    wrapper force_loads a single archive per link, so two thin slices won't
    //    do (see wrapper/CMakeLists.txt VXN_CLAP_STATIC note).
    let archive = if universal {
        build_universal_static(&root, release)?
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
        let a = static_lib_path(&root.join("target").join(profile));
        if !a.exists() {
            return Err(format!("built static archive not found at {}", a.display()));
        }
        a
    };

    // 2. Configure + build the wrapper CMake project. The build dir is reused
    //    across runs (CMake decides what to rebuild); `rm -rf target/wrapper-*`
    //    forces a clean rebuild (ticket 0011 Notes).
    let build_dir = root.join("target").join(format!("wrapper-{profile}"));
    let out_dir = build_dir.join("out");
    fs::create_dir_all(&build_dir).map_err(io("create wrapper build dir"))?;

    let mut cfg = Command::new("cmake");
    cfg.current_dir(&root)
        .arg("-S")
        .arg("vxn-1/wrapper")
        .arg("-B")
        .arg(&build_dir)
        .arg(format!("-DVXN_CLAP_STATIC={}", archive.display()))
        .arg(format!(
            "-DVXN_CLAP_SDK_DIR={}",
            root.join("vendor/clap").display()
        ))
        .arg(format!(
            "-DVXN_VST3_SDK_DIR={}",
            root.join("vendor/vst3sdk").display()
        ))
        .arg(format!(
            "-DVXN_CLAP_WRAPPER_DIR={}",
            root.join("vendor/clap-wrapper").display()
        ))
        .arg(format!("-DVXN_OUTPUT_DIR={}", out_dir.display()));
    if universal {
        cfg.arg("-DCMAKE_OSX_ARCHITECTURES=arm64;x86_64");
    }
    // Ninja is single-config: without an explicit build type it defaults to an
    // empty/Debug config (the `--config Release` on the build step is ignored),
    // leaving the C++ side on the debug CRT while the Rust staticlib is built
    // `--release`. Pin Release here so both sides use the release runtime; it is
    // harmless on multi-config generators, which honour `--config` instead.
    cfg.arg("-DCMAKE_BUILD_TYPE=Release");
    // Prefer Ninja when present (fast, single-config); otherwise the platform
    // default generator. The `--config Release` on the build below is harmless
    // on Ninja and required on multi-config generators (Xcode/MSBuild).
    if ninja_available() {
        cfg.arg("-G").arg("Ninja");
    }
    let status = cfg
        .status()
        .map_err(|e| format!("failed to run cmake configure: {e}"))?;
    if !status.success() {
        return Err("cmake configure failed (see output above)".into());
    }

    let status = Command::new("cmake")
        .current_dir(&root)
        .arg("--build")
        .arg(&build_dir)
        .arg("--parallel")
        .arg("--config")
        .arg("Release")
        .status()
        .map_err(|e| format!("failed to run cmake --build: {e}"))?;
    if !status.success() {
        return Err("cmake --build failed (see output above)".into());
    }

    // 3. Locate the finished bundle. Our CMake stages it to VXN_OUTPUT_DIR, but
    //    multi-config generators can also leave one under a `Release/` subdir;
    //    find the newest `VXN1.vst3` under the build tree to be generator-proof.
    let vst3 = find_vst3(&out_dir, &build_dir)?;

    // 4. Copy to target/bundled/VXN1.vst3 (mirrors the CLAP output location).
    let bundled = root.join("target").join("bundled");
    fs::create_dir_all(&bundled).map_err(io("create bundled dir"))?;
    let dest = bundled.join(format!("{PLUGIN_NAME}.vst3"));
    let _ = fs::remove_dir_all(&dest);
    copy_dir_recursive(&vst3, &dest)?;
    println!("bundled → {}", dest.display());

    // 5. Optionally install to the user VST3 directory.
    if install {
        let dest_dir = vst3_install_dir()?;
        fs::create_dir_all(&dest_dir).map_err(io("create VST3 install dir"))?;
        let installed = dest_dir.join(format!("{PLUGIN_NAME}.vst3"));
        copy_clap(&dest, &installed)?;
        println!("installed → {}", installed.display());
    }
    Ok(())
}

/// Build `VXN1.app` (macOS) or `VXN1.exe` (Windows) by wrapping the `vxn-clap`
/// staticlib through clap-wrapper's standalone target (E014 / ticket 0028).
/// The standalone provides RtAudio output, RtMidi input, and a native main
/// window that hosts the wry editor via the same `gui` set_parent path DAWs use.
///
/// Flow: build the staticlib slice(s) → configure + build the `standalone/`
/// CMake project → copy the staged artifact to `target/bundled/`. On macOS
/// `--universal` produces a fat-binary `.app`; on Windows always x86_64 MSVC.
fn standalone(release: bool, install: bool, universal: bool) -> Result<(), String> {
    if universal && !cfg!(target_os = "macos") {
        return Err("--universal is macOS-only (omit it on Windows)".into());
    }
    let root = workspace_root();
    let profile = if release { "release" } else { "debug" };

    ensure_cmake()?;
    ensure_msvc()?;
    ensure_submodules_standalone(&root)?;

    let archive = if universal {
        build_universal_static(&root, release)?
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
        let a = static_lib_path(&root.join("target").join(profile));
        if !a.exists() {
            return Err(format!("built static archive not found at {}", a.display()));
        }
        a
    };

    let build_dir = root.join("target").join(format!("standalone-{profile}"));
    let out_dir = build_dir.join("out");
    fs::create_dir_all(&build_dir).map_err(io("create standalone build dir"))?;

    let mut cfg = Command::new("cmake");
    cfg.current_dir(&root)
        .arg("-S")
        .arg("standalone")
        .arg("-B")
        .arg(&build_dir)
        .arg(format!("-DVXN_CLAP_STATIC={}", archive.display()))
        .arg(format!(
            "-DVXN_CLAP_SDK_DIR={}",
            root.join("vendor/clap").display()
        ))
        .arg(format!(
            "-DVXN_CLAP_WRAPPER_DIR={}",
            root.join("vendor/clap-wrapper").display()
        ))
        .arg(format!("-DVXN_OUTPUT_DIR={}", out_dir.display()))
        .arg("-DVXN_PLUGIN_NAME=VXN1")
        .arg("-DVXN_BUNDLE_ID=labs.vulpus.vxn1.standalone");
    if universal {
        cfg.arg("-DCMAKE_OSX_ARCHITECTURES=arm64;x86_64");
    }
    if ninja_available() {
        cfg.arg("-G").arg("Ninja");
    }
    let status = cfg
        .status()
        .map_err(|e| format!("failed to run cmake configure: {e}"))?;
    if !status.success() {
        return Err("cmake configure failed (see output above)".into());
    }

    let status = Command::new("cmake")
        .current_dir(&root)
        .arg("--build")
        .arg(&build_dir)
        .arg("--parallel")
        .arg("--config")
        .arg("Release")
        .status()
        .map_err(|e| format!("failed to run cmake --build: {e}"))?;
    if !status.success() {
        return Err("cmake --build failed (see output above)".into());
    }

    let artifact = find_standalone(&out_dir, &build_dir)?;

    let bundled = root.join("target").join("bundled");
    fs::create_dir_all(&bundled).map_err(io("create bundled dir"))?;

    if cfg!(target_os = "macos") {
        let dest = bundled.join("VXN1.app");
        let _ = fs::remove_dir_all(&dest);
        copy_dir_recursive(&artifact, &dest)?;
        println!("bundled → {}", dest.display());

        if install {
            let app_dir = PathBuf::from("/Applications");
            let installed = app_dir.join("VXN1.app");
            let _ = fs::remove_dir_all(&installed);
            copy_dir_recursive(&dest, &installed)?;
            println!("installed → {}", installed.display());
        }
    } else {
        let dest = bundled.join("VXN1.exe");
        let _ = fs::remove_file(&dest);
        fs::copy(&artifact, &dest).map_err(io("copy standalone exe"))?;
        println!("bundled → {}", dest.display());
    }
    Ok(())
}

/// Find the staged standalone artifact. On macOS looks for `VXN1.app`; on
/// Windows/Linux for `VXN1.exe`. Prefers the copy our CMake stages into
/// `out_dir`; falls back to a search under the build tree.
fn find_standalone(out_dir: &Path, build_dir: &Path) -> Result<PathBuf, String> {
    let name = if cfg!(target_os = "macos") {
        "VXN1.app"
    } else {
        "VXN1.exe"
    };
    let staged = out_dir.join(name);
    if staged.exists() {
        return Ok(staged);
    }
    // Multi-config generator fallback: search the build tree.
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    if cfg!(target_os = "macos") {
        find_named_dirs(build_dir, name, &mut |p| {
            let mtime = fs::metadata(p).and_then(|m| m.modified()).ok();
            if let Some(t) = mtime
                && best.as_ref().map(|(bt, _)| t > *bt).unwrap_or(true)
            {
                best = Some((t, p.to_path_buf()));
            }
        });
    } else {
        // Windows/Linux: find the exe file
        let _ = find_files_named(build_dir, name, &mut |p| {
            let mtime = fs::metadata(p).and_then(|m| m.modified()).ok();
            if let Some(t) = mtime
                && best.as_ref().map(|(bt, _)| t > *bt).unwrap_or(true)
            {
                best = Some((t, p.to_path_buf()));
            }
        });
    }
    best.map(|(_, p)| p).ok_or_else(|| {
        format!(
            "{name} not found under {} after a successful build",
            build_dir.display()
        )
    })
}

/// Recursively find files named `name` under `dir`, calling `f` on each match.
fn find_files_named(dir: &Path, name: &str, f: &mut impl FnMut(&Path)) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_file() && entry.file_name() == *name {
            f(&p);
        } else if p.is_dir() {
            find_files_named(&p, name, f);
        }
    }
}

/// Like [`ensure_submodules`] but checks the standalone-specific set: vendor/clap
/// and vendor/clap-wrapper (no vst3sdk needed for standalone).
fn ensure_submodules_standalone(root: &Path) -> Result<(), String> {
    for sub in ["vendor/clap", "vendor/clap-wrapper"] {
        let p = root.join(sub);
        let empty = fs::read_dir(&p)
            .map(|mut d| d.next().is_none())
            .unwrap_or(true);
        if empty {
            return Err(format!(
                "submodule {sub} is missing or empty — run \
                 `git submodule update --init --recursive`"
            ));
        }
    }
    Ok(())
}

/// Path to the `vxn-clap` static archive under a profile dir (the `.a`/`.lib`
/// analogue of [`lib_path`]).
fn static_lib_path(profile_dir: &Path) -> PathBuf {
    if cfg!(target_os = "windows") {
        profile_dir.join(format!("{LIB_NAME}.lib"))
    } else {
        profile_dir.join(format!("lib{LIB_NAME}.a"))
    }
}

/// Build both macOS slices of the staticlib and `lipo` them into one fat
/// archive; returns its path. The static analogue of [`build_universal`] — the
/// wrapper force_loads a single archive, so the slices must be combined here.
fn build_universal_static(root: &Path, release: bool) -> Result<PathBuf, String> {
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
        let a = static_lib_path(&root.join("target").join(triple).join(profile));
        if !a.exists() {
            return Err(format!(
                "{triple} static archive not found at {}",
                a.display()
            ));
        }
        slices.push(a);
    }

    let out_dir = root.join("target").join("universal").join(profile);
    fs::create_dir_all(&out_dir).map_err(io("create universal dir"))?;
    let out = out_dir.join(format!("lib{LIB_NAME}.a"));
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

/// Error unless `cmake` is invokable, with an install hint.
fn ensure_cmake() -> Result<(), String> {
    Command::new("cmake")
        .arg("--version")
        .output()
        .map(|_| ())
        .map_err(|_| {
            "cmake not found on PATH — install it (`brew install cmake`, or \
             https://cmake.org/download/) to build the VST3"
                .to_string()
        })
}

/// On Windows, error unless the MSVC toolchain (`cl.exe`) is reachable, hinting
/// at the Developer PowerShell. We deliberately don't locate and source
/// `vcvars64.bat` ourselves — that's a rabbit hole (ticket 0012). No-op on
/// other platforms. Spawn succeeding (even with a non-zero "no input" exit)
/// proves the compiler is on PATH; the env vars (`INCLUDE`/`LIB`) that the
/// Ninja+MSVC build needs come from the same Developer shell.
fn ensure_msvc() -> Result<(), String> {
    if !cfg!(target_os = "windows") {
        return Ok(());
    }
    Command::new("cl.exe").output().map(|_| ()).map_err(|_| {
        "MSVC compiler (cl.exe) not found on PATH — run xtask from a \
         \"Developer PowerShell for VS 2022\" (or a shell where you've run \
         vcvars64.bat) so the C++ toolchain and its INCLUDE/LIB env are set"
            .to_string()
    })
}

/// Error unless the `vendor/` submodules the wrapper CMake needs are checked
/// out, pointing at the init command rather than letting CMake fail opaquely.
fn ensure_submodules(root: &Path) -> Result<(), String> {
    for sub in ["vendor/clap", "vendor/clap-wrapper", "vendor/vst3sdk"] {
        let p = root.join(sub);
        let empty = fs::read_dir(&p)
            .map(|mut d| d.next().is_none())
            .unwrap_or(true);
        if empty {
            return Err(format!(
                "submodule {sub} is missing or empty — run \
                 `git submodule update --init --recursive`"
            ));
        }
    }
    Ok(())
}

/// Whether `ninja` is invokable (preferred CMake generator when present).
fn ninja_available() -> bool {
    Command::new("ninja").arg("--version").output().is_ok()
}

/// Find the staged `VXN1.vst3` bundle. Prefer the copy our CMake stages into
/// `out_dir`; fall back to the newest `VXN1.vst3` anywhere under the build tree
/// (multi-config generators can place it under a `Release/` subdir).
fn find_vst3(out_dir: &Path, build_dir: &Path) -> Result<PathBuf, String> {
    let staged = out_dir.join(format!("{PLUGIN_NAME}.vst3"));
    if staged.exists() {
        return Ok(staged);
    }
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    find_named_dirs(build_dir, &format!("{PLUGIN_NAME}.vst3"), &mut |p| {
        let mtime = fs::metadata(p).and_then(|m| m.modified()).ok();
        if let Some(t) = mtime
            && best.as_ref().map(|(bt, _)| t > *bt).unwrap_or(true)
        {
            best = Some((t, p.to_path_buf()));
        }
    });
    best.map(|(_, p)| p).ok_or_else(|| {
        format!(
            "VXN1.vst3 not found under {} after a successful build",
            build_dir.display()
        )
    })
}

/// Recursively visit directories named `name` under `dir`, calling `f` on each.
/// Does not descend into a matched directory (a bundle is a leaf for our needs).
fn find_named_dirs(dir: &Path, name: &str, f: &mut impl FnMut(&Path)) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if entry.file_name() == *name {
                f(&p);
            } else {
                find_named_dirs(&p, name, f);
            }
        }
    }
}

/// The user VST3 install directory for the host platform.
fn vst3_install_dir() -> Result<PathBuf, String> {
    if cfg!(target_os = "macos") {
        let home = env::var("HOME").map_err(|_| "HOME not set".to_string())?;
        Ok(PathBuf::from(home).join("Library/Audio/Plug-Ins/VST3"))
    } else if cfg!(target_os = "windows") {
        // Per-user VST3 path (`%LOCALAPPDATA%\Programs\Common\VST3`, ticket 0012)
        // rather than the machine-wide `%CommonProgramFiles%\VST3` — the latter
        // needs admin and we install for the current user, matching the CLAP path.
        let local =
            env::var("LOCALAPPDATA").map_err(|_| "LOCALAPPDATA not set".to_string())?;
        Ok(PathBuf::from(local).join(r"Programs\Common\VST3"))
    } else {
        let home = env::var("HOME").map_err(|_| "HOME not set".to_string())?;
        Ok(PathBuf::from(home).join(".vst3"))
    }
}

const WASM_PKG: &str = "vxn-wasm";
const WASM_ARTIFACT: &str = "vxn_wasm.wasm";

// 0044: the main-thread controller wasm — a SECOND module (engine runs in the
// worklet, controller on main; ADR 0009 §1). Built into the same web bundle.
const CONTROLLER_PKG: &str = "vxn-web-controller";
const CONTROLLER_ARTIFACT: &str = "vxn_web_controller.wasm";

/// Build the wasm and assemble a self-contained `target/web-dist/` (ticket 0041).
///
/// One command → a servable directory: the engine `.wasm` (release + SIMD128 by
/// default), the E015 transport JS modules, the production worklet, and a page.
/// `serve` then hands the bundle to `serve-coep.mjs` with the COOP/COEP headers
/// `SharedArrayBuffer` needs (ticket 0045); the AudioContext boot that drives it
/// is 0042.
fn web(release: bool, serve: bool, port: Option<&str>) -> Result<(), String> {
    let root = workspace_root();
    let profile = if release { "release" } else { "debug" };

    // 1. Compile BOTH wasm crates for wasm32-unknown-unknown (ADR 0009 §1):
    //    the engine (runs in the worklet) and the main-thread controller (0044).
    let wasm = build_wasm(&root, WASM_PKG, WASM_ARTIFACT, release, profile)?;
    let controller_wasm = build_wasm(&root, CONTROLLER_PKG, CONTROLLER_ARTIFACT, release, profile)?;

    // 2. Assemble target/web-dist/ from scratch (a clean, portable copy).
    let dist = root.join("target").join("web-dist");
    let _ = fs::remove_dir_all(&dist);
    fs::create_dir_all(&dist).map_err(io("create web-dist"))?;

    // 2a. Both wasm modules.
    fs::copy(&wasm, dist.join(WASM_ARTIFACT)).map_err(io("copy engine wasm"))?;
    fs::copy(&controller_wasm, dist.join(CONTROLLER_ARTIFACT))
        .map_err(io("copy controller wasm"))?;

    // 2b. The E015 production transport modules + worklet. Curated by hand: the
    //     *.test.mjs suites and the Node harnesses stay out of the shipped
    //     bundle. The production worklet is `vxn-processor.js`.
    let web_src = root.join("vxn-1/crates/vxn-wasm/web");
    const MODULES: [(&str, &str); 16] = [
        ("event-ring.mjs", "event-ring.mjs"),
        ("event-codec.mjs", "event-codec.mjs"),
        ("param-store.mjs", "param-store.mjs"),
        ("audio-host.mjs", "audio-host.mjs"),
        ("host-runner.mjs", "host-runner.mjs"),
        ("vxn-processor.js", "vxn-processor.js"),
        // The main-thread coordinator (ticket 0042): the page imports WebHost.
        ("coordinator.mjs", "coordinator.mjs"),
        // The controller wasm glue (ticket 0044): instantiates the controller
        // module, posts UiEvent opcodes, drains ViewEvents, mirrors the SAB.
        ("controller.mjs", "controller.mjs"),
        // User-preset persistence (E019 / 0063-0064): the IndexedDB primitive +
        // the async-storage <-> sync-controller bridge faceplate-bridge imports.
        ("preset-storage.mjs", "preset-storage.mjs"),
        ("preset-persistence.mjs", "preset-persistence.mjs"),
        // Full patch-state autosave/restore (E019 / 0065): the host-state-blob
        // analogue faceplate-bridge restores at boot + debounces writes on edit.
        ("state-autosave.mjs", "state-autosave.mjs"),
        // Patch export/import + URL share-link (E019 / 0066): faceplate-bridge
        // injects the preset-bar controls + applies a `#patch=` link at boot.
        ("patch-io.mjs", "patch-io.mjs"),
        // E017 input adapters (tickets 0053-0056): browser input → E015 ring
        // producers. The faceplate (E018) imports attachMidi / attachKeyboard /
        // attachKeyMode. .test.mjs suites stay out of the bundle as usual.
        ("midi-input.mjs", "midi-input.mjs"),
        ("keyboard-input.mjs", "keyboard-input.mjs"),
        ("key-mode.mjs", "key-mode.mjs"),
        // The faceplate transport bridge (E018 / 0057-0061): boots WebHost +
        // WebController, routes opcodes <-> ViewEvents, runs the DOM text input.
        ("faceplate-bridge.mjs", "faceplate-bridge.mjs"),
    ];
    for (src, dest) in MODULES {
        let from = web_src.join(src);
        if !from.exists() {
            return Err(format!("missing web module {}", from.display()));
        }
        fs::copy(&from, dist.join(dest)).map_err(io("copy web module"))?;
    }

    // 2c. The faceplate page (E018 / 0057). Generated by the `gen-web-page` bin
    //     in `vxn-ui-web`, which assembles the SAME splice the plugin uses
    //     (markup + CSS + JS + byte-identical param-descriptor JSON) with the wry
    //     IPC swapped for the web boot head + `faceplate-bridge.mjs` loader. Run
    //     as a subprocess so xtask carries no wry-pulling dependency and the
    //     JSON-shaping stays single-sourced. The 0042 coordinator-smoke page
    //     (`web_index_html`) is retired by this.
    let page = gen_faceplate_page(&root)?;
    fs::write(dist.join("index.html"), page).map_err(io("write index.html"))?;

    // 2c'. The baked factory bank (E019 / 0062). Run vxn-engine's `bake-factory`
    //      bin, which serializes the embedded bank (meta + canonical state blob
    //      per preset) through the SAME `EnginePresetStore` the desktop build
    //      serves, and capture its stdout as `factory.bin`. The page fetches it
    //      at boot and feeds it to the controller (deps only vxn-app, ADR 0009).
    let factory_bin = bake_factory_bin(&root)?;
    fs::write(dist.join("factory.bin"), factory_bin).map_err(io("write factory.bin"))?;

    // 2d. A Netlify-style `_headers` file so dropping dist/ onto a static host
    //     (Netlify / Cloudflare Pages, both read `_headers`) carries the same
    //     two isolation headers the dev server sets — no extra config. The dev
    //     server (serve-coep.mjs) ignores it; it's purely the prod recipe baked
    //     into the artifact (ticket 0045 / WEB-HOSTING.md).
    fs::write(dist.join("_headers"), web_dist_headers()).map_err(io("write _headers"))?;

    println!("web bundle → {}", dist.display());

    if serve {
        return serve_dist(&root, &dist, port);
    }
    println!(
        "  note: SharedArrayBuffer needs cross-origin isolation — serve with \
         COOP/COEP (`cargo xtask web --serve`, ticket 0045)"
    );
    Ok(())
}

/// Bake the embedded factory bank into the flat `factory.bin` asset (E019 /
/// 0062) by running `vxn-engine`'s `bake-factory` bin and capturing its stdout.
/// Run as a subprocess so xtask carries no engine dependency and the asset
/// codec stays single-sourced in `vxn-app::factory_asset`.
fn bake_factory_bin(root: &Path) -> Result<Vec<u8>, String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(&cargo)
        .current_dir(root)
        .args([
            "run",
            "--quiet",
            "--release",
            "--package",
            "vxn-engine",
            "--bin",
            "bake-factory",
        ])
        .output()
        .map_err(|e| format!("failed to run bake-factory: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "bake-factory failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(out.stdout)
}

/// Generate the faceplate `index.html` by running `vxn-ui-web`'s `gen-web-page`
/// bin and capturing its stdout (E018 / 0057). The bin assembles the page via
/// `vxn_ui_web::build_web_faceplate_html` — the same splice the plugin's wry
/// editor uses, so the markup/CSS/JS and the param-descriptor JSON are byte-
/// identical; only the transport head differs. Running it as a subprocess keeps
/// xtask free of the (wry-pulling) `vxn-ui-web` dependency.
fn gen_faceplate_page(root: &Path) -> Result<String, String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(&cargo)
        .current_dir(root)
        .args([
            "run",
            "--quiet",
            "--package",
            "vxn-ui-web",
            "--bin",
            "gen-web-page",
        ])
        .output()
        .map_err(|e| format!("failed to run gen-web-page: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "gen-web-page failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("gen-web-page emitted non-UTF8: {e}"))
}

/// Netlify/Cloudflare-Pages `_headers`: applies the COOP/COEP (+CORP) headers to
/// every path so the served document is cross-origin isolated and SAB is
/// constructible. Same three headers `serve-coep.mjs` sets locally.
fn web_dist_headers() -> &'static str {
    "/*\n  \
     Cross-Origin-Opener-Policy: same-origin\n  \
     Cross-Origin-Embedder-Policy: require-corp\n  \
     Cross-Origin-Resource-Policy: same-origin\n"
}

/// Serve the built bundle with COOP/COEP via the crate's `serve-coep.mjs`
/// (ticket 0045). Requires `node` on PATH. Blocks until the server is killed.
fn serve_dist(root: &Path, dist: &Path, port: Option<&str>) -> Result<(), String> {
    let server = root.join("vxn-1/crates/vxn-wasm/serve-coep.mjs");
    if !server.exists() {
        return Err(format!("serve-coep.mjs not found at {}", server.display()));
    }
    let port = port.unwrap_or("8080");
    let mut cmd = Command::new("node");
    cmd.current_dir(root).arg(&server).arg(port).arg(dist);
    let status = cmd
        .status()
        .map_err(|e| format!("failed to run node (is it on PATH?): {e}"))?;
    // The server runs until Ctrl-C; a non-success exit (e.g. port in use) is an
    // error worth surfacing.
    if !status.success() {
        return Err("serve-coep.mjs exited with an error".into());
    }
    Ok(())
}

/// Compile one wasm crate for `wasm32-unknown-unknown` (release + SIMD128 by
/// default) and return the path to its `.wasm` artifact. Shared by the engine
/// and the 0044 controller builds so both go through the same flags.
fn build_wasm(
    root: &Path,
    package: &str,
    artifact: &str,
    release: bool,
    profile: &str,
) -> Result<PathBuf, String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut build = Command::new(&cargo);
    build.current_dir(root).args([
        "build",
        "--package",
        package,
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
