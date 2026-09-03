//! The VXN release runner.
//!
//! One release line: vxn-2 and VXN1b share a version, a bare-semver tag and a
//! release page, and every published release builds both products. This tool
//! drives that end to end — verify, bump, push, wait, tag, wait, deploy both
//! web bundles, repoint the site, check the result from the outside.
//!
//! The written process is [`RELEASING.md`](../../RELEASING.md); this is its
//! executable half. Where the two could drift, the doc points here.
//!
//! # Why a tool rather than a checklist
//!
//! The release is ten steps, two of which wait on GitHub for minutes, and the
//! last three of which are irreversible and outward-facing. A checklist gets
//! most of it right most of the time; what it does not do is refuse to tag when
//! CI is red, notice that the site checkout is four commits behind, or run the
//! one test suite CI does not have (vxn-2's web glue — see [`steps::verify`]).
//! All three of those have actually gone wrong.
//!
//! # Resumability
//!
//! `all` records each completed step in `target/vxn-release/ledger` and skips
//! it on a re-run. A failure in `site` therefore costs the `site` step, not a
//! second full `verify`. The ledger is keyed by version, so releasing a
//! different version starts clean.
//!
//! # Gates
//!
//! Every irreversible action prints what it is about to do and waits. `--yes`
//! skips those prompts; nothing else does, and the read-only steps never ask.

use std::fs;
use std::path::{Path, PathBuf};

mod plan;
mod sh;
mod steps;

use sh::Gate;

/// The repo root.
///
/// **Not** `vxn_xtask_common::workspace_root`: that walks up *two* levels,
/// which is right for the product xtasks at `<repo>/<product>/xtask` and wrong
/// for this one at `<repo>/xtask`. Borrowing it silently resolved to the
/// directory *above* the repo, where the first `git` call failed with
/// `not a git repository` — the same class of mistake its own doc comment warns
/// about, which is why it tells each xtask to assert its own answer in a test.
/// `root_of` is separated out so that test can exist without a checkout.
fn workspace_root() -> PathBuf {
    root_of(env!("CARGO_MANIFEST_DIR"))
}

fn root_of(manifest_dir: &str) -> PathBuf {
    PathBuf::from(manifest_dir)
        .parent()
        .unwrap_or_else(|| panic!("no workspace root one level above {manifest_dir}"))
        .to_path_buf()
}

/// Default location of the release notes for a version.
fn notes_path(root: &Path, version: &str) -> PathBuf {
    root.join(format!("release-notes/{version}.md"))
}

fn ledger_path(root: &Path) -> PathBuf {
    root.join("target/vxn-release/ledger")
}

