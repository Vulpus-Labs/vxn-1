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

/// CLAP bundle stem *and* the executable name inside the macOS bundle.
const PLUGIN_NAME: &str = "vxn1b";
const BUNDLE_NAME: &str = "vxn1b.clap";
/// VST3 bundle stem. Capitalised, unlike the CLAP — see the module docs.
/// VST3 bundle FILENAME (`VXN1b.vst3`), not a display name. Renaming it would
/// orphan every installed copy and re-scan as a new plugin, so it keeps the
/// original spelling; the name a host shows comes from the CLAP descriptor.
const VST3_NAME: &str = "VXN1b";
const BUNDLE_ID: &str = "labs.vulpus.vxn1b";
/// Human-readable product name — the DAW's plugin list and CFBundleName.
/// Hyphenated, matching the faceplate banner and the other synths' styling.
/// NOT an identifier: `BUNDLE_ID`, `VST3_NAME` and the user-preset directory
/// deliberately keep their unhyphenated spelling (see their notes).
const DISPLAY_NAME: &str = "VXN-1b";
/// The cdylib/staticlib file stem: the `vxn1b-clap` package name with `-` → `_`
/// (cargo's crate-name rule). Coupled to the `--package vxn1b-clap` build below
/// by hand. A rename can't *silently* ship an empty bundle: every build path
/// checks the produced artifact exists at this name and errors with the path if
/// it doesn't — update this constant and `CLAP_PACKAGE` together.
const LIB_NAME: &str = "vxn1b_clap";
const CLAP_PACKAGE: &str = "vxn1b-clap";

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
        "bundle" => run_formats(&formats, |fmt| match fmt {
            Format::Clap => bundle(universal, false).map(|_| ()),
            Format::Vst3 => bundle_vst3(universal, false),
        }),
        "install" => run_formats(&formats, |fmt| match fmt {
            Format::Clap => bundle(universal, true).map(|_| ()),
            Format::Vst3 => bundle_vst3(universal, true),
        }),
        "uninstall" => run_formats(&formats, |fmt| uninstall(fmt)),
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

/// Run `f` for each requested format, stopping at the first failure. Formats are
/// independent artifacts, so a CLAP that built fine still stands when the VST3
/// leg fails — the error names which one.
fn run_formats(
    formats: &[Format],
    mut f: impl FnMut(Format) -> Result<(), String>,
) -> Result<(), String> {
    for &fmt in formats {
        f(fmt).map_err(|e| format!("{}: {e}", fmt.as_str()))?;
    }
    Ok(())
}

fn print_help() {
    println!(
        "cargo xtask <subcommand> [--universal] [--format clap,vst3]

Subcommands:
  bundle      Build {CLAP_PACKAGE} (release) and stage the artifact(s) in target/bundled/.
  install     Bundle, then copy to the user CLAP/VST3 directories.
  uninstall   Remove the installed artifact(s) if present.
  web         Build the browser bundle into target/web-dist-vxn1b/: both wasm modules,
              the transport JS + worklet, the generated faceplate page, and a
              COOP/COEP _headers. Pass --serve [--port N] for the dev server.
  --help      Show this message.

Flags:
  --universal  macOS only: build arm64 + x86_64 and lipo into one fat binary.
  --format     Comma-separated artifacts to produce (default: clap).
               `vst3` needs CMake and `git submodule update --init --recursive`."
    );
}

/// Output formats the commands can act on, selected by `--format`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Format {
    Clap,
    Vst3,
}

impl Format {
    fn as_str(self) -> &'static str {
        match self {
            Format::Clap => "clap",
            Format::Vst3 => "vst3",
        }
    }
}

/// Value of a `--flag value` pair (e.g. `--format clap,vst3`).
fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Parse `--format a,b,c` into a deduped, order-preserving format list. Absent
/// or value-less → `[Clap]`, which is what `deploy.sh` relies on. Unknown tokens
/// are a hard error rather than a silent skip: a typo'd format that quietly
/// built only the CLAP would look like a successful VST3 build.
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
    // CARGO_MANIFEST_DIR is .../vxn-1b/xtask/. The flat workspace root sits two
    // levels up (the repo root is a single workspace).
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf()
}

fn bundled_dir() -> PathBuf {
    workspace_root().join("target").join("bundled")
}

/// Path to the `vxn1b-clap` shared library under a profile dir.
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

/// Path to the `vxn1b-clap` static archive under a profile dir (the `.a`/`.lib`
/// analogue of [`lib_path`]).
fn static_lib_path(profile_dir: &Path) -> PathBuf {
    if cfg!(target_os = "windows") {
        profile_dir.join(format!("{LIB_NAME}.lib"))
    } else {
        profile_dir.join(format!("lib{LIB_NAME}.a"))
    }
}

