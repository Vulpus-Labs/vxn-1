//! The decisions, separated from the doing.
//!
//! Everything in this module is a pure function over strings, so the parts of a
//! release that are easy to get quietly wrong — which section of `Cargo.toml`
//! the version lives in, which front-matter keys the site resolves downloads
//! against, whether a set of CI runs is actually green — are covered by
//! `cargo test --workspace` rather than by cutting a release and looking.

// ── Version ─────────────────────────────────────────────────────────────────

/// Accept `MAJOR.MINOR.PATCH`, digits only.
///
/// Deliberately narrower than semver: every tag this repo has ever cut is three
/// numbers, the release workflow keys off the bare tag, and a pre-release
/// suffix would need `--latest=false` handling that does not exist. Reject it
/// here rather than discover it after the tag is public.
pub fn validate_version(v: &str) -> Result<(), String> {
    let parts: Vec<&str> = v.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit())) {
        return Err(format!(
            "`{v}` is not a MAJOR.MINOR.PATCH version. The release workflow \
             triggers on the bare semver tag, so that is the only shape that works."
        ));
    }
    Ok(())
}

/// Rewrite the `version` under `[workspace.package]` and nowhere else.
///
/// Scoped to the section on purpose: `Cargo.toml` also carries a `version = `
/// inside `[workspace.dependencies]` entries in some workspaces, and a blind
/// first-match replace is exactly how a release ends up bumping a dependency
/// pin instead of the product.
pub fn bump_workspace_version(manifest: &str, to: &str) -> Result<String, String> {
    let mut out = String::with_capacity(manifest.len());
    let mut in_section = false;
    let mut replaced = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_section = trimmed == "[workspace.package]";
        }
        if in_section && !replaced && trimmed.starts_with("version") && trimmed.contains('=') {
            out.push_str(&format!("version = \"{to}\"\n"));
            replaced = true;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !replaced {
        return Err("no `version` key under [workspace.package] in Cargo.toml".into());
    }
    Ok(out)
}

/// Read the current `[workspace.package]` version back out.
pub fn current_version(manifest: &str) -> Option<String> {
    let mut in_section = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_section = trimmed == "[workspace.package]";
        }
        if in_section && trimmed.starts_with("version") {
            return trimmed.split('=').nth(1).map(|v| v.trim().trim_matches('"').to_string());
        }
    }
    None
}

// ── Site front matter ───────────────────────────────────────────────────────

/// Point a product page's TOML front matter at `version`.
///
/// Both keys move together and both must exist. `version` is display text;
/// `release` is the **git tag** the theme resolves download assets against
/// (`releases/download/<release>/<asset>`), so leaving it behind silently
/// serves the previous release's binaries under the new version's heading —
/// which is what VXN1b's page did while `release` still read `vxn-1b-0.2.0`.
pub fn repoint_front_matter(page: &str, version: &str) -> Result<String, String> {
    let mut out = String::with_capacity(page.len());
    let mut fences = 0usize;
    let (mut saw_version, mut saw_release) = (false, false);
    for line in page.lines() {
        if line.trim() == "+++" {
            fences += 1;
        }
        // Only inside the opening front-matter block: the body may quite
        // reasonably contain a line beginning "version".
        let in_front_matter = fences == 1;
        let trimmed = line.trim_start();
        if in_front_matter && trimmed.starts_with("version") && trimmed.contains('=') {
            out.push_str(&format!("version = \"{version}\"\n"));
            saw_version = true;
            continue;
        }
        if in_front_matter && trimmed.starts_with("release") && trimmed.contains('=') {
            out.push_str(&format!("release = \"{version}\"\n"));
            saw_release = true;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    match (saw_version, saw_release) {
        (true, true) => Ok(out),
        (false, _) => Err("product page has no `version` key in its front matter".into()),
        (_, false) => Err("product page has no `release` key in its front matter".into()),
    }
}

// ── Release assets ──────────────────────────────────────────────────────────

/// The eight assets a release must carry: two products x two platforms x two
/// formats. Hard-coded rather than derived from the workflow, so a job silently
/// dropping out of `release.yml` fails the check instead of shrinking it.
pub const EXPECTED_ASSETS: [&str; 8] = [
    "VXN1b-macOS-universal.clap.zip",
    "VXN1b-macOS-universal.vst3.zip",
    "VXN1b-windows-x64.clap",
    "VXN1b-windows-x64.vst3.zip",
    "VXN2-macOS-universal.clap.zip",
    "VXN2-macOS-universal.vst3.zip",
    "VXN2-windows-x64.clap",
    "VXN2-windows-x64.vst3.zip",
];

/// Which of [`EXPECTED_ASSETS`] are missing from `attached`.
pub fn missing_assets(attached: &str) -> Vec<&'static str> {
    let have: Vec<&str> = attached.lines().map(str::trim).collect();
    EXPECTED_ASSETS
        .iter()
        .copied()
        .filter(|want| !have.contains(want))
        .collect()
}

// ── CI status ───────────────────────────────────────────────────────────────

/// One workflow run, as `gh run list --jq` hands it over.
pub struct Run {
    pub name: String,
    pub status: String,
    pub conclusion: String,
}

/// Parse the tab-separated `name\tstatus\tconclusion` lines `gh --jq` emits.
pub fn parse_runs(out: &str) -> Vec<Run> {
    out.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut f = l.split('\t');
            Run {
                name: f.next().unwrap_or("").to_string(),
                status: f.next().unwrap_or("").to_string(),
                conclusion: f.next().unwrap_or("").to_string(),
            }
        })
        .collect()
}

