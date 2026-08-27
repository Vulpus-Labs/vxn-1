//! The VXN bundler, once (ticket 0317).
//!
//! `vxn-1b/xtask`, `vxn-2/xtask` and `vxn-3/xtask` were the same tool three
//! times with three name sets — around fifteen near-verbatim functions, and the
//! newest fork carrying the best version of them. That is a bad shape for any
//! code and a worse one here: the bundler is where
//! [[vxn-windows-vst3-optref-strip]] happened, and a fix applied to one copy
//! was a fix absent from two.
//!
//! Everything a product cannot decide for itself is here. What stays in each
//! `main.rs` is its [`Product`] descriptor, its own subcommands (`web`,
//! `level-presets`), and its help text.
//!
//! # What is deliberately NOT flattened
//!
//! - **The Info.plist version.** It is `Product::version`, passed by each
//!   `main.rs` from its OWN `env!("CARGO_PKG_VERSION")`. Reading it here would
//!   stamp this crate's version into every bundle — vxn-1b ships 0.0.x and the
//!   others ride the workspace 0.1.x, so the plists would all silently become
//!   the wrong number.
//! - **The profile.** vxn-2 has a real debug path (`bundle` without
//!   `--release`); vxn-1b is release-only and 0311 deleted its no-op flag.
//!   [`Profile`] carries the difference rather than hard-coding one.
//! - **VST3 at all.** vxn-3 has no wrapper directory, so [`Product::vst3`] is
//!   `None` and its `--format vst3` fails with a reason instead of a stack of
//!   CMake errors.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// ── Product descriptor ──────────────────────────────────────────────────────

/// Everything the bundler needs to know about one synth.
#[derive(Clone, Copy)]
pub struct Product {
    /// The executable name inside a macOS bundle, and the CLAP bundle stem.
    pub plugin_name: &'static str,
    /// CLAP bundle filename, e.g. `vxn1b.clap` / `VXN2.clap`. Not derivable
    /// from `plugin_name`: the two products disagree about capitalisation and
    /// renaming either would orphan every installed copy.
    pub bundle_name: &'static str,
    /// `CFBundleIdentifier`, and the string the CI non-hollow check greps for.
    pub bundle_id: &'static str,
    /// `CFBundleName` — what a DAW's plugin list shows.
    pub display_name: &'static str,
    /// Crate name of the `*-clap` library, without the `lib` prefix or suffix.
    pub lib_name: &'static str,
    /// Cargo package to build.
    pub clap_package: &'static str,
    /// The product's own `CARGO_PKG_VERSION` — see the module docs.
    pub version: &'static str,
    /// `LSMinimumSystemVersion` for the bundle plist. Per-product because the
    /// three genuinely differ (vxn-1b declares 11.0.0, the other two 10.13.0),
    /// and quietly unifying them would change what hosts believe about two
    /// shipped products.
    pub min_macos: &'static str,
    /// VST3 support, or `None` for a CLAP-only product.
    pub vst3: Option<Vst3>,
    /// Directory (relative to the workspace root) staged into the macOS
    /// bundle's `Contents/Resources/`, or `None` for a product whose assets are
    /// only ever `include_str!`-embedded.
    ///
    /// vxn-2 uses it for dev hot-reload: with `VXN2_DEV_ASSETS=1` the editor
    /// reads CSS/JS from the bundle instead of its embed, so a designer can
    /// iterate without rebuilding the cdylib. Production never sets the var.
    /// **This field is why `Product` is not just names** — the first cut of
    /// 0317 dropped the staging, and the bundle still built and still loaded.
    pub resources_dir: Option<&'static str>,
}

/// The VST3 half of a [`Product`], for the ones that have one.
#[derive(Clone, Copy)]
pub struct Vst3 {
    /// VST3 bundle stem (`VXN1b` → `VXN1b.vst3`). Capitalised where the CLAP
    /// stem may not be; it is a FILENAME, not a display name.
    pub name: &'static str,
    /// Wrapper CMake project, relative to the workspace root.
    pub wrapper_dir: &'static str,
    /// CMake build directory under `target/`, e.g. `vxn1b-wrapper-release`.
    /// Carries the profile because vxn-2 builds both.
    pub build_dir_stem: &'static str,
}