/// The user CLAP install directory for the host platform.
fn clap_install_dir() -> Result<PathBuf, String> {
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

/// The user VST3 install directory for the host platform.
fn vst3_install_dir() -> Result<PathBuf, String> {
    if cfg!(target_os = "macos") {
        let home = env::var("HOME").map_err(|_| "HOME not set".to_string())?;
        Ok(PathBuf::from(home).join("Library/Audio/Plug-Ins/VST3"))
    } else if cfg!(target_os = "windows") {
        // Per-user VST3 path rather than the machine-wide
        // `%CommonProgramFiles%\VST3` — the latter needs admin, and we install
        // for the current user, matching the CLAP path above.
        let local = env::var("LOCALAPPDATA").map_err(|_| "LOCALAPPDATA not set".to_string())?;
        Ok(PathBuf::from(local).join(r"Programs\Common\VST3"))
    } else {
        let home = env::var("HOME").map_err(|_| "HOME not set".to_string())?;
        Ok(PathBuf::from(home).join(".vst3"))
    }
}

/// Where a given format's installed artifact lives.
fn install_path(fmt: Format) -> Result<PathBuf, String> {
    Ok(match fmt {
        Format::Clap => clap_install_dir()?.join(BUNDLE_NAME),
        Format::Vst3 => vst3_install_dir()?.join(format!("{VST3_NAME}.vst3")),
    })
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

/// Run `cargo build --release -p vxn1b-clap`, optionally for one `--target`.
/// Emits the cdylib *and* the staticlib in one go (crate-type cdylib+rlib+
/// staticlib), so both bundle paths share this.
fn cargo_build(root: &Path, triple: Option<&str>) -> Result<(), String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut build = Command::new(&cargo);
    build
        .current_dir(root)
        .args(["build", "--release", "--package", CLAP_PACKAGE]);
    if let Some(t) = triple {
        build.args(["--target", t]);
    }
    let status = build
        .status()
        .map_err(|e| format!("failed to run cargo: {e}"))?;
    if !status.success() {
        return Err(match triple {
            Some(t) => format!("cargo build failed for {t}"),
            None => "cargo build failed".into(),
        });
    }
    Ok(())
}

const TRIPLES: [&str; 2] = ["aarch64-apple-darwin", "x86_64-apple-darwin"];

/// Build both macOS slices and `lipo` them into one fat artifact; returns its
/// path. `pick` selects the cdylib or the staticlib out of each slice's profile
/// dir, and `out_name` is the fat file's name — the two builds are otherwise
/// identical, and the wrapper force_loads a single archive per link, so the
/// static slices *must* be combined here rather than passed as two.
fn build_universal(
    root: &Path,
    pick: fn(&Path) -> PathBuf,
    out_name: &str,
) -> Result<PathBuf, String> {
    let mut slices = Vec::new();
    for triple in TRIPLES {
        cargo_build(root, Some(triple))?;
        let artifact = pick(&root.join("target").join(triple).join("release"));
        if !artifact.exists() {
            return Err(format!(
                "{triple} artifact not found at {}",
                artifact.display()
            ));
        }
        slices.push(artifact);
    }

    let out_dir = root.join("target").join("universal").join("release");
    fs::create_dir_all(&out_dir).map_err(io("create universal dir"))?;
    let out = out_dir.join(out_name);
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

/// Build the CLAP and stage it in `target/bundled/`; optionally install it.
fn bundle(universal: bool, install: bool) -> Result<PathBuf, String> {
    let root = workspace_root();
    touch_factory()?;

    // 1. Compile the cdylib (one fat dylib for a macOS universal build,
    //    otherwise the host-target shared library).
    let lib = if universal {
        if !cfg!(target_os = "macos") {
            return Err("--universal is macOS-only".into());
        }
        build_universal(&root, lib_path, &format!("lib{LIB_NAME}.dylib"))?
    } else {
        cargo_build(&root, None)?;
        let lib = lib_path(&root.join("target").join("release"));
        if !lib.exists() {
            return Err(format!(
                "built library not found at {} (cross-compile target?)",
                lib.display()
            ));
        }
        lib
    };

    // 2. Assemble the .clap.
    let out_dir = bundled_dir();
    fs::create_dir_all(&out_dir).map_err(io("create bundled dir"))?;
    let clap_path = out_dir.join(BUNDLE_NAME);

    if cfg!(target_os = "macos") {
        build_macos_bundle(&clap_path, &lib)?;
    } else {
        // Linux/Windows: a CLAP is just the shared library with a .clap name.
        let _ = fs::remove_file(&clap_path);
        fs::copy(&lib, &clap_path).map_err(io("copy library"))?;
    }
    println!("bundled → {}", clap_path.display());

    if install {
        install_artifact(Format::Clap, &clap_path)?;
    }
    Ok(clap_path)
}

fn build_macos_bundle(clap_path: &Path, lib: &Path) -> Result<(), String> {
    let _ = fs::remove_dir_all(clap_path);
    let macos_dir = clap_path.join("Contents").join("MacOS");
    fs::create_dir_all(&macos_dir).map_err(io("create Contents/MacOS"))?;
    fs::copy(lib, macos_dir.join(PLUGIN_NAME)).map_err(io("copy dylib into bundle"))?;
    fs::write(clap_path.join("Contents").join("Info.plist"), info_plist())
        .map_err(io("write Info.plist"))?;
    fs::write(clap_path.join("Contents").join("PkgInfo"), "BNDL????")
        .map_err(io("write PkgInfo"))?;
    Ok(())
}

/// Build `VXN1b.vst3` by wrapping the `vxn1b-clap` staticlib through
/// clap-wrapper (0213). The engine, params, controller and faceplate are the
/// same source as the CLAP; VST3 is purely a distribution artifact.
///
/// Flow: build the staticlib slice(s) → configure + build the `vxn-1b/wrapper`
/// CMake project (whole-archives the archive into a VST3 MODULE) → copy the
/// staged bundle to `target/bundled/`, and on install to the user VST3
/// directory. macOS (universal) and Windows (x86_64 MSVC); the wrapper CMake
/// handles both platforms' bundle layout.
fn bundle_vst3(universal: bool, install: bool) -> Result<(), String> {
    if !(cfg!(target_os = "macos") || cfg!(target_os = "windows")) {
        return Err("--format vst3 is supported on macOS and Windows only".into());
    }
    if universal && !cfg!(target_os = "macos") {
        return Err("--universal is macOS-only (omit it on Windows; the build is x86_64)".into());
    }
    let root = workspace_root();

    // Preflight: fail early with actionable hints rather than letting CMake or
    // the linker fail opaquely deep in the build.
    ensure_cmake()?;
    ensure_msvc()?;
    ensure_submodules(&root)?;

    touch_factory()?;

    // 1. Build the staticlib. The cdylib comes out of the same invocation; we
    //    only consume the archive here.
    let archive = if universal {
        build_universal(&root, static_lib_path, &format!("lib{LIB_NAME}.a"))?
    } else {
        cargo_build(&root, None)?;
        let a = static_lib_path(&root.join("target").join("release"));
        if !a.exists() {
            return Err(format!("built static archive not found at {}", a.display()));
        }
        a
    };

    // 2. Configure + build the wrapper CMake project. The build dir is reused
    //    across runs (CMake decides what to rebuild); `rm -rf
    //    target/vxn1b-wrapper-release` forces a clean rebuild.
    let build_dir = root.join("target").join("vxn1b-wrapper-release");
    let out_dir = build_dir.join("out");
    fs::create_dir_all(&build_dir).map_err(io("create wrapper build dir"))?;

    let mut cfg = Command::new("cmake");
    cfg.current_dir(&root)
        .arg("-S")
        .arg("vxn-1b/wrapper")
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
    //    find the newest match under the build tree to be generator-proof.
    let vst3 = find_vst3(&out_dir, &build_dir)?;

    // 4. Copy to target/bundled/ (mirrors the CLAP output location).
    let bundled = bundled_dir();
    fs::create_dir_all(&bundled).map_err(io("create bundled dir"))?;
    let dest = bundled.join(format!("{VST3_NAME}.vst3"));
    let _ = fs::remove_dir_all(&dest);
    copy_dir_recursive(&vst3, &dest)?;
    println!("bundled → {}", dest.display());

    if install {
        install_artifact(Format::Vst3, &dest)?;
    }
    Ok(())
}

/// Copy a staged artifact into its user plug-in directory.
fn install_artifact(fmt: Format, src: &Path) -> Result<(), String> {
    let dest = install_path(fmt)?;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(io("create install dir"))?;
    }
    copy_artifact(src, &dest)?;
    println!("installed → {}", dest.display());
    Ok(())
}

fn uninstall(fmt: Format) -> Result<(), String> {
    let dest = install_path(fmt)?;
    if !dest.exists() {
        println!("nothing to uninstall at {}", dest.display());
        return Ok(());
    }
    // A macOS/Windows-folder bundle is a directory; a Linux/Windows CLAP is a
    // single file. Remove whichever this one is.
    if dest.is_dir() {
        fs::remove_dir_all(&dest).map_err(io("remove install"))?;
    } else {
        fs::remove_file(&dest).map_err(io("remove install"))?;
    }
    println!("uninstalled → {}", dest.display());
    Ok(())
}

/// Copy a bundle (directory) or a bare `.clap` (file) over any existing one.
fn copy_artifact(src: &Path, dest: &Path) -> Result<(), String> {
    if src.is_dir() {
        let _ = fs::remove_dir_all(dest);
        copy_dir_recursive(src, dest)
    } else {
        let _ = fs::remove_file(dest);
        fs::copy(src, dest).map(|_| ()).map_err(io("install copy"))
    }
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
/// `vcvars64.bat` ourselves — that's a rabbit hole. No-op on other platforms.
/// Spawn succeeding (even with a non-zero "no input" exit) proves the compiler
/// is on PATH; the env vars (`INCLUDE`/`LIB`) that the Ninja+MSVC build needs
/// come from the same Developer shell.
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

/// Error unless the repo-root `vendor/` submodules the wrapper CMake needs are
/// checked out, pointing at the init command rather than letting CMake fail
/// opaquely. These are the same checkouts vxn-1 and vxn-2 use.
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

/// Find the staged `VXN1b.vst3` bundle. Prefer the copy our CMake stages into
/// `out_dir`; fall back to the newest match anywhere under the build tree
/// (multi-config generators can place it under a `Release/` subdir).
fn find_vst3(out_dir: &Path, build_dir: &Path) -> Result<PathBuf, String> {
    let name = format!("{VST3_NAME}.vst3");
    let staged = out_dir.join(&name);
    if staged.exists() {
        return Ok(staged);
    }
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    find_named_dirs(build_dir, &name, &mut |p| {
        let mtime = fs::metadata(p).and_then(|m| m.modified()).ok();
        if let Some(t) = mtime
            && best.as_ref().map(|(bt, _)| t > *bt).unwrap_or(true)
        {
            best = Some((t, p.to_path_buf()));
        }
    });
    best.map(|(_, p)| p).ok_or_else(|| {
        format!(
            "{name} not found under {} after a successful build",
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
    <key>LSMinimumSystemVersion</key><string>11.0.0</string>
</dict>
</plist>
"#,
        version = env!("CARGO_PKG_VERSION"),
    )
}

fn io(ctx: &'static str) -> impl Fn(std::io::Error) -> String {
    move |e| format!("{ctx}: {e}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn absent_format_defaults_to_clap() {
        // `deploy.sh` passes no `--format`; that must stay a CLAP-only build.
        let f = parse_formats(&args(&["bundle"])).unwrap();
        assert_eq!(f.len(), 1);
        assert!(f[0] == Format::Clap);
    }

    #[test]
    fn both_formats_parse_in_order_and_dedupe() {
        let f = parse_formats(&args(&["bundle", "--format", "vst3,clap,vst3"])).unwrap();
        assert_eq!(
            f.iter().map(|x| x.as_str()).collect::<Vec<_>>(),
            ["vst3", "clap"]
        );
    }

    #[test]
    fn an_unknown_format_is_an_error_not_a_silent_clap_build() {
        let err = parse_formats(&args(&["bundle", "--format", "au"])).unwrap_err();
        assert!(err.contains("au"), "{err}");
    }

    #[test]
    fn a_valueless_or_empty_format_falls_back_to_clap() {
        assert_eq!(parse_formats(&args(&["bundle", "--format"])).unwrap().len(), 1);
        assert_eq!(
            parse_formats(&args(&["bundle", "--format", ","]))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn the_plist_stamps_the_crate_version_and_bundle_identity() {
        let plist = info_plist();
        assert!(plist.contains(&format!("<string>{}</string>", env!("CARGO_PKG_VERSION"))));
        assert!(plist.contains(BUNDLE_ID));
        // CFBundleExecutable must match the file the bundle actually contains.
        assert!(plist.contains(&format!(
            "<key>CFBundleExecutable</key><string>{PLUGIN_NAME}</string>"
        )));
    }

    #[test]
    fn run_formats_names_the_failing_leg() {
        let err = run_formats(&[Format::Clap, Format::Vst3], |f| match f {
            Format::Clap => Ok(()),
            Format::Vst3 => Err("cmake exploded".into()),
        })
        .unwrap_err();
        assert_eq!(err, "vst3: cmake exploded");
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
