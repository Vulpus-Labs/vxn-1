//! Headless preset auto-leveller.
//!
//! Renders every factory preset playing a held C-major triad over C4, measures
//! integrated loudness (ITU-R BS.1770 K-weighted LUFS) and full-window sample
//! peak, then computes the `master-volume` (dB) that lands the preset at a
//! target loudness — pulled down further if needed so the peak keeps the
//! requested headroom. The render is deterministic (RNG seeds from
//! note/velocity/voice-counter, no entropy), so re-runs are repeatable.
//!
//! Usage (from repo root):
//!   cargo run --release -p vxn2-engine --example level_presets            # dry run, print table
//!   cargo run --release -p vxn2-engine --example level_presets -- --apply # rewrite master-volume in each TOML
//!   ... -- --lufs -18 --headroom 3   # override target loudness / peak headroom
//!
//! Or via xtask:  cargo xtask level-presets [-- --apply]

use std::fs;
use std::path::{Path, PathBuf};

use vxn2_engine::engine::Engine;
use vxn2_engine::preset::from_toml_str;
use vxn2_engine::shared::{ParamModel, SharedParams};

const SR: f32 = 48_000.0;
const BLOCK: usize = 512;
const RENDER_SECS: f32 = 1.0;
const TRIAD: [u8; 3] = [60, 64, 67]; // C4, E4, G4
const VELOCITY: u8 = 100;

// master-volume param range (dB) and default when the key is absent.
const VOL_MIN_DB: f32 = -60.0;
const VOL_MAX_DB: f32 = 6.0;
const VOL_DEFAULT_DB: f32 = -6.0;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let apply = args.iter().any(|a| a == "--apply");
    let target_lufs = flag_f32(&args, "--lufs").unwrap_or(-16.0);
    let headroom_db = flag_f32(&args, "--headroom").unwrap_or(3.0);
    let peak_ceil_dbfs = -headroom_db;

    let files = collect_presets(&factory_dir());
    if files.is_empty() {
        eprintln!("no presets found under {}", factory_dir().display());
        std::process::exit(1);
    }

    println!(
        "target {target_lufs:.1} LUFS, peak ceiling {peak_ceil_dbfs:.1} dBFS  ({} presets){}",
        files.len(),
        if apply { "  [APPLY]" } else { "  [dry run]" }
    );
    println!(
        "{:<34} {:>8} {:>8} {:>8} {:>8}  {}",
        "preset", "LUFS", "peak", "cur dB", "new dB", ""
    );

    let mut changed = 0;
    for path in &files {
        let src = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("skip {}: {e}", path.display());
                continue;
            }
        };
        let blob = match from_toml_str(&src) {
            Ok((_meta, blob, _warn)) => blob,
            Err(e) => {
                eprintln!("skip {}: parse error {e:?}", path.display());
                continue;
            }
        };

        let m = measure(&blob);
        let cur_db = parse_master_volume(&src).unwrap_or(VOL_DEFAULT_DB);

        // Both LUFS and peak scale linearly (in dB) with the final master gain,
        // so a delta in master-volume shifts each by the same dB amount.
        let loud_delta = target_lufs - m.lufs;
        let peak_at_target = m.peak_dbfs + loud_delta;
        let excess = (peak_at_target - peak_ceil_dbfs).max(0.0); // how far over the ceiling
        let delta = loud_delta - excess; // peak guard wins
        let new_db = (cur_db + delta).clamp(VOL_MIN_DB, VOL_MAX_DB);

        let clamp_note = if (cur_db + delta) < VOL_MIN_DB {
            "  (clamped: floor)"
        } else if (cur_db + delta) > VOL_MAX_DB {
            "  (clamped: ceiling — still quiet)"
        } else if excess > 0.0 {
            "  (peak-capped)"
        } else {
            ""
        };

        let label = preset_label(path);
        println!(
            "{label:<34} {:>8.1} {:>8.1} {:>8.1} {:>8.1}{clamp_note}",
            m.lufs, m.peak_dbfs, cur_db, new_db
        );

        if apply && (new_db - cur_db).abs() > 0.05 {
            let out = set_master_volume(&src, new_db);
            if let Err(e) = fs::write(path, out) {
                eprintln!("write {}: {e}", path.display());
            } else {
                changed += 1;
            }
        }
    }

    if apply {
        println!("\napplied to {changed} file(s).");
    } else {
        println!("\ndry run — pass --apply to write master-volume back.");
    }
}

struct Measure {
    lufs: f32,
    peak_dbfs: f32,
}

