//! Subprocess plumbing, the wasm toolchain fix, and the confirmation gate.
//!
//! Everything here is I/O; the decision logic that can be tested without a
//! network or a checkout lives in [`crate::plan`].

use std::env;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Run a command, streaming its output, and fail with a readable message.
///
/// `ctx` names the step in the error, because a bare "exit status 101" three
/// screens below a cargo dump is not a diagnosis.
pub fn run(ctx: &str, dir: &Path, program: &str, args: &[&str]) -> Result<(), String> {
    run_env(ctx, dir, program, args, &[])
}

/// [`run`], plus environment variables for this child only.
pub fn run_env(
    ctx: &str,
    dir: &Path,
    program: &str,
    args: &[&str],
    envs: &[(&str, String)],
) -> Result<(), String> {
    let mut cmd = Command::new(program);
    cmd.current_dir(dir).args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let status = cmd
        .status()
        .map_err(|e| format!("{ctx}: could not spawn `{program}`: {e}"))?;
    if !status.success() {
        return Err(format!("{ctx}: `{program}` exited {status}"));
    }
    Ok(())
}

/// Run a command and capture stdout, trimmed. Stderr passes through.
///
/// A non-zero exit is an error rather than empty output: `gh` in particular
/// prints nothing and exits 1 when a run does not exist, and treating that as
/// "no runs are in flight" would sail straight past a failed CI check.
pub fn capture(ctx: &str, dir: &Path, program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .current_dir(dir)
        .args(args)
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("{ctx}: could not spawn `{program}`: {e}"))?;
    if !out.status.success() {
        return Err(format!("{ctx}: `{program}` exited {}", out.status));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Capture stdout and tolerate a non-zero exit, returning `(ok, stdout)`.
/// For the probes where "this command failed" is itself the answer.
pub fn try_capture(dir: &Path, program: &str, args: &[&str]) -> (bool, String) {
    match Command::new(program)
        .current_dir(dir)
        .args(args)
        .stderr(Stdio::null())
        .output()
    {
        Ok(out) => (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        ),
        Err(_) => (false, String::new()),
    }
}

/// Is `program` on PATH at all?
pub fn have(program: &str) -> bool {
    try_capture(Path::new("."), "which", &[program]).0
}

// ── The wasm toolchain fix ──────────────────────────────────────────────────

/// Environment that forces cargo onto the **rustup** toolchain.
///
/// This machine (and any like it) has Homebrew rust on PATH ahead of rustup.
/// Brew's `rustc` is host-only, and cargo shells out to a bare `rustc` resolved
/// through PATH — so `--target wasm32-unknown-unknown` fails with
/// `error[E0463]: can't find crate for 'std'` even when
/// `rustup target add wasm32-unknown-unknown` has been run, because the target
/// was added to a toolchain cargo is not using.
///
/// `rustup which rustc` resolves through `rust-toolchain.toml`, so this picks
/// up the repo's pinned channel rather than hard-coding a triple that would rot
/// the first time the pin moves or someone runs this on Intel.
///
/// Returned rather than exported process-wide: only the wasm builds need it,
/// and mutating this process's PATH would apply it to everything downstream
/// including commands where brew's rustc is perfectly fine.
pub fn rustup_env() -> Result<Vec<(&'static str, String)>, String> {
    if !have("rustup") {
        return Err("rustup is not on PATH. The wasm32 builds need it — a \
                    Homebrew rust is host-only. Install from https://rustup.rs."
            .into());
    }
    let rustc = capture("toolchain", Path::new("."), "rustup", &["which", "rustc"])?;
    let bin = PathBuf::from(&rustc)
        .parent()
        .ok_or_else(|| format!("toolchain: `rustup which rustc` gave `{rustc}`, which has no parent directory"))?
        .to_path_buf();
    let path = match env::var_os("PATH") {
        Some(p) => format!("{}:{}", bin.display(), p.to_string_lossy()),
        None => bin.display().to_string(),
    };
    Ok(vec![("RUSTC", rustc), ("PATH", path)])
}

/// Fail unless the rustup toolchain can actually build for wasm32.
pub fn ensure_wasm_target() -> Result<(), String> {
    let installed = capture(
        "toolchain",
        Path::new("."),
        "rustup",
        &["target", "list", "--installed"],
    )?;
    if installed.lines().any(|l| l.trim() == "wasm32-unknown-unknown") {
        return Ok(());
    }
    Err("the rustup toolchain has no wasm32-unknown-unknown target — both web \
         bundles need it. Run: rustup target add wasm32-unknown-unknown"
        .into())
}

// ── The confirmation gate ───────────────────────────────────────────────────

/// Ask before an irreversible action. `Ok(())` to proceed, `Err` to abort.
///
/// `detail` is printed verbatim above the prompt, so callers spell out exactly
/// what is about to leave this machine — a push, a public release, a site
/// deploy. `--yes` (see [`Gate::Yes`]) skips the prompt for an unattended run;
/// nothing else does.
#[derive(Clone, Copy, PartialEq)]
pub enum Gate {
    Ask,
    Yes,
}

impl Gate {
    pub fn confirm(self, action: &str, detail: &str) -> Result<(), String> {
        println!("\n==> {action}");
        for line in detail.lines() {
            println!("    {line}");
        }
        if self == Gate::Yes {
            println!("    (--yes) proceeding");
            return Ok(());
        }
        print!("    continue? [y/N] ");
        io::stdout().flush().ok();
        let mut answer = String::new();
        io::stdin()
            .read_line(&mut answer)
            .map_err(|e| format!("could not read the answer: {e}"))?;
        match answer.trim() {
            "y" | "Y" | "yes" => Ok(()),
            _ => Err(format!("aborted at: {action}")),
        }
    }
}

/// Poll `probe` until it reports done, printing a heartbeat.
///
/// `interval_secs` is deliberately coarse. Both callers poll the GitHub API,
/// which is rate-limited, and a release build takes minutes — a tight loop buys
/// nothing and risks a 403 that reads as a failed release.
pub fn poll_until(
    what: &str,
    interval_secs: u64,
    max_secs: u64,
    mut probe: impl FnMut() -> Result<Option<String>, String>,
) -> Result<String, String> {
    let start = std::time::Instant::now();
    loop {
        if let Some(done) = probe()? {
            return Ok(done);
        }
        let waited = start.elapsed().as_secs();
        if waited >= max_secs {
            return Err(format!(
                "{what}: still not finished after {}m. Not a failure in itself — \
                 check the run on GitHub, then re-run this step; completed steps \
                 are skipped.",
                max_secs / 60
            ));
        }
        println!("    ... {what} ({waited}s)");
        std::thread::sleep(std::time::Duration::from_secs(interval_secs));
    }
}