/// What a batch of runs means. `None` while any run is still going.
///
/// An empty list is **not** success: it means GitHub has not registered the
/// push yet, and treating "no runs" as "nothing failed" would tag an untested
/// commit. That is the single most dangerous misreading available here, so it
/// gets its own arm.
pub enum CiVerdict {
    Pending,
    Green,
    Red(Vec<String>),
}

pub fn verdict(runs: &[Run], expect_at_least: usize) -> CiVerdict {
    if runs.len() < expect_at_least || runs.iter().any(|r| r.status != "completed") {
        return CiVerdict::Pending;
    }
    let failed: Vec<String> = runs
        .iter()
        .filter(|r| r.conclusion != "success")
        .map(|r| format!("{} → {}", r.name, r.conclusion))
        .collect();
    if failed.is_empty() {
        CiVerdict::Green
    } else {
        CiVerdict::Red(failed)
    }
}

// ── The resume ledger ───────────────────────────────────────────────────────

/// Steps completed so far, one name per line.
///
/// A release spends most of its wall-clock waiting on GitHub, so a failure in
/// the last step must not mean re-running the first. The ledger is keyed by
/// version: bumping to a different version starts a fresh run rather than
/// inheriting a half-finished one.
pub fn ledger_done(ledger: &str, version: &str, step: &str) -> bool {
    let key = format!("{version} {step}");
    ledger.lines().any(|l| l.trim() == key)
}