// ── Profile ─────────────────────────────────────────────────────────────────

/// Cargo profile to build. vxn-2 exposes both; the others are release-only.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Profile {
    Debug,
    Release,
}

impl Profile {
    /// The `target/` subdirectory cargo writes this profile into.
    pub fn dir(self) -> &'static str {
        match self {
            Profile::Debug => "debug",
            Profile::Release => "release",
        }
    }

    /// CMake's spelling of this profile — both the `CMAKE_BUILD_TYPE` a
    /// single-config generator needs at configure time and the `--config` a
    /// multi-config one needs at build time.
    pub fn cmake_config(self) -> &'static str {
        match self {
            Profile::Debug => "Debug",
            Profile::Release => "Release",
        }
    }
}

// ── Formats ─────────────────────────────────────────────────────────────────

/// Output formats `bundle` can emit, selected by `--format`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Clap,
    Vst3,
}

/// Parse `--format clap,vst3`. Defaults to CLAP alone, dedupes, and preserves
/// the order given so `install` reports in the order asked for.
pub fn parse_formats(args: &[String]) -> Result<Vec<Format>, String> {
    let Some(raw) = arg_value(args, "--format") else {
        return Ok(vec![Format::Clap]);
    };
    let mut out = Vec::new();
    for part in raw.split(',') {
        let fmt = match part.trim().to_ascii_lowercase().as_str() {
            "clap" => Format::Clap,
            "vst3" => Format::Vst3,
            "" => continue,
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
    // `--format ,` is a typo, not a request for nothing: fall back to the
    // default rather than building zero artifacts and exiting 0.
    if out.is_empty() {
        return Ok(vec![Format::Clap]);
    }
    Ok(out)
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Clap => "clap",
            Format::Vst3 => "vst3",
        }
    }
}

/// Run `f` for each requested format, stopping at the first failure and naming
/// which leg failed. Formats are independent artifacts, so a CLAP that built
/// fine still stands when the VST3 leg fails.
pub fn run_formats(
    formats: &[Format],
    mut f: impl FnMut(Format) -> Result<(), String>,
) -> Result<(), String> {
    for &fmt in formats {
        f(fmt).map_err(|e| format!("{}: {e}", fmt.as_str()))?;
    }
    Ok(())
}

/// Value of a `--flag value` pair (e.g. `--format clap,vst3`).
pub fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

// ── Paths ───────────────────────────────────────────────────────────────────

/// The workspace root, from an xtask's `CARGO_MANIFEST_DIR`.
///
/// Every product's xtask sits at `<repo>/<product>/xtask`, so the root is two
/// levels up — but that is a property of the LAYOUT, not of this function, and
/// it has been wrong before ([[vxn2-xtask-flat-workspace]]). Each product's
/// `main.rs` asserts its own answer in a test rather than trusting this.
pub fn workspace_root(manifest_dir: &str) -> PathBuf {
    PathBuf::from(manifest_dir)
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| panic!("no workspace root two levels above {manifest_dir}"))
        .to_path_buf()
}

/// Where finished bundles are staged.
pub fn bundled_dir(root: &Path) -> PathBuf {
    root.join("target").join("bundled")
}

impl Product {
    /// Path to this product's shared library under a profile dir.
    pub fn lib_path(&self, profile_dir: &Path) -> PathBuf {
        let (prefix, ext) = if cfg!(target_os = "windows") {
            ("", "dll")
        } else if cfg!(target_os = "macos") {
            ("lib", "dylib")
        } else {
            ("lib", "so")
        };
        profile_dir.join(format!("{prefix}{}.{ext}", self.lib_name))
    }

