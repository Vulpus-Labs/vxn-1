//! The seven steps of a release, in the order they must run.
//!
//! Each is independently runnable and independently re-runnable: the ledger in
//! [`crate::plan`] records what has completed, and `all` skips those. That is
//! not a nicety — `publish` alone spends ten minutes waiting on GitHub, and a
//! failure in `site` must not cost a second full verify.

use std::fs;
use std::path::{Path, PathBuf};

use crate::plan::{self, CiVerdict};
use crate::sh::{self, Gate};

/// The two product pages whose front matter resolves download URLs.
const PRODUCT_PAGES: [&str; 2] = [
    "content/products/vxn-1b/index.md",
    "content/products/vxn-2/index.md",
];

/// The three workflows a push to `main` triggers: Test, Bundle, Build Windows.
/// `publish` will not tag until all three are green, and will not mistake "none
/// have registered yet" for that.
const PUSH_WORKFLOWS: usize = 3;

/// Where the Hugo site lives. `SITE=` overrides, matching `deploy-web.sh`.
pub fn site_dir() -> PathBuf {
    std::env::var("SITE").map(PathBuf::from).unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(home).join("src/vulpus-labs-site")
    })
}

// ── 1. preflight ────────────────────────────────────────────────────────────

/// Everything that must be true before a release starts, checked while it is
/// still free to stop.
///
/// This also **pulls the site**, which is not merely tidiness: the site
/// checkout was four commits behind origin when 0.3.0 was cut, and a
/// `deploy-web.sh` run on a stale checkout pushes a merge or fails outright
/// after the release is already public.
pub fn preflight(root: &Path) -> Result<(), String> {
    println!("==> preflight");
    let site = site_dir();

    for tool in ["git", "gh", "node", "npm", "rsync", "curl"] {
        if !sh::have(tool) {
            return Err(format!("preflight: `{tool}` is not on PATH"));
        }
    }
    if !sh::try_capture(root, "gh", &["auth", "status"]).0 {
        return Err("preflight: `gh` is not authenticated. Run: gh auth login".into());
    }
    sh::ensure_wasm_target()?;
    println!("    tools ok");

    // ── the monorepo ──
    let branch = sh::capture("preflight", root, "git", &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch != "main" {
        return Err(format!(
            "preflight: on branch `{branch}`. Releases are cut from `main` — this repo has no \
             release branches."
        ));
    }
    let dirty = sh::capture("preflight", root, "git", &["status", "--porcelain"])?;
    if !dirty.is_empty() {
        return Err(format!(
            "preflight: the working tree is not clean. A release tags HEAD, so uncommitted work \
             would ship or be lost:\n{dirty}"
        ));
    }
    sh::run("preflight", root, "git", &["fetch", "--tags", "--quiet"])?;
    let counts = sh::capture(
        "preflight",
        root,
        "git",
        &["rev-list", "--left-right", "--count", "origin/main...HEAD"],
    )?;
    if counts.split_whitespace().next() != Some("0") {
        return Err(format!(
            "preflight: `main` is behind origin ({counts} behind/ahead). Pull first — releasing \
             from a stale checkout tags a commit that is not the tip."
        ));
    }
    println!("    repo: on main, clean, synced with origin");

    // ── the site ──
    if !site.join(".git").is_dir() {
        return Err(format!(
            "preflight: {} is not a git checkout. Clone it, or set SITE= to point elsewhere.",
            site.display()
        ));
    }
    let site_dirty = sh::capture("preflight", &site, "git", &["status", "--porcelain"])?;
    if !site_dirty.is_empty() {
        return Err(format!(
            "preflight: the site checkout has uncommitted work. The deploy steps commit into it, \
             so clear this first:\n{site_dirty}"
        ));
    }
    sh::run("preflight", &site, "git", &["pull", "--ff-only", "--quiet"])?;
    println!("    site: clean, pulled ({})", site.display());

    // vxn-1b's Vitest suite is shelled by a cargo test, and `npm ci` inside
    // that test is not something cargo will do for us.
    let assets = root.join("vxn-1b/crates/vxn1b-ui-web/assets");
    if !assets.join("node_modules").is_dir() {
        println!("    installing vxn-1b JS deps (npm ci)");
        sh::run("preflight", &assets, "npm", &["ci"])?;
    }
    println!("    js deps ok");
    Ok(())
}

// ── 2. verify ───────────────────────────────────────────────────────────────

/// Everything CI runs, plus the leg CI does not.
///
/// **vxn-2's `web/*.test.mjs` suites are not in `test.yml`.** That gap is what
/// let 0345 ship: vxn-1b's identical suite went red on `main` and was found the
/// same afternoon, while vxn-2's dropped `scale_curve` silently. Until the
/// workflow grows that leg, this is the only thing that runs it, so it is not
/// optional here.
pub fn verify(root: &Path) -> Result<(), String> {
    println!("==> verify");
    let env = sh::rustup_env()?;
    let envs: Vec<(&str, String)> = env.clone();

    // The node suites load the real wasm and FAIL rather than skip when it is
    // missing (0295), so both bundles have to exist first.
    println!("    building web bundles (both ports)");
    sh::run_env(
        "verify",
        root,
        "cargo",
        &["run", "--quiet", "--package", "vxn1b-xtask", "--", "web"],
        &envs,
    )?;
    sh::run_env("verify", &root.join("vxn-2"), "cargo", &["xtask", "web"], &envs)?;

    println!("    cargo test --workspace (VXN_JS_TESTS=1)");
    let mut test_env = envs.clone();
    test_env.push(("VXN_JS_TESTS", "1".to_string()));
    sh::run_env("verify", root, "cargo", &["test", "--workspace"], &test_env)?;

    // A shell glob, because the suites are a directory of files and the list
    // is the shell's job rather than a hard-coded array that would go stale.
    for (label, glob) in [
        ("vxn-1b", "vxn-1b/crates/vxn1b-wasm/web/*.test.mjs"),
        ("vxn-2", "vxn-2/crates/vxn2-wasm/web/*.test.mjs"),
    ] {
        println!("    node --test {label} web suite");
        sh::run("verify", root, "sh", &["-c", &format!("node --test {glob}")])?;
    }

    println!("    cargo bench --no-run --workspace");
    sh::run_env("verify", root, "cargo", &["bench", "--no-run", "--workspace"], &envs)?;
    println!("    verify: green");
    Ok(())
}

// ── 3. bump ─────────────────────────────────────────────────────────────────

/// Set the workspace version and commit it.
///
/// Idempotent: if `Cargo.toml` already reads `version`, this reports and
/// returns rather than making an empty commit, so a resumed run is harmless.
pub fn bump(root: &Path, version: &str, gate: Gate) -> Result<(), String> {
    println!("==> bump → {version}");
    let manifest_path = root.join("Cargo.toml");
    let manifest = fs::read_to_string(&manifest_path).map_err(vxn_xtask_common::io("Cargo.toml"))?;
    let current = plan::current_version(&manifest).unwrap_or_default();

    if current == version {
        println!("    already {version}");
    } else {
        let bumped = plan::bump_workspace_version(&manifest, version)?;
        fs::write(&manifest_path, bumped).map_err(vxn_xtask_common::io("Cargo.toml"))?;
        println!("    {current} → {version}");
    }

    // `cargo check` is what rewrites Cargo.lock's 30-odd workspace entries.
    // Committing without it leaves the lock a version behind, and the next
    // build silently makes it dirty again.
    println!("    refreshing Cargo.lock");
    sh::run("bump", root, "cargo", &["check", "--workspace", "--quiet"])?;

    let staged = sh::capture("bump", root, "git", &["status", "--porcelain"])?;
    if staged.is_empty() {
        println!("    nothing to commit");
        return Ok(());
    }
    gate.confirm(
        &format!("bump: commit the version change to {version}"),
        &format!("in {}\n{staged}", root.display()),
    )?;
    sh::run("bump", root, "git", &["add", "Cargo.toml", "Cargo.lock"])?;
    sh::run(
        "bump",
        root,
        "git",
        &["commit", "-q", "-m", &format!("chore(release): {version}")],
    )?;
    Ok(())
}

// ── 4. publish ──────────────────────────────────────────────────────────────

/// Push, wait for CI, tag, wait for the builds, check the assets.
///
/// The two waits are the reason this tool exists. Tagging before CI is green
/// publishes a release page whose binaries then fail to build, and the release
/// page is the thing the site links to.
pub fn publish(root: &Path, version: &str, notes: &Path, gate: Gate) -> Result<(), String> {
    println!("==> publish {version}");
    if !notes.is_file() {
        return Err(format!(
            "publish: no release notes at {}. Write them first — `gh release create` will \
             otherwise publish an empty release page.",
            notes.display()
        ));
    }

    // ── push ──
    let ahead = sh::capture(
        "publish",
        root,
        "git",
        &["log", "--oneline", "origin/main..HEAD"],
    )?;
    if ahead.is_empty() {
        println!("    nothing to push");
    } else {
        gate.confirm(
            "publish: push to origin/main",
            &format!("{}\n\nthis is public, and starts CI", ahead),
        )?;
        sh::run("publish", root, "git", &["push", "origin", "main"])?;
    }

    // ── wait for CI on this exact commit ──
    let sha = sh::capture("publish", root, "git", &["rev-parse", "HEAD"])?;
    println!("    waiting for CI on {}", &sha[..8]);
    let jq = format!(
        r#".[] | select(.headSha=="{sha}") | "\(.name)\t\(.status)\t\(.conclusion)""#
    );
    sh::poll_until("CI", 30, 30 * 60, || {
        let out = sh::capture(
            "publish",
            root,
            "gh",
            &["run", "list", "--limit", "20", "--json", "headSha,name,status,conclusion", "--jq", &jq],
        )?;
        match plan::verdict(&plan::parse_runs(&out), PUSH_WORKFLOWS) {
            CiVerdict::Pending => Ok(None),
            CiVerdict::Green => Ok(Some("green".into())),
            CiVerdict::Red(f) => Err(format!(
                "publish: CI is red on {}, not tagging:\n    {}",
                &sha[..8],
                f.join("\n    ")
            )),
        }
    })?;
    println!("    CI green");

    // ── tag + release ──
    let (tag_exists, _) = sh::try_capture(root, "gh", &["release", "view", version]);
    if tag_exists {
        println!("    release {version} already exists");
    } else {
        let preview: String = fs::read_to_string(notes)
            .map_err(vxn_xtask_common::io("release notes"))?
            .lines()
            .take(3)
            .collect::<Vec<_>>()
            .join("\n");
        gate.confirm(
            &format!("publish: create the public release {version}"),
            &format!(
                "tag {version} on {} (marked `latest`)\nnotes from {}:\n{preview}\n...",
                &sha[..8],
                notes.display()
            ),
        )?;
        sh::run(
            "publish",
            root,
            "gh",
            &[
                "release",
                "create",
                version,
                "--title",
                &format!("VXN {version}"),
                "--notes-file",
                &notes.to_string_lossy(),
                "--latest",
            ],
        )?;
    }

    // ── wait for the four build jobs, then count the assets ──
    println!("    waiting for the release builds");
    let jq = format!(
        r#".[] | select(.headBranch=="{version}") | "\(.name)\t\(.status)\t\(.conclusion)""#
    );
    sh::poll_until("release builds", 30, 30 * 60, || {
        let out = sh::capture(
            "publish",
            root,
            "gh",
            &["run", "list", "--workflow=release.yml", "--limit", "10", "--json",
              "headBranch,name,status,conclusion", "--jq", &jq],
        )?;
        match plan::verdict(&plan::parse_runs(&out), 1) {
            CiVerdict::Pending => Ok(None),
            CiVerdict::Green => Ok(Some("built".into())),
            CiVerdict::Red(f) => Err(format!(
                "publish: the release build failed:\n    {}\n\nThe release page exists but is \
                 missing binaries. Fix, then re-run: gh run rerun <id>",
                f.join("\n    ")
            )),
        }
    })?;

    let attached = sh::capture(
        "publish",
        root,
        "gh",
        &["release", "view", version, "--json", "assets", "--jq", ".assets[].name"],
    )?;
    let missing = plan::missing_assets(&attached);
    if !missing.is_empty() {
        return Err(format!(
            "publish: the release is missing {} asset(s):\n    {}\n\nA VST3 whose staticlib was \
             not force-loaded still links and still produces a plausible bundle, so the workflow's \
             non-hollow check is what usually catches this. Read the job log before re-running.",
            missing.len(),
            missing.join("\n    ")
        ));
    }
    println!("    all {} assets attached", plan::EXPECTED_ASSETS.len());
    Ok(())
}

// ── 5. web ──────────────────────────────────────────────────────────────────

/// Rebuild both browser bundles and mirror them into the site checkout.
///
/// Delegates to each port's `deploy-web.sh` rather than reimplementing them:
/// those scripts own the `_headers` append (COOP/COEP per subpath, appended
/// never rewritten — rewriting takes the other synths off the air) and the
/// `rsync --delete` mirror. `NO_PUSH=1` because the site is pushed once, by
/// [`site`], after the front matter moves too.
pub fn web(root: &Path) -> Result<(), String> {
    println!("==> web");
    let mut envs = sh::rustup_env()?;
    envs.push(("NO_PUSH", "1".to_string()));
    for script in [
        "vxn-1b/crates/vxn1b-wasm/deploy-web.sh",
        "vxn-2/crates/vxn2-wasm/deploy-web.sh",
    ] {
        println!("    {script}");
        sh::run_env("web", root, &root.join(script).to_string_lossy(), &[], &envs)?;
    }
    Ok(())
}

// ── 6. site ─────────────────────────────────────────────────────────────────

/// Repoint both product pages at the new release, then push the site.
///
/// This is the step that actually changes what a visitor downloads, and it is
/// the last irreversible one.
pub fn site(version: &str, gate: Gate) -> Result<(), String> {
    println!("==> site → {version}");
    let site = site_dir();
    for page in PRODUCT_PAGES {
        let path = site.join(page);
        let text = fs::read_to_string(&path).map_err(vxn_xtask_common::io("product page"))?;
        let out = plan::repoint_front_matter(&text, version)
            .map_err(|e| format!("site: {page}: {e}"))?;
        if out != text {
            fs::write(&path, out).map_err(vxn_xtask_common::io("product page"))?;
            println!("    {page} → {version}");
        } else {
            println!("    {page} already {version}");
        }
    }

    // Hugo is not a build dependency of this repo, but if it is installed a
    // failed render here is far cheaper than one on Netlify.
    if sh::have("hugo") {
        println!("    hugo build check");
        sh::run("site", &site, "hugo", &["--quiet"])?;
    }

    let pending = sh::capture("site", &site, "git", &["status", "--porcelain"])?;
    if !pending.is_empty() {
        sh::run("site", &site, "git", &["add", PRODUCT_PAGES[0], PRODUCT_PAGES[1]])?;
        sh::run(
            "site",
            &site,
            "git",
            &["commit", "-q", "-m", &format!("Point both product pages at the {version} release")],
        )?;
    }

    let ahead = sh::capture("site", &site, "git", &["log", "--oneline", "origin/main..HEAD"])?;
    if ahead.is_empty() {
        println!("    site already deployed");
        return Ok(());
    }
    gate.confirm(
        "site: push to the live site",
        &format!(
            "{ahead}\n\nNetlify deploys from main, so this push IS the deploy — the new bundles \
             and download links go live immediately."
        ),
    )?;
    sh::run("site", &site, "git", &["push", "origin", "main"])?;
    Ok(())
}

// ── 7. check ────────────────────────────────────────────────────────────────

/// Verify the released thing from the outside: assets, headers, live links.
///
/// Deliberately hits the public URLs rather than the local checkout. Every
/// failure mode this catches — a missing asset, a lost `_headers` block, a page
/// still pointing at the previous tag — looks completely fine locally.
pub fn check(root: &Path, version: &str) -> Result<(), String> {
    println!("==> check {version}");
    let mut bad = Vec::new();

    for asset in plan::EXPECTED_ASSETS {
        let url = format!(
            "https://github.com/Vulpus-Labs/vxn-1/releases/download/{version}/{asset}"
        );
        let code = http_code(root, &url)?;
        println!("    {code}  {asset}");
        if code != "200" {
            bad.push(format!("asset {asset} → HTTP {code}"));
        }
    }

    for (label, path) in [
        ("VXN1b", "products/vxn-1b/web/"),
        ("VXN2", "products/vxn-2/web/"),
    ] {
        let url = format!("https://vulpuslabs.com/{path}");
        let headers = sh::capture("check", root, "curl", &["-sI", &url])?.to_lowercase();
        let isolated = headers.contains("cross-origin-opener-policy: same-origin")
            && headers.contains("cross-origin-embedder-policy: require-corp");
        println!("    {} {label} web (cross-origin isolated: {isolated})",
            if headers.contains("200") { "200 " } else { "??? " });
        if !isolated {
            // Without these the page loads and then fails to construct a
            // SharedArrayBuffer, which reads to a player as "the synth is
            // broken" rather than as a missing header.
            bad.push(format!(
                "{label} web is not cross-origin isolated — SharedArrayBuffer will fail"
            ));
        }
    }

    for (label, path) in [("VXN1b", "products/vxn-1b/"), ("VXN2", "products/vxn-2/")] {
        let url = format!("https://vulpuslabs.com/{path}");
        let body = sh::capture("check", root, "curl", &["-s", &url])?;
        let needle = format!("download/{version}/");
        let ok = body.contains(&needle);
        println!("    {} {label} product page points at {version}", if ok { "ok " } else { "BAD" });
        if !ok {
            bad.push(format!("{label} product page does not link {needle}"));
        }
    }

    if bad.is_empty() {
        println!("    check: all green");
        Ok(())
    } else {
        Err(format!("check failed:\n    {}", bad.join("\n    ")))
    }
}

fn http_code(root: &Path, url: &str) -> Result<String, String> {
    sh::capture(
        "check",
        root,
        "curl",
        &["-sIL", "-o", "/dev/null", "-w", "%{http_code}", url],
    )
}