/// Load a preset blob into a fresh engine, play the held triad, render
/// `RENDER_SECS`, and return integrated loudness + sample peak.
fn measure(blob: &[u8]) -> Measure {
    let shared = SharedParams::new();
    shared
        .load_bytes(blob)
        .expect("blob came from from_toml_str, must load");

    let mut engine = Engine::new(SR, BLOCK);
    engine.snapshot_params(&shared);
    // Measure the raw bus: the safety limiter would cap our peak reading.
    engine.params_mut().master.limiter_on = false;
    engine.apply_block_params();

    for &n in &TRIAD {
        engine.note_on(n, VELOCITY);
    }

    let total = (SR * RENDER_SECS) as usize;
    let mut l = vec![0.0f32; BLOCK];
    let mut r = vec![0.0f32; BLOCK];

    let mut peak = 0.0f32;
    let mut k = KWeight::new();
    let mut sum_sq = 0.0f64;
    let mut count = 0usize;

    let mut done = 0;
    while done < total {
        let n = BLOCK.min(total - done);
        l[..n].fill(0.0);
        r[..n].fill(0.0);
        engine.process_block(&mut l[..n], &mut r[..n]);
        for i in 0..n {
            peak = peak.max(l[i].abs()).max(r[i].abs());
            let kl = k.l.process(l[i]);
            let kr = k.r.process(r[i]);
            sum_sq += (kl * kl + kr * kr) as f64;
            count += 1;
        }
        done += n;
    }

    let mean_sq = if count > 0 { sum_sq / count as f64 } else { 0.0 };
    // BS.1770 integrated loudness (ungated; signal is a steady held chord).
    let lufs = if mean_sq > 1e-12 {
        -0.691 + 10.0 * (mean_sq as f32).log10()
    } else {
        -120.0
    };
    let peak_dbfs = if peak > 1e-9 {
        20.0 * peak.log10()
    } else {
        -120.0
    };
    Measure { lufs, peak_dbfs }
}

// ── BS.1770 K-weighting (two cascaded biquads, 48 kHz coefficients) ──────────

struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    fn new(b0: f32, b1: f32, b2: f32, a1: f32, a2: f32) -> Self {
        Self { b0, b1, b2, a1, a2, x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0 }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// Cascaded stage-1 (high shelf) + stage-2 (RLB highpass) for one channel.
struct KFilter {
    stage1: Biquad,
    stage2: Biquad,
}
impl KFilter {
    fn new() -> Self {
        // ITU-R BS.1770-4 coefficients for fs = 48 kHz.
        let stage1 = Biquad::new(
            1.53512485958697,
            -2.69169618940638,
            1.19839281085285,
            -1.69065929318241,
            0.73248077421585,
        );
        let stage2 = Biquad::new(
            1.0,
            -2.0,
            1.0,
            -1.99004745483398,
            0.99007225036621,
        );
        Self { stage1, stage2 }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        self.stage2.process(self.stage1.process(x))
    }
}

struct KWeight {
    l: KFilter,
    r: KFilter,
}
impl KWeight {
    fn new() -> Self {
        Self { l: KFilter::new(), r: KFilter::new() }
    }
}

// ── preset file plumbing ─────────────────────────────────────────────────────

fn factory_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("presets/factory")
}

/// All `<category>/<name>.toml` one level deep, sorted for stable output.
fn collect_presets(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(cats) = fs::read_dir(root) else {
        return out;
    };
    for cat in cats.flatten() {
        if !cat.path().is_dir() {
            continue;
        }
        let Ok(files) = fs::read_dir(cat.path()) else {
            continue;
        };
        for f in files.flatten() {
            let p = f.path();
            if p.extension().map(|e| e == "toml").unwrap_or(false) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn preset_label(path: &Path) -> String {
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
    let cat = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let mut s = format!("{cat}/{name}");
    if s.len() > 34 {
        s.truncate(33);
        s.push('…');
    }
    s
}

/// Read the current `master-volume = <db>` value, if the key is present.
fn parse_master_volume(src: &str) -> Option<f32> {
    for line in src.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("master-volume") {
            let rest = rest.trim_start();
            if let Some(val) = rest.strip_prefix('=') {
                // strip trailing inline comment
                let val = val.split('#').next().unwrap_or("").trim();
                if let Ok(v) = val.parse::<f32>() {
                    return Some(v);
                }
            }
        }
    }
    None
}

/// Return `src` with `master-volume` set to `new_db`. Replaces the existing
/// key if present; otherwise inserts it at the end of the `[params]` table
/// (before the next table header, or EOF).
fn set_master_volume(src: &str, new_db: f32) -> String {
    let new_line = format!("master-volume = {new_db:.2}");

    // Replace in place if the key already exists.
    if src.lines().any(|l| l.trim_start().starts_with("master-volume")) {
        let mut out = String::with_capacity(src.len());
        for line in src.lines() {
            if line.trim_start().starts_with("master-volume") {
                out.push_str(&new_line);
            } else {
                out.push_str(line);
            }
            out.push('\n');
        }
        return out;
    }

    // Insert: walk to `[params]`, then before the next `[...]` header insert.
    let lines: Vec<&str> = src.lines().collect();
    let mut out = String::with_capacity(src.len() + new_line.len() + 1);
    let mut in_params = false;
    let mut inserted = false;
    for line in &lines {
        let t = line.trim_start();
        if !inserted && in_params && t.starts_with('[') {
            out.push_str(&new_line);
            out.push('\n');
            inserted = true;
        }
        if t.starts_with("[params]") {
            in_params = true;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !inserted {
        // No table after [params] (or no [params] at all): append at EOF.
        if !in_params {
            out.push_str("\n[params]\n");
        }
        out.push_str(&new_line);
        out.push('\n');
    }
    out
}

fn flag_f32(args: &[String], name: &str) -> Option<f32> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1)?.parse().ok()
}
