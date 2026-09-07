//! Offline renderer — play a note sequence through vxn-4 into a WAV file.
//!
//! ```text
//! cargo run --release -p vxn4-render -- --patch 2 --seq chord --os 16 out.wav
//! cargo run --release -p vxn4-render -- --all          # every patch, every sequence
//! ```
//!
//! This exists so ear-driven choices can be made before there is a plugin to
//! play. It is also the deterministic harness: the same arguments produce the
//! same samples, so a render can be diffed across a change.

// `fft` belongs to the `alias` binary only, which pulls it in by path. Declaring
// it here too would make it dead code in this binary.
mod seq;
mod wav;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use vxn4_engine::{Engine, N_MACROS, Quality, patch_names};

use seq::{SEQUENCES, Sequence, render_sequence};

const SR: f32 = 48_000.0;

struct Args {
    patch: usize,
    sequence: &'static Sequence,
    quality: Quality,
    /// Macro knob positions, held for the whole render.
    ///
    /// Static, not swept: the point is to A/B a knob position by ear, and a
    /// sweep would make every difference a moving target. A moving macro is
    /// the plugin's job.
    macros: [f32; N_MACROS],
    out: PathBuf,
    all: bool,
}

fn usage() -> String {
    let patches = patch_names()
        .iter()
        .enumerate()
        .map(|(i, n)| format!("{i}={n}"))
        .collect::<Vec<_>>()
        .join(" ");
    let seqs = SEQUENCES
        .iter()
        .map(|s| s.name)
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "vxn4-render — play a note sequence through vxn-4 into a WAV file\n\
         \n\
         USAGE:\n    \
             vxn4-render [OPTIONS] <out.wav>\n    \
             vxn4-render --all [--os 8|16]\n\
         \n\
         OPTIONS:\n    \
             --patch <n>    patch index (default 0).  {patches}\n    \
             --seq <name>   note sequence (default chord).  {seqs}\n    \
             --os <8|16>    operator-block oversampling (default 8)\n    \
             --macro <n=v>  macro knob n (1..{N_MACROS}) at v in 0..1, repeatable\n    \
             --all          render every patch x every sequence into ./vxn4-out/\n    \
             -h, --help     this text\n"
    )
}

/// Parse `--macro 3=0.7` into a zero-based index and a value.
///
/// Knobs are numbered from 1 on the command line because that is how they are
/// labelled everywhere else — `SOURCE_LABELS` says "Macro 1", and a CLI that
/// disagreed with the label would be a standing off-by-one.
fn parse_macro(v: &str) -> Result<(usize, f32), String> {
    let (n, val) = v
        .split_once('=')
        .ok_or_else(|| format!("--macro wants n=v, got {v:?}"))?;
    let n: usize = n.parse().map_err(|_| format!("bad macro number {n:?}"))?;
    if n == 0 || n > N_MACROS {
        return Err(format!("macro {n} out of range (1..={N_MACROS})"));
    }
    let val: f32 = val
        .parse()
        .map_err(|_| format!("bad macro value {val:?}"))?;
    if !(0.0..=1.0).contains(&val) {
        return Err(format!("macro value {val} out of range (0..=1)"));
    }
    Ok((n - 1, val))
}