/// Run `step` unless the ledger says it already completed for this version.
fn once(root: &Path, version: &str, name: &str, f: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    let path = ledger_path(root);
    let ledger = fs::read_to_string(&path).unwrap_or_default();
    if plan::ledger_done(&ledger, version, name) {
        println!("==> {name}: already done for {version}, skipping");
        return Ok(());
    }
    f()?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(vxn_xtask_common::io("ledger directory"))?;
    }
    fs::write(&path, plan::ledger_mark(&ledger, version, name))
        .map_err(vxn_xtask_common::io("ledger"))?;
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let gate = if args.iter().any(|a| a == "--yes") { Gate::Yes } else { Gate::Ask };
    let root = workspace_root();

    // The version argument, for the steps that take one. Positional so the
    // common case reads `cargo release all 0.3.0`.
    let version = args.get(1).filter(|a| !a.starts_with("--")).cloned();
    let notes = vxn_xtask_common::arg_value(&args, "--notes").map(PathBuf::from);

    let need_version = |what: &str| -> Result<String, String> {
        let v = version
            .clone()
            .ok_or_else(|| format!("`{what}` needs a version, e.g. `cargo release {what} 0.3.0`"))?;
        plan::validate_version(&v)?;
        Ok(v)
    };

    let result = match cmd {
        "preflight" => steps::preflight(&root),
        "verify" => steps::verify(&root),
        "bump" => need_version("bump").and_then(|v| steps::bump(&root, &v, gate)),
        "publish" => need_version("publish").and_then(|v| {
            let n = notes.clone().unwrap_or_else(|| notes_path(&root, &v));
            steps::publish(&root, &v, &n, gate)
        }),
        "web" => steps::web(&root),
        "site" => need_version("site").and_then(|v| steps::site(&v, gate)),
        "check" => need_version("check").and_then(|v| steps::check(&root, &v)),
        "all" => need_version("all").and_then(|v| {
            let n = notes.clone().unwrap_or_else(|| notes_path(&root, &v));
            // Ordered, and the order is load-bearing: nothing is pushed before
            // it is verified, nothing is tagged before CI agrees, and the site
            // is not repointed at a release whose assets have not attached.
            once(&root, &v, "preflight", || steps::preflight(&root))?;
            once(&root, &v, "verify", || steps::verify(&root))?;
            once(&root, &v, "bump", || steps::bump(&root, &v, gate))?;
            once(&root, &v, "publish", || steps::publish(&root, &v, &n, gate))?;
            once(&root, &v, "web", || steps::web(&root))?;
            once(&root, &v, "site", || steps::site(&v, gate))?;
            once(&root, &v, "check", || steps::check(&root, &v))?;
            println!(
                "\n==> VXN {v} is released.\n    \
                 https://github.com/Vulpus-Labs/vxn-1/releases/tag/{v}\n    \
                 https://vulpuslabs.com/products/vxn-1b/\n    \
                 https://vulpuslabs.com/products/vxn-2/"
            );
            Ok(())
        }),
        "--help" | "-h" | "help" => {
            print_help();
            return;
        }
        "" => {
            print_help();
            std::process::exit(2);
        }
        other => {
            eprintln!("release: unknown subcommand `{other}`");
            print_help();
            std::process::exit(2);
        }
    };

    if let Err(e) = result {
        eprintln!("\nrelease: {e}");
        std::process::exit(1);
    }
}

fn print_help() {
    println!(
        "cargo release <subcommand> [version]

Cuts a VXN release: both synths, one version, one tag, one release page.
The full process is documented in RELEASING.md.

Subcommands:
  preflight            clean tree, on main, synced; site pulled; tools present
  verify               web bundles, cargo test --workspace, BOTH node suites,
                       cargo bench --no-run
  bump <version>       set [workspace.package] version, refresh the lock, commit
  publish <version>    push, wait for CI, tag + release, wait for the builds,
                       check all 8 assets attached
  web                  rebuild both browser bundles into the site checkout
  site <version>       repoint both product pages, push the site (Netlify deploys)
  check <version>      verify from outside: asset URLs, COOP/COEP, live links
  all <version>        every step in order, resumable

Options:
  --yes                skip the confirmation before each irreversible action
  --notes <file>       release notes (default: release-notes/<version>.md)

Environment:
  SITE=<path>          the Hugo site checkout (default: ~/src/vulpus-labs-site)

Steps are independently runnable and re-runnable. `all` records progress in
target/vxn-release/ledger and skips what is already done, so a failure late in
a release does not cost the ten minutes of waiting that came before it."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    // The assertion `vxn_xtask_common::workspace_root`'s doc comment asks every
    // xtask to make for itself. This one is one level up, not two.
    #[test]
    fn the_root_is_one_level_above_this_crate() {
        assert_eq!(root_of("/repo/xtask"), PathBuf::from("/repo"));
        assert_ne!(
            root_of(env!("CARGO_MANIFEST_DIR")),
            vxn_xtask_common::workspace_root(env!("CARGO_MANIFEST_DIR")),
            "if these agree, this crate has moved and one of them is now wrong"
        );
    }

    // A real check against the checkout: the root is the directory holding the
    // workspace manifest and the release workflow this tool drives.
    #[test]
    fn the_root_holds_the_workspace_manifest() {
        let root = workspace_root();
        assert!(root.join("Cargo.toml").is_file(), "no Cargo.toml at {}", root.display());
        assert!(root.join(".github/workflows/release.yml").is_file());
        assert!(root.join("xtask/Cargo.toml").is_file());
    }
}