    /// Path to this product's static archive under a profile dir — the `.a` /
    /// `.lib` analogue of [`Self::lib_path`], and what the wrapper force-loads.
    pub fn static_lib_path(&self, profile_dir: &Path) -> PathBuf {
        if cfg!(target_os = "windows") {
            profile_dir.join(format!("{}.lib", self.lib_name))
        } else {
            profile_dir.join(format!("lib{}.a", self.lib_name))
        }
    }

    /// Where a given format's installed artifact lives.
    pub fn install_path(&self, fmt: Format) -> Result<PathBuf, String> {
        Ok(match fmt {
            Format::Clap => clap_install_dir()?.join(self.bundle_name),
            Format::Vst3 => vst3_install_dir()?.join(format!("{}.vst3", self.vst3_or_err()?.name)),
        })
    }

    fn vst3_or_err(&self) -> Result<Vst3, String> {
        self.vst3.ok_or_else(|| {
            format!(
                "{} has no VST3 build — it is CLAP-only (no wrapper project)",
                self.display_name
            )
        })
    }
}

/// User plug-in directory for one format on this platform.
///
/// The two used to be the same three-branch HOME / LOCALAPPDATA / HOME shape
/// written twice, differing only in the joined suffix.
fn user_plugin_dir(macos: &str, windows: &str, unix: &str) -> Result<PathBuf, String> {
    if cfg!(target_os = "macos") {
        let home = env::var("HOME").map_err(|_| "HOME not set".to_string())?;
        Ok(PathBuf::from(home).join(macos))
    } else if cfg!(target_os = "windows") {
        // Per-user rather than the machine-wide `%CommonProgramFiles%` paths:
        // those need admin, and we install for the current user.
        let local = env::var("LOCALAPPDATA").map_err(|_| "LOCALAPPDATA not set".to_string())?;
        Ok(PathBuf::from(local).join(windows))
    } else {
        let home = env::var("HOME").map_err(|_| "HOME not set".to_string())?;
        Ok(PathBuf::from(home).join(unix))
    }
}

/// The user CLAP install directory for the host platform.
pub fn clap_install_dir() -> Result<PathBuf, String> {
    user_plugin_dir("Library/Audio/Plug-Ins/CLAP", r"Programs\Common\CLAP", ".clap")
}

/// The user VST3 install directory for the host platform.
pub fn vst3_install_dir() -> Result<PathBuf, String> {
    user_plugin_dir("Library/Audio/Plug-Ins/VST3", r"Programs\Common\VST3", ".vst3")
}

// ── Filesystem ──────────────────────────────────────────────────────────────

/// Attach a context string to an `io::Error`.
pub fn io(ctx: &'static str) -> impl Fn(std::io::Error) -> String {
    move |e| format!("{ctx}: {e}")
}