fn parse() -> Result<Args, String> {
    let mut patch = 0usize;
    let mut sequence = &SEQUENCES[0];
    let mut quality = Quality::X8;
    let mut macros = [0.0f32; N_MACROS];
    let mut out = None;
    let mut all = false;

    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => return Err(usage()),
            "--all" => all = true,
            "--patch" => {
                let v = it.next().ok_or("--patch needs a value")?;
                patch = v.parse::<usize>().map_err(|_| format!("bad patch {v:?}"))?;
                if patch >= vxn4_engine::N_PATCHES {
                    return Err(format!(
                        "patch {patch} out of range (0..{})",
                        vxn4_engine::N_PATCHES
                    ));
                }
            }
            "--seq" => {
                let v = it.next().ok_or("--seq needs a value")?;
                sequence = SEQUENCES
                    .iter()
                    .find(|s| s.name == v)
                    .ok_or_else(|| format!("unknown sequence {v:?}"))?;
            }
            "--os" => {
                let v = it.next().ok_or("--os needs a value")?;
                quality = match v.as_str() {
                    "8" => Quality::X8,
                    "16" => Quality::X16,
                    _ => return Err(format!("--os must be 8 or 16, got {v:?}")),
                };
            }
            "--macro" => {
                let v = it.next().ok_or("--macro needs a value")?;
                let (i, val) = parse_macro(&v)?;
                macros[i] = val;
            }
            other if other.starts_with('-') => return Err(format!("unknown flag {other:?}")),
            other => out = Some(PathBuf::from(other)),
        }
    }

    if all {
        return Ok(Args {
            patch,
            sequence,
            quality,
            macros,
            out: PathBuf::from("vxn4-out"),
            all,
        });
    }

    Ok(Args {
        patch,
        sequence,
        quality,
        macros,
        out: out.ok_or_else(|| format!("no output path given\n\n{}", usage()))?,
        all,
    })
}

fn peak_dbfs(l: &[f32], r: &[f32]) -> f32 {
    let p = l.iter().chain(r.iter()).fold(0.0f32, |m, s| m.max(s.abs()));
    if p <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * p.log10()
    }
}

/// Macro positions as `1:0.70 4:0.30`, or empty when every knob is at zero.
fn macro_summary(macros: &[f32; N_MACROS]) -> String {
    macros
        .iter()
        .enumerate()
        .filter(|(_, v)| **v != 0.0)
        .map(|(i, v)| format!("{}:{v:.2}", i + 1))
        .collect::<Vec<_>>()
        .join(" ")
}

fn render_one(
    patch: usize,
    seq: &Sequence,
    quality: Quality,
    macros: &[f32; N_MACROS],
    out: &Path,
) -> std::io::Result<()> {
    let mut engine = Engine::new(SR);
    engine.set_patch(patch);
    engine.set_quality(quality);
    // After `set_patch`, which does not reset macros but does rebuild the
    // routing — setting them first would work too, and the order is asserted
    // by `macros_survive_a_patch_change`. Kept explicit here anyway.
    for (i, v) in macros.iter().enumerate() {
        engine.set_macro(i, *v);
    }

    let (l, r) = render_sequence(&mut engine, seq, SR);
    wav::write_stereo(out, &l, &r, SR as u32)?;

    println!(
        "  {:<8} {:<8} {:>3}x  {:>5.1}s  peak {:>6.1} dBFS  {:<12} -> {}",
        patch_names()[patch],
        seq.name,
        quality.factor(),
        l.len() as f32 / SR,
        peak_dbfs(&l, &r),
        macro_summary(macros),
        out.display()
    );
    Ok(())
}

fn run() -> Result<(), String> {
    let args = parse()?;

    if args.all {
        std::fs::create_dir_all(&args.out)
            .map_err(|e| format!("cannot create {}: {e}", args.out.display()))?;
        println!(
            "rendering {} patches x {} sequences",
            vxn4_engine::N_PATCHES,
            SEQUENCES.len()
        );
        for p in 0..vxn4_engine::N_PATCHES {
            for s in SEQUENCES.iter() {
                let name = format!(
                    "{}-{}-{}x.wav",
                    patch_names()[p],
                    s.name,
                    args.quality.factor()
                );
                render_one(p, s, args.quality, &args.macros, &args.out.join(name))
                    .map_err(|e| format!("write failed: {e}"))?;
            }
        }
        println!("\nwrote {}/", args.out.display());
        return Ok(());
    }

    render_one(
        args.patch,
        args.sequence,
        args.quality,
        &args.macros,
        &args.out,
    )
    .map_err(|e| format!("write failed: {e}"))
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            // --help arrives here too; it is not an error, but routing it
            // through the same path keeps the exit code honest for scripts.
            if msg.starts_with("vxn4-render —") {
                print!("{msg}");
                ExitCode::SUCCESS
            } else {
                eprintln!("error: {msg}");
                ExitCode::FAILURE
            }
        }
    }
}