pub fn ledger_mark(ledger: &str, version: &str, step: &str) -> String {
    if ledger_done(ledger, version, step) {
        return ledger.to_string();
    }
    let mut out = ledger.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!("{version} {step}\n"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_shape_is_three_numbers() {
        assert!(validate_version("0.3.0").is_ok());
        assert!(validate_version("10.0.11").is_ok());
        for bad in ["0.3", "0.3.0.1", "v0.3.0", "0.3.0-rc1", "", "a.b.c"] {
            assert!(validate_version(bad).is_err(), "accepted `{bad}`");
        }
    }

    // The bump must not wander out of [workspace.package]. A dependency pin
    // that happens to sort earlier in the file is the trap.
    #[test]
    fn bump_only_touches_the_workspace_package_section() {
        let manifest = "\
[workspace]
members = [\"a\"]

[workspace.package]
version = \"0.2.0\"
edition = \"2024\"

[workspace.dependencies]
serde = { version = \"1.0.0\" }
";
        let out = bump_workspace_version(manifest, "0.3.0").unwrap();
        assert!(out.contains("version = \"0.3.0\""));
        assert!(out.contains("serde = { version = \"1.0.0\" }"), "dependency pin moved");
        assert_eq!(out.matches("0.3.0").count(), 1);
    }

    #[test]
    fn bump_fails_loudly_when_the_section_is_missing() {
        assert!(bump_workspace_version("[workspace]\nmembers = []\n", "0.3.0").is_err());
    }

    #[test]
    fn current_version_round_trips_a_bump() {
        let m = "[workspace.package]\nversion = \"0.2.0\"\n";
        assert_eq!(current_version(m).as_deref(), Some("0.2.0"));
        let bumped = bump_workspace_version(m, "0.4.1").unwrap();
        assert_eq!(current_version(&bumped).as_deref(), Some("0.4.1"));
    }

    // `release` is the tag downloads resolve against. VXN1b's page carried
    // `vxn-1b-0.2.0` there long after the two products merged onto one tag, so
    // this asserts BOTH keys move, not just the visible one.
    #[test]
    fn front_matter_repoints_version_and_release_together() {
        let page = "\
+++
title = \"VXN1b\"
version = \"0.2.0\"
release = \"vxn-1b-0.2.0\"
formats = [\"CLAP\"]
+++

Body text mentioning a release, and a version, in prose.
";
        let out = repoint_front_matter(page, "0.3.0").unwrap();
        assert!(out.contains("version = \"0.3.0\""));
        assert!(out.contains("release = \"0.3.0\""));
        assert!(!out.contains("vxn-1b-0.2.0"));
        // The body is not front matter and must be left alone.
        assert!(out.contains("Body text mentioning a release, and a version, in prose."));
    }

    #[test]
    fn front_matter_missing_a_key_is_an_error_not_a_silent_pass() {
        let no_release = "+++\nversion = \"0.2.0\"\n+++\n";
        assert!(repoint_front_matter(no_release, "0.3.0").is_err());
        let no_version = "+++\nrelease = \"0.2.0\"\n+++\n";
        assert!(repoint_front_matter(no_version, "0.3.0").is_err());
    }

    #[test]
    fn missing_assets_names_exactly_what_did_not_attach() {
        let all = EXPECTED_ASSETS.join("\n");
        assert!(missing_assets(&all).is_empty());
        let short = EXPECTED_ASSETS[..6].join("\n");
        assert_eq!(
            missing_assets(&short),
            vec!["VXN2-windows-x64.clap", "VXN2-windows-x64.vst3.zip"]
        );
        assert_eq!(missing_assets("").len(), 8);
    }

    // No runs yet is PENDING, never green: GitHub takes a moment to register a
    // push, and reading that gap as success tags an untested commit.
    #[test]
    fn no_runs_yet_is_pending_not_green() {
        assert!(matches!(verdict(&[], 3), CiVerdict::Pending));
        let one = parse_runs("Test\tcompleted\tsuccess");
        assert!(matches!(verdict(&one, 3), CiVerdict::Pending));
    }

    #[test]
    fn a_run_still_going_holds_the_verdict() {
        let runs = parse_runs(
            "Test\tcompleted\tsuccess\nBundle\tin_progress\t\nBuild Windows\tcompleted\tsuccess",
        );
        assert!(matches!(verdict(&runs, 3), CiVerdict::Pending));
    }

    #[test]
    fn a_failed_run_is_named_in_the_verdict() {
        let runs = parse_runs(
            "Test\tcompleted\tfailure\nBundle\tcompleted\tsuccess\nBuild Windows\tcompleted\tsuccess",
        );
        match verdict(&runs, 3) {
            CiVerdict::Red(f) => assert_eq!(f, vec!["Test → failure"]),
            _ => panic!("a failed run did not read as red"),
        }
    }

    #[test]
    fn all_green_is_green() {
        let runs = parse_runs(
            "Test\tcompleted\tsuccess\nBundle\tcompleted\tsuccess\nBuild Windows\tcompleted\tsuccess",
        );
        assert!(matches!(verdict(&runs, 3), CiVerdict::Green));
    }

    // A cancelled or skipped run is not a success. Only "success" is.
    #[test]
    fn cancelled_does_not_count_as_success() {
        let runs = parse_runs("Test\tcompleted\tcancelled\nBundle\tcompleted\tsuccess");
        assert!(matches!(verdict(&runs, 2), CiVerdict::Red(_)));
    }

    #[test]
    fn ledger_is_keyed_by_version_so_a_new_release_starts_clean() {
        let l = ledger_mark("", "0.3.0", "verify");
        assert!(ledger_done(&l, "0.3.0", "verify"));
        assert!(!ledger_done(&l, "0.3.0", "publish"));
        assert!(!ledger_done(&l, "0.4.0", "verify"), "ledger leaked across versions");
    }

    #[test]
    fn marking_twice_does_not_duplicate() {
        let l = ledger_mark("", "0.3.0", "verify");
        assert_eq!(ledger_mark(&l, "0.3.0", "verify"), l);
    }
}
