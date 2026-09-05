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

use vxn4_engine::{Engine, Quality, patch_names};

use seq::{SEQUENCES, Sequence, render_sequence};

const SR: f32 = 48_000.0;

struct Args {
    patch: usize,
    sequence: &'static Sequence,
    quality: Quality,
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
             --all          render every patch x every sequence into ./vxn4-out/\n    \
             -h, --help     this text\n"
    )
}

fn parse() -> Result<Args, String> {
    let mut patch = 0usize;
    let mut sequence = &SEQUENCES[0];
    let mut quality = Quality::X8;
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
            other if other.starts_with('-') => return Err(format!("unknown flag {other:?}")),
            other => out = Some(PathBuf::from(other)),
        }
    }

    if all {
        return Ok(Args {
            patch,
            sequence,
            quality,
            out: PathBuf::from("vxn4-out"),
            all,
        });
    }

    Ok(Args {
        patch,
        sequence,
        quality,
        out: out.ok_or_else(|| format!("no output path given\n\n{}", usage()))?,
        all,
    })
}

fn peak_dbfs(l: &[f32], r: &[f32]) -> f32 {
    let p = l
        .iter()
        .chain(r.iter())
        .fold(0.0f32, |m, s| m.max(s.abs()));
    if p <= 0.0 { f32::NEG_INFINITY } else { 20.0 * p.log10() }
}

fn render_one(patch: usize, seq: &Sequence, quality: Quality, out: &Path) -> std::io::Result<()> {
    let mut engine = Engine::new(SR);
    engine.set_patch(patch);
    engine.set_quality(quality);

    let (l, r) = render_sequence(&mut engine, seq, SR);
    wav::write_stereo(out, &l, &r, SR as u32)?;

    println!(
        "  {:<8} {:<8} {:>3}x  {:>5.1}s  peak {:>6.1} dBFS  -> {}",
        patch_names()[patch],
        seq.name,
        quality.factor(),
        l.len() as f32 / SR,
        peak_dbfs(&l, &r),
        out.display()
    );
    Ok(())
}

fn run() -> Result<(), String> {
    let args = parse()?;

    if args.all {
        std::fs::create_dir_all(&args.out)
            .map_err(|e| format!("cannot create {}: {e}", args.out.display()))?;
        println!("rendering {} patches x {} sequences", vxn4_engine::N_PATCHES, SEQUENCES.len());
        for p in 0..vxn4_engine::N_PATCHES {
            for s in SEQUENCES.iter() {
                let name = format!("{}-{}-{}x.wav", patch_names()[p], s.name, args.quality.factor());
                render_one(p, s, args.quality, &args.out.join(name))
                    .map_err(|e| format!("write failed: {e}"))?;
            }
        }
        println!("\nwrote {}/", args.out.display());
        return Ok(());
    }

    render_one(args.patch, args.sequence, args.quality, &args.out)
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
