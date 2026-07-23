//! Headless preset auto-leveller.
//!
//! Renders every factory preset holding a full vamping chord (C2, C3, E4, G4,
//! C5) at full velocity, measures the full-window sample peak (and integrated
//! ITU-R BS.1770 K-weighted LUFS, for reference), then sets each preset's
//! `master-volume` (dB) so the peak lands at the target ceiling (−6 dBFS by
//! default). The render is deterministic (RNG seeds from note/velocity/voice-
//! counter, no entropy), so re-runs are repeatable.
//!
//! Usage (from repo root):
//!   cargo run --release -p vxn2-engine --example level_presets            # dry run, print table
//!   cargo run --release -p vxn2-engine --example level_presets -- --apply # rewrite master-volume in each TOML
//!   ... -- --peak -8                 # override the target peak ceiling (dBFS)
//!
//! Or via xtask:  cargo xtask level-presets [-- --apply]

use std::fs;
use std::path::{Path, PathBuf};

use vxn2_engine::engine::Engine;
use vxn2_engine::preset::from_toml_str;
use vxn2_engine::shared::{ParamModel, SharedParams};

const SR: f32 = 48_000.0;
const BLOCK: usize = 512;
// Adaptive render window. The chord's true sample peak lands at the apex of the
// carriers' attack, which for a slow attack rate is *seconds* away (rate 13
// ≈ 6.4 s to full amplitude; rate 0 ≈ 20 s). A fixed short window measures the
// envelope mid-climb, under-reads the peak, and the one-shot correction then
// over-boosts long-attack presets (e.g. "Evolution"). So render until the
// running peak settles: at least MIN_SECS (stable LUFS reference), then stop
// once the peak has not risen for SETTLE_HOLD_SECS, capped at MAX_SECS.
const MIN_SECS: f32 = 1.0;
const MAX_SECS: f32 = 25.0;
const SETTLE_HOLD_SECS: f32 = 0.5;
// A new sample must exceed the running peak by this relative margin (~0.0087 dB)
// to count as the peak still rising — ignores sub-audible creep so a preset that
// has effectively plateaued isn't held open by rounding noise.
const PEAK_RISE_EPS: f32 = 1e-3;
// A vamping player's full voicing: octave C2+C3 root, plus E4/G4/C5 on top.
const CHORD: [u8; 5] = [36, 48, 64, 67, 72]; // C2, C3, E4, G4, C5
const VELOCITY: u8 = 127;

// master-volume param range (dB) and default when the key is absent.
const VOL_MIN_DB: f32 = -60.0;
const VOL_MAX_DB: f32 = 6.0;
const VOL_DEFAULT_DB: f32 = -6.0;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let apply = args.iter().any(|a| a == "--apply");
    let target_peak_dbfs = flag_f32(&args, "--peak").unwrap_or(-6.0);

    let files = collect_presets(&factory_dir());
    if files.is_empty() {
        eprintln!("no presets found under {}", factory_dir().display());
        std::process::exit(1);
    }

    println!(
        "target peak {target_peak_dbfs:.1} dBFS on chord [C2 C3 E4 G4 C5] @ vel {VELOCITY}  ({} presets){}",
        files.len(),
        if apply { "  [APPLY]" } else { "  [dry run]" }
    );
    println!(
        "{:<34} {:>8} {:>8} {:>6} {:>8} {:>8}  {}",
        "preset", "LUFS", "peak", "rendS", "cur dB", "new dB", ""
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

        // Peak scales linearly (in dB) with the final master gain, so shifting
        // master-volume by `delta` shifts the measured peak by the same amount.
        let delta = target_peak_dbfs - m.peak_dbfs;
        let new_db = (cur_db + delta).clamp(VOL_MIN_DB, VOL_MAX_DB);

        let clamp_note = if (cur_db + delta) < VOL_MIN_DB {
            "  (clamped: floor)"
        } else if (cur_db + delta) > VOL_MAX_DB {
            "  (clamped: ceiling — still under target)"
        } else {
            ""
        };

        let label = preset_label(path);
        println!(
            "{label:<34} {:>8.1} {:>8.1} {:>6.1} {:>8.1} {:>8.1}{clamp_note}",
            m.lufs, m.peak_dbfs, m.render_secs, cur_db, new_db
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
    /// Seconds actually rendered before the peak settled (or the cap hit).
    render_secs: f32,
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

    let min_samples = (SR * MIN_SECS) as usize;
    let max_samples = (SR * MAX_SECS) as usize;
    let hold_samples = (SR * SETTLE_HOLD_SECS) as usize;
    let mut l = vec![0.0f32; BLOCK];
    let mut r = vec![0.0f32; BLOCK];

    // Settle the master-gain smoother at the preset's target *before* striking
    // the chord. A fresh engine primes the smoother at the default gain and
    // `apply_block_params` only glides toward the preset value over ~5 ms
    // (MASTER_VOL_SMOOTH_MS). Strike the chord during that glide and the attack
    // transient is measured mid-ramp — the peak then isn't a pure function of
    // master-volume, so a single peak→drop correction misses. Render silence
    // past the glide first (master gain is the last, purely linear stage, so
    // once settled `peak_dbfs` scales 1:1 with master-volume → one exact pass).
    let warm = (SR * 0.05) as usize; // 50 ms ≫ the smoother time constant
    let mut done = 0;
    while done < warm {
        let n = BLOCK.min(warm - done);
        l[..n].fill(0.0);
        r[..n].fill(0.0);
        engine.process_block(&mut l[..n], &mut r[..n]);
        done += n;
    }

    for &n in &CHORD {
        engine.note_on(n, VELOCITY);
    }

    let mut peak = 0.0f32;
    let mut k = KWeight::new();
    let mut sum_sq = 0.0f64;
    let mut count = 0usize;
    // Samples elapsed since the running peak last rose past PEAK_RISE_EPS. Once
    // this exceeds `hold_samples` (and we're past the minimum window) the attack
    // apex is behind us and the peak reading is final.
    let mut since_rise = 0usize;

    while count < max_samples {
        let n = BLOCK.min(max_samples - count);
        l[..n].fill(0.0);
        r[..n].fill(0.0);
        engine.process_block(&mut l[..n], &mut r[..n]);
        for i in 0..n {
            let a = l[i].abs().max(r[i].abs());
            if a > peak * (1.0 + PEAK_RISE_EPS) {
                since_rise = 0;
            } else {
                since_rise += 1;
            }
            peak = peak.max(a);
            let kl = k.l.process(l[i]);
            let kr = k.r.process(r[i]);
            sum_sq += (kl * kl + kr * kr) as f64;
            count += 1;
        }
        if count >= min_samples && since_rise >= hold_samples {
            break;
        }
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
    Measure { lufs, peak_dbfs, render_secs: count as f32 / SR }
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