pub fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<(), String> {
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

/// Copy a staged artifact over whatever is at `dest`, bundle or bare file.
pub fn copy_artifact(src: &Path, dest: &Path) -> Result<(), String> {
    if src.is_dir() {
        let _ = fs::remove_dir_all(dest);
        copy_dir_recursive(src, dest)
    } else {
        let _ = fs::remove_file(dest);
        fs::copy(src, dest).map(|_| ()).map_err(io("install copy"))
    }
}

/// Recursively visit directories named `name` under `dir`, calling `f` on each.
/// Does not descend into a matched directory (a bundle is a leaf for our needs).
pub fn find_named_dirs(dir: &Path, name: &str, f: &mut impl FnMut(&Path)) {
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

// ── Cargo ───────────────────────────────────────────────────────────────────

const TRIPLES: [&str; 2] = ["aarch64-apple-darwin", "x86_64-apple-darwin"];

impl Product {
    /// `cargo build [--release] --package <clap_package>`, optionally for one
    /// `--target`. Emits the cdylib *and* the staticlib in one go (crate-type
    /// cdylib+rlib+staticlib), so both bundle paths share this.
    pub fn cargo_build(
        &self,
        root: &Path,
        profile: Profile,
        triple: Option<&str>,
    ) -> Result<(), String> {
        let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let mut build = Command::new(&cargo);
        build
            .current_dir(root)
            .args(["build", "--package", self.clap_package]);
        if profile == Profile::Release {
            build.arg("--release");
        }
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

    /// Build both macOS slices and `lipo` them into one fat artifact; returns
    /// its path.
    ///
    /// `pick` selects the cdylib or the staticlib out of each slice's profile
    /// dir, and `out_name` is the fat file's name. The static slices **must** be
    /// combined here rather than passed as two: the wrapper force-loads a single
    /// archive per link (see the products' `wrapper/CMakeLists.txt`).
    pub fn build_universal(
        &self,
        root: &Path,
        profile: Profile,
        pick: impl Fn(&Product, &Path) -> PathBuf,
        out_name: &str,
    ) -> Result<PathBuf, String> {
        let mut slices = Vec::new();
        for triple in TRIPLES {
            self.cargo_build(root, profile, Some(triple))?;
            let artifact = pick(
                self,
                &root.join("target").join(triple).join(profile.dir()),
            );
            if !artifact.exists() {
                return Err(format!(
                    "{triple} artifact not found at {}",
                    artifact.display()
                ));
            }
            slices.push(artifact);
        }

        let out_dir = root.join("target").join("universal").join(profile.dir());
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
}

// ── macOS bundle ────────────────────────────────────────────────────────────

impl Product {
    /// Assemble a `.clap` (or any BNDL) around `lib`.
    pub fn build_macos_bundle(&self, bundle_path: &Path, lib: &Path) -> Result<(), String> {
        let _ = fs::remove_dir_all(bundle_path);
        let macos_dir = bundle_path.join("Contents").join("MacOS");
        fs::create_dir_all(&macos_dir).map_err(io("create Contents/MacOS"))?;
        fs::copy(lib, macos_dir.join(self.plugin_name)).map_err(io("copy dylib into bundle"))?;
        fs::write(
            bundle_path.join("Contents").join("Info.plist"),
            self.info_plist(),
        )
        .map_err(io("write Info.plist"))?;
        fs::write(bundle_path.join("Contents").join("PkgInfo"), "BNDL????")
            .map_err(io("write PkgInfo"))?;
        Ok(())
    }

    /// Stage `resources_dir` into the bundle, if this product has one.
    ///
    /// Errors rather than skipping when the directory is missing: a silently
    /// empty `Resources/` is a dev-assets path that fails only when someone
    /// sets the env var, which is exactly when they are least able to explain
    /// it.
    fn stage_resources(&self, root: &Path, bundle_path: &Path) -> Result<(), String> {
        let Some(rel) = self.resources_dir else {
            return Ok(());
        };
        let src = root.join(rel);
        if !src.is_dir() {
            return Err(format!(
                "expected bundle resources at {}, but the directory is missing",
                src.display()
            ));
        }
        copy_dir_recursive(&src, &bundle_path.join("Contents").join("Resources"))
    }

    /// The bundle's `Info.plist`. `CFBundleExecutable` must match the file the
    /// bundle actually contains.
    pub fn info_plist(&self) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key><string>English</string>
    <key>CFBundleExecutable</key><string>{plugin}</string>
    <key>CFBundleIdentifier</key><string>{id}</string>
    <key>CFBundleName</key><string>{display}</string>
    <key>CFBundlePackageType</key><string>BNDL</string>
    <key>CFBundleVersion</key><string>{version}</string>
    <key>CFBundleShortVersionString</key><string>{version}</string>
    <key>CFBundleSupportedPlatforms</key>
    <array><string>MacOSX</string></array>
    <key>LSMinimumSystemVersion</key><string>{min_macos}</string>
</dict>
</plist>
"#,
            plugin = self.plugin_name,
            id = self.bundle_id,
            display = self.display_name,
            version = self.version,
            min_macos = self.min_macos,
        )
    }
}

// ── CLAP ────────────────────────────────────────────────────────────────────

impl Product {
    /// Build the CLAP and stage it in `target/bundled/`. Returns its path.
    pub fn bundle_clap(
        &self,
        root: &Path,
        profile: Profile,
        universal: bool,
    ) -> Result<PathBuf, String> {
        let lib = if universal {
            if !cfg!(target_os = "macos") {
                return Err("--universal is macOS-only".into());
            }
            self.build_universal(
                root,
                profile,
                Product::lib_path,
                &format!("lib{}.dylib", self.lib_name),
            )?
        } else {
            self.cargo_build(root, profile, None)?;
            let lib = self.lib_path(&root.join("target").join(profile.dir()));
            if !lib.exists() {
                return Err(format!(
                    "built library not found at {} (cross-compile target?)",
                    lib.display()
                ));
            }
            lib
        };

        let out_dir = bundled_dir(root);
        fs::create_dir_all(&out_dir).map_err(io("create bundled dir"))?;
        let clap_path = out_dir.join(self.bundle_name);

        if cfg!(target_os = "macos") {
            self.build_macos_bundle(&clap_path, &lib)?;
            self.stage_resources(root, &clap_path)?;
        } else {
            // Linux/Windows: a CLAP is just the shared library with a .clap name.
            let _ = fs::remove_file(&clap_path);
            fs::copy(&lib, &clap_path).map_err(io("copy library"))?;
        }
        Ok(clap_path)
    }
}

// ── VST3 (clap-wrapper) ─────────────────────────────────────────────────────

/// Error unless `cmake` is invokable, with an install hint.
pub fn ensure_cmake() -> Result<(), String> {
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
pub fn ensure_msvc() -> Result<(), String> {
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
/// opaquely.
pub fn ensure_submodules(root: &Path) -> Result<(), String> {
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
pub fn ninja_available() -> bool {
    Command::new("ninja").arg("--version").output().is_ok()
}

/// Find the staged `<name>.vst3` bundle. Prefer the copy our CMake stages into
/// `out_dir`; fall back to the newest match anywhere under the build tree
/// (multi-config generators can place it under a `Release/` subdir).
pub fn find_vst3(out_dir: &Path, build_dir: &Path, name: &str) -> Result<PathBuf, String> {
    let name = format!("{name}.vst3");
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

/// One wrapper CMake build, resolved: the paths it reads and writes plus the
/// two switches that change what it produces. A record rather than a seven-
/// argument pair of functions — `configure` and `build` need almost all of it,
/// and every field is a path or a flag that would otherwise thread through both
/// call sites in the same order.
struct WrapperBuild<'a> {
    root: &'a Path,
    vst3: Vst3,
    /// The Rust staticlib CMake whole-archives into the VST3 module.
    archive: PathBuf,
    /// Reused across runs — CMake decides what to rebuild, and removing this
    /// directory is what forces a clean one.
    build_dir: PathBuf,
    /// Where our CMake stages the finished bundle (`VXN_OUTPUT_DIR`).
    out_dir: PathBuf,
    profile: Profile,
    universal: bool,
}

impl WrapperBuild<'_> {
    fn configure(&self) -> Result<(), String> {
        let mut cfg = Command::new("cmake");
        cfg.current_dir(self.root)
            .arg("-S")
            .arg(self.vst3.wrapper_dir)
            .arg("-B")
            .arg(&self.build_dir)
            .arg(format!("-DVXN_CLAP_STATIC={}", self.archive.display()))
            .arg(format!(
                "-DVXN_CLAP_SDK_DIR={}",
                self.root.join("vendor/clap").display()
            ))
            .arg(format!(
                "-DVXN_VST3_SDK_DIR={}",
                self.root.join("vendor/vst3sdk").display()
            ))
            .arg(format!(
                "-DVXN_CLAP_WRAPPER_DIR={}",
                self.root.join("vendor/clap-wrapper").display()
            ))
            .arg(format!("-DVXN_OUTPUT_DIR={}", self.out_dir.display()));
        if self.universal {
            cfg.arg("-DCMAKE_OSX_ARCHITECTURES=arm64;x86_64");
        }
        // Ninja is single-config: without an explicit build type it defaults to
        // an empty/Debug config (the `--config Release` on the build step is
        // ignored), leaving the C++ side on the debug CRT while the Rust
        // staticlib is built `--release`. Pin the type here so both sides use
        // the same runtime; harmless on multi-config generators, which honour
        // `--config` instead.
        cfg.arg(format!("-DCMAKE_BUILD_TYPE={}", self.profile.cmake_config()));
        // Prefer Ninja when present (fast, single-config); otherwise the
        // platform default generator.
        if ninja_available() {
            cfg.arg("-G").arg("Ninja");
        }
        let status = cfg
            .status()
            .map_err(|e| format!("failed to run cmake configure: {e}"))?;
        if !status.success() {
            return Err("cmake configure failed (see output above)".into());
        }
        Ok(())
    }

    fn build(&self) -> Result<(), String> {
        let status = Command::new("cmake")
            .current_dir(self.root)
            .arg("--build")
            .arg(&self.build_dir)
            .arg("--parallel")
            .arg("--config")
            .arg(self.profile.cmake_config())
            .status()
            .map_err(|e| format!("failed to run cmake --build: {e}"))?;
        if !status.success() {
            return Err("cmake --build failed (see output above)".into());
        }
        Ok(())
    }
}

/// Reject the host/flag combinations the wrapper build cannot serve, then check
/// its three external prerequisites. Runs before anything is built so a missing
/// toolchain fails in a sentence rather than opaquely deep inside CMake or the
/// linker.
fn vst3_preflight(root: &Path, universal: bool) -> Result<(), String> {
    if !(cfg!(target_os = "macos") || cfg!(target_os = "windows")) {
        return Err("--format vst3 is supported on macOS and Windows only".into());
    }
    if universal && !cfg!(target_os = "macos") {
        return Err("--universal is macOS-only (omit it on Windows; the build is x86_64)".into());
    }
    ensure_cmake()?;
    ensure_msvc()?;
    ensure_submodules(root)
}

impl Product {
    /// Build the Rust staticlib CMake will whole-archive, and return its path.
    /// The cdylib comes out of the same invocation; only the archive is
    /// consumed here.
    fn build_vst3_archive(
        &self,
        root: &Path,
        profile: Profile,
        universal: bool,
    ) -> Result<PathBuf, String> {
        if universal {
            return self.build_universal(
                root,
                profile,
                Product::static_lib_path,
                &format!("lib{}.a", self.lib_name),
            );
        }
        self.cargo_build(root, profile, None)?;
        let a = self.static_lib_path(&root.join("target").join(profile.dir()));
        if !a.exists() {
            return Err(format!("built static archive not found at {}", a.display()));
        }
        Ok(a)
    }

    /// Build `<name>.vst3` by wrapping this product's clap staticlib through
    /// clap-wrapper, and stage it in `target/bundled/`. Returns its path.
    ///
    /// The engine, params, controller and faceplate are the same source as the
    /// CLAP; VST3 is purely a distribution artifact.
    ///
    /// Flow: preflight → build the staticlib slice(s) → configure + build the
    /// product's wrapper CMake project (whole-archives the archive into a VST3
    /// MODULE) → copy the staged bundle to `target/bundled/`. macOS (universal)
    /// and Windows (x86_64 MSVC); the wrapper CMake handles both bundle
    /// layouts.
    pub fn bundle_vst3(
        &self,
        root: &Path,
        profile: Profile,
        universal: bool,
    ) -> Result<PathBuf, String> {
        let vst3 = self.vst3_or_err()?;
        vst3_preflight(root, universal)?;

        let archive = self.build_vst3_archive(root, profile, universal)?;

        let build_dir = root.join("target").join(vst3.build_dir_stem);
        let out_dir = build_dir.join("out");
        fs::create_dir_all(&build_dir).map_err(io("create wrapper build dir"))?;
        let wrapper =
            WrapperBuild { root, vst3, archive, build_dir, out_dir, profile, universal };
        wrapper.configure()?;
        wrapper.build()?;

        // Locate the finished bundle. Our CMake stages it to VXN_OUTPUT_DIR,
        // but multi-config generators can also leave one under a `Release/`
        // subdir; find the newest match under the build tree to be
        // generator-proof.
        let staged = find_vst3(&wrapper.out_dir, &wrapper.build_dir, vst3.name)?;

        // Copy to target/bundled/ (mirrors the CLAP output location).
        let bundled = bundled_dir(root);
        fs::create_dir_all(&bundled).map_err(io("create bundled dir"))?;
        let dest = bundled.join(format!("{}.vst3", vst3.name));
        let _ = fs::remove_dir_all(&dest);
        copy_dir_recursive(&staged, &dest)?;
        Ok(dest)
    }

    /// Copy a staged artifact into its user plug-in directory.
    pub fn install_artifact(&self, fmt: Format, src: &Path) -> Result<(), String> {
        let dest = self.install_path(fmt)?;
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(io("create install dir"))?;
        }
        copy_artifact(src, &dest)?;
        println!("installed → {}", dest.display());
        Ok(())
    }

    /// Remove one installed format, reporting whether anything was there.
    pub fn uninstall(&self, fmt: Format) -> Result<(), String> {
        let dest = self.install_path(fmt)?;
        if !dest.exists() {
            println!("nothing installed at {}", dest.display());
            return Ok(());
        }
        if dest.is_dir() {
            fs::remove_dir_all(&dest).map_err(io("remove bundle"))?;
        } else {
            fs::remove_file(&dest).map_err(io("remove file"))?;
        }
        println!("removed {}", dest.display());
        Ok(())
    }
}

// ── Not here: the web bundlers ──────────────────────────────────────────────
//
// `build_wasm`, `serve_dist` and the `_headers` blob look like three more
// triplicated helpers and are not. vxn-1b serves through `node serve-coep.mjs`
// and vxn-2 through a python script; their `_headers` differ by a
// `Cross-Origin-Resource-Policy` line; their RUSTFLAGS handling differs; and
// vxn-3 has no web build at all. Unioning two different tools that share a verb
// buys a parameter for every difference and a shared home for neither. They
// stay in their products (0317).

#[cfg(test)]
mod tests {
    use super::*;

    const P: Product = Product {
        plugin_name: "vxnX",
        bundle_name: "vxnX.clap",
        bundle_id: "labs.vulpus.vxnX",
        display_name: "VXN-X",
        lib_name: "vxnX_clap",
        clap_package: "vxnX-clap",
        version: "9.9.9",
        min_macos: "11.0.0",
        resources_dir: None,
        vst3: Some(Vst3 {
            name: "VXNX",
            wrapper_dir: "vxn-X/wrapper",
            build_dir_stem: "vxnX-wrapper-release",
        }),
    };

    #[test]
    fn formats_default_to_clap_and_dedupe_in_order() {
        let f = |s: &str| parse_formats(&["--format".into(), s.into()]).unwrap();
        assert_eq!(parse_formats(&[]).unwrap(), vec![Format::Clap]);
        assert_eq!(f("vst3,clap"), vec![Format::Vst3, Format::Clap], "order kept");
        assert_eq!(f("clap,clap"), vec![Format::Clap], "deduped");
        assert_eq!(f(" VST3 "), vec![Format::Vst3], "trimmed, case-insensitive");
        assert!(parse_formats(&["--format".into(), "au".into()]).is_err());
        // A typo'd list is not a request to build nothing.
        assert_eq!(f(","), vec![Format::Clap]);
    }

    #[test]
    fn arg_value_reads_the_following_token_only() {
        let args: Vec<String> = ["--port", "9000", "--serve"].iter().map(|s| s.to_string()).collect();
        assert_eq!(arg_value(&args, "--port").as_deref(), Some("9000"));
        assert_eq!(arg_value(&args, "--serve"), None, "trailing flag has no value");
        assert_eq!(arg_value(&args, "--nope"), None);
    }

    /// The plist must name the executable the bundle actually contains, and
    /// carry the PRODUCT's version — not this crate's. Reading
    /// `env!("CARGO_PKG_VERSION")` here would stamp every bundle with the
    /// shared crate's number.
    #[test]
    fn the_plist_carries_the_products_own_identity() {
        let plist = P.info_plist();
        assert!(plist.contains("<key>CFBundleExecutable</key><string>vxnX</string>"));
        assert!(plist.contains("<key>CFBundleIdentifier</key><string>labs.vulpus.vxnX</string>"));
        assert!(plist.contains("<key>CFBundleName</key><string>VXN-X</string>"));
        assert!(plist.contains("<string>9.9.9</string>"), "the product's version");
        assert!(plist.contains("<string>11.0.0</string>"), "the product's macOS floor");
        assert!(!plist.contains(env!("CARGO_PKG_VERSION")), "not this crate's");
    }

    #[test]
    fn library_names_follow_the_platform() {
        let dir = Path::new("/t/target/release");
        let lib = P.lib_path(dir);
        let stat = P.static_lib_path(dir);
        if cfg!(target_os = "macos") {
            assert!(lib.ends_with("libvxnX_clap.dylib"));
            assert!(stat.ends_with("libvxnX_clap.a"));
        } else if cfg!(target_os = "windows") {
            assert!(lib.ends_with("vxnX_clap.dll"));
            assert!(stat.ends_with("vxnX_clap.lib"));
        } else {
            assert!(lib.ends_with("libvxnX_clap.so"));
            assert!(stat.ends_with("libvxnX_clap.a"));
        }
    }

    /// A CLAP-only product must say so, not fail somewhere inside CMake.
    #[test]
    fn a_clap_only_product_refuses_vst3_with_a_reason() {
        let clap_only = Product { vst3: None, ..P };
        let e = clap_only.install_path(Format::Vst3).unwrap_err();
        assert!(e.contains("CLAP-only"), "got: {e}");
        assert!(clap_only.install_path(Format::Clap).is_ok());
    }

    /// The error names the leg that failed — a `bundle --format clap,vst3`
    /// that fails must say which artifact did.
    #[test]
    fn run_formats_prefixes_the_failing_leg() {
        let e = run_formats(&[Format::Clap, Format::Vst3], |f| match f {
            Format::Clap => Ok(()),
            Format::Vst3 => Err("cmake exploded".into()),
        })
        .unwrap_err();
        assert_eq!(e, "vst3: cmake exploded");
    }

    /// A product that declares a resources dir must fail loudly when it is
    /// absent — the failure mode otherwise is an empty `Resources/` that only
    /// bites the person who sets the dev-assets env var.
    #[test]
    fn missing_bundle_resources_are_an_error_not_an_empty_dir() {
        let with_res = Product { resources_dir: Some("no/such/dir"), ..P };
        let e = with_res
            .stage_resources(Path::new("/definitely/not/here"), Path::new("/tmp/x.clap"))
            .unwrap_err();
        assert!(e.contains("directory is missing"), "got: {e}");
        // ...and a product without one stages nothing and succeeds.
        assert!(P.stage_resources(Path::new("/nope"), Path::new("/tmp/x.clap")).is_ok());
    }

    #[test]
    fn profile_dirs_are_cargos() {
        assert_eq!(Profile::Release.dir(), "release");
        assert_eq!(Profile::Debug.dir(), "debug");
    }

    /// Two levels up from `<repo>/<product>/xtask`. Wrong before
    /// ([[vxn2-xtask-flat-workspace]]), so each product asserts its own too.
    #[test]
    fn workspace_root_is_two_levels_above_an_xtask() {
        assert_eq!(workspace_root("/repo/vxn-2/xtask"), PathBuf::from("/repo"));
    }
}
