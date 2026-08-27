//! Codegen guard for the shared-DSP extractions (ticket 0223).
//!
//! Every E040–E044 ticket claims "vectorisation unchanged". This makes that
//! checkable: disassemble the **linked release artefacts**, count SIMD
//! instructions inside each named hot symbol, and fail if one that used to
//! vectorise no longer does.
//!
//! Run: `cargo run -p vxn-asm-check --release -- [--update] [--verbose]`
//!
//! # Why the linked artefact, not the crate
//!
//! `[profile.release]` sets **thin** LTO, so a per-crate `.o` does not show what
//! actually ships — inlining across the crate boundary happens at link time.
//! Disassembling the cdylib is the only measurement that answers the question
//! the extractions raise ([[vxn1-ota-filter-perf]], and ADR 0002 §4).
//!
//! # Counting rule, and why the obvious one is wrong
//!
//! On AArch64 the vector arrangement suffix can sit on **either** the mnemonic
//! or the operands, depending on which syntax the disassembler emits:
//!
//! ```text
//!   Apple/LLVM style:      fadd.4s  v0, v1, v2      <- suffix on the MNEMONIC
//!   Canonical ARM style:   fadd     v0.4s, v1.4s    <- suffix on the OPERANDS
//! ```
//!
//! Ticket 0223's design said to match operand text specifically. On this
//! toolchain that is exactly backwards: against `libvxn1b_clap.dylib`,
//! `llvm-objdump` emits Apple style, so an operand-anchored `v[0-9]+\.4s`
//! matches **5** lines while the true count is **8940**. A harness written to
//! the ticket's letter would have reported near-zero on fully-vectorised code —
//! the precise failure [[vxn1-neon-grep-pitfall]] warns about, reproduced inside
//! the tool meant to catch it.
//!
//! So we match the arrangement suffix **anywhere in the instruction text**, and
//! stay syntax-agnostic: CI may run a different objdump than a dev machine, and
//! the count must not depend on which.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Vector arrangement suffixes. `.2s` is included even though it is a 64-bit
/// (half-register) form: dropping from `.4s` to `.2s` is still vectorised, and
/// we are guarding against the fall to *scalar*, not against narrowing.
const ARRANGEMENTS: [&str; 7] = [".16b", ".8b", ".8h", ".4h", ".4s", ".2s", ".2d"];

/// A hot symbol to watch, and the floor its SIMD count must not fall below.
///
/// Floors are deliberately **not** exact counts. Instruction selection moves a
/// little with every compiler and every unrelated edit; pinning exact numbers
/// would make this a tripwire for noise rather than for de-vectorisation. What
/// we care about is the cliff — a kernel that stops vectorising drops to zero or
/// near it, never by 5%.
struct Watch {
    /// Substring matched against the demangled symbol name.
    needle: &'static str,
    /// Which artefact it lives in.
    artefact: &'static str,
    /// Minimum SIMD instruction count. `0` means "record only, no floor yet".
    floor: usize,
    /// What breaks if this de-vectorises.
    why: &'static str,
}

/// Reference floors captured on main at ticket 0223 (macOS/aarch64, rustc
/// 1.95.0, thin LTO). See the module doc on why these are floors not equalities.
const WATCHES: &[Watch] = &[
    // ---- vxn-1 / vxn-1b (libvxn1b_clap.dylib) ----
    Watch {
        needle: "bank::RenderBank::render",
        artefact: "libvxn1b_clap.dylib",
        floor: 6000,
        why: "The whole per-block voice path inlines here — oscillators, the OTA \
              ladder lane loop, the envelopes. By far the largest SIMD symbol in \
              the build (9632 at capture), so a collapse here is the loudest \
              possible signal that an extraction de-vectorised the hot path.",
    },
    Watch {
        needle: "poly::oscillator::PolyOscillator::process",
        artefact: "libvxn1b_clap.dylib",
        floor: 200,
        why: "Poly oscillator: the sync/PM monomorphs inline into this symbol. A \
              runtime enum match reaching the lane loop drops it to scalar \
              (memory vxn1-soa-match-defeats-simd).",
    },
    Watch {
        needle: "engine::Engine::process_block",
        artefact: "libvxn1b_clap.dylib",
        floor: 180,
        why: "Block entry: mixing, metering and the output stage. 0235 would \
              touch the OutputStage decimator that lives here.",
    },
    Watch {
        needle: "fx::FxChain::process_block",
        artefact: "libvxn1b_clap.dylib",
        floor: 80,
        why: "The FX chain E041 rewrites wholesale (0228-0232). The single most \
              likely place for a shared-kernel extraction to cost vectorisation.",
    },
    Watch {
        needle: "voice::Voices::fill_stack_pos",
        artefact: "libvxn1b_clap.dylib",
        floor: 60,
        why: "Per-voice SoA scatter. 0067-0071's ratio-locked pitch work lives \
              near here.",
    },
    // ---- vxn-2 (libvxn2_clap.dylib) ----
    Watch {
        needle: "engine::Engine::cook_stacks_block",
        artefact: "libvxn2_clap.dylib",
        floor: 140,
        why: "vxn-2's largest DSP symbol: per-block stack cook. 0234 rewrites the \
              span plumbing that feeds it and 0237 the coefficient ramps.",
    },
    Watch {
        needle: "stack::lane_route_algo_",
        artefact: "libvxn2_clap.dylib",
        floor: 200,
        why: "The 32 #[inline(never)] lane-route monomorphs, summed (286 across \
              22 surviving symbols at capture — the linker folds identical ones). \
              These ARE the SoA fan-in the epic forbids touching; if a shared \
              type reaches them they go scalar.",
    },
    Watch {
        needle: "stack::Stack::note_on",
        artefact: "libvxn2_clap.dylib",
        floor: 50,
        why: "Voice-stack note-on scatter across 8 lanes.",
    },
    Watch {
        needle: "stack::stack_tick_stereo",
        artefact: "libvxn2_clap.dylib",
        floor: 35,
        why: "Per-sample stereo tick over the stack — the innermost vxn-2 loop.",
    },
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "usage: cargo run -p vxn-asm-check --release -- [--update] [--verbose]\n\
             \n\
             Counts SIMD instructions per hot symbol in the linked release\n\
             artefacts and fails if one falls below its recorded floor.\n\
             \n\
             --update   print the counts as a fresh WATCHES table and exit 0\n\
             --verbose  list every matched symbol, not just the watched ones"
        );
        return;
    }
    let update = args.iter().any(|a| a == "--update");
    let verbose = args.iter().any(|a| a == "--verbose");

    let objdump = match find_objdump() {
        Some(p) => p,
        None => {
            eprintln!(
                "asm-check: no llvm-objdump found.\n\
                 Tried: $LLVM_OBJDUMP, the active rustup toolchain's llvm-tools,\n\
                 /opt/homebrew/opt/llvm/bin, /usr/local/opt/llvm/bin.\n\
                 Install with `rustup component add llvm-tools` or `brew install llvm`.\n\
                 NOTE: the host /usr/bin tools are unreliable on these artefacts \
                 (memory vxn-host-nm-broken-llvm22)."
            );
            std::process::exit(2);
        }
    };
    eprintln!("asm-check: using {}", objdump.display());

    let root = workspace_root();
    let mut failures = Vec::new();
    let mut rows: Vec<(String, usize, usize)> = Vec::new();

    // Group by artefact so each is disassembled once — a cdylib is megabytes and
    // the disassembly dominates runtime.
    let mut by_artefact: BTreeMap<&str, Vec<&Watch>> = BTreeMap::new();
    for w in WATCHES {
        by_artefact.entry(w.artefact).or_default().push(w);
    }

    for (artefact, watches) in by_artefact {
        let path = root.join("target/release").join(artefact);
        if !path.exists() {
            eprintln!(
                "asm-check: {} not built.\n  Build it first, e.g. `cargo build --release -p vxn1b-clap`.",
                path.display()
            );
            std::process::exit(2);
        }
        let text = disassemble(&objdump, &path);
        let counts = count_per_symbol(&text);

        if verbose {
            eprintln!("--- {artefact}: {} symbols with SIMD ---", counts.len());
            for (sym, n) in counts.iter().filter(|(_, n)| **n > 0) {
                eprintln!("  {n:6}  {sym}");
            }
        }

        for w in watches {
            // Sum across monomorphs: a generic kernel appears once per
            // instantiation, and which ones exist is not this tool's business.
            let matched: Vec<(&String, &usize)> =
                counts.iter().filter(|(s, _)| s.contains(w.needle)).collect();
            let total: usize = matched.iter().map(|(_, n)| **n).sum();
            rows.push((w.needle.to_string(), total, w.floor));

            if matched.is_empty() {
                failures.push(format!(
                    "{}: NO SYMBOL matched `{}` in {}.\n    \
                     Either it was renamed/inlined away, or the artefact is stale.\n    \
                     Why it is watched: {}",
                    w.needle, w.needle, artefact, w.why
                ));
            } else if total < w.floor {
                failures.push(format!(
                    "{}: {} SIMD instructions across {} monomorph(s), floor is {}.\n    \
                     Why it is watched: {}",
                    w.needle,
                    total,
                    matched.len(),
                    w.floor,
                    w.why
                ));
            }
        }
    }

    println!("\n{:<52} {:>8} {:>8}", "symbol", "simd", "floor");
    println!("{}", "-".repeat(70));
    for (sym, n, floor) in &rows {
        let short = if sym.len() > 50 { &sym[sym.len() - 50..] } else { sym.as_str() };
        let flag = if *n < *floor { " FAIL" } else { "" };
        println!("{short:<52} {n:>8} {floor:>8}{flag}");
    }

    if update {
        println!("\n-- counts above are current; update WATCHES floors by hand --");
        return;
    }

    if !failures.is_empty() {
        eprintln!("\nasm-check FAILED — {} symbol(s) lost vectorisation:\n", failures.len());
        for f in &failures {
            eprintln!("  {f}\n");
        }
        std::process::exit(1);
    }
    println!("\nasm-check OK — every watched symbol is still vectorised.");
}

/// Disassemble with demangled symbol names.
fn disassemble(objdump: &Path, artefact: &Path) -> String {
    let out = Command::new(objdump)
        .args(["-d", "--demangle"])
        .arg(artefact)
        .output()
        .unwrap_or_else(|e| panic!("running {}: {e}", objdump.display()));
    if !out.status.success() {
        panic!(
            "{} failed on {}: {}",
            objdump.display(),
            artefact.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Walk the disassembly, attributing each SIMD instruction to the symbol it
/// falls under. Symbol headers look like `0000000000002788 <name>:`.
fn count_per_symbol(text: &str) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if let Some(sym) = symbol_header(line) {
            current = Some(sym);
            counts.entry(current.clone().unwrap()).or_insert(0usize);
            continue;
        }
        if let Some(sym) = &current
            && is_simd(line)
        {
            *counts.get_mut(sym).unwrap() += 1;
        }
    }
    counts
}

/// `0000000000002788 <some::symbol>:` → `some::symbol`.
fn symbol_header(line: &str) -> Option<String> {
    let rest = line.strip_suffix(':')?;
    let open = rest.find(" <")?;
    let (addr, name) = rest.split_at(open);
    if addr.is_empty() || !addr.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(name.trim_start_matches(" <").trim_end_matches('>').to_string())
}

/// Does this disassembly line carry a vector arrangement suffix?
///
/// Matches the suffix anywhere in the instruction text, so both the Apple
/// (`fadd.4s v0, v1`) and canonical (`fadd v0.4s, v1.4s`) syntaxes count. See
/// the module doc — anchoring to operands, as ticket 0223 specified, undercounts
/// by three orders of magnitude on this toolchain.
fn is_simd(line: &str) -> bool {
    // Instruction lines are `   addr: bytes\tmnemonic\toperands` — note the
    // SECOND tab. Testing only the mnemonic field would miss canonical ARM
    // syntax, where the suffix is on the operands; testing only the operands
    // would miss Apple syntax, where it is on the mnemonic. Join everything
    // after the first tab and test that.
    //
    // (The first version of this function took `.nth(1)` and reproduced exactly
    // the undercount it exists to prevent. `both_aarch64_syntaxes_count_as_simd`
    // caught it. Keep both syntaxes in that test.)
    let Some(first_tab) = line.find('\t') else {
        return false;
    };
    let insn = &line[first_tab + 1..];
    ARRANGEMENTS.iter().any(|a| insn.contains(a))
}

/// Locate an llvm-objdump. Host `/usr/bin` tools are deliberately last resort —
/// they misreport on rust-1.95 staticlibs (memory vxn-host-nm-broken-llvm22).
fn find_objdump() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("LLVM_OBJDUMP") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    // rustup's llvm-tools component, for the active toolchain.
    if let Ok(out) = Command::new("rustc").arg("--print").arg("sysroot").output()
        && out.status.success()
    {
        let sysroot = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if let Ok(rd) = std::fs::read_dir(Path::new(&sysroot).join("lib/rustlib")) {
            for e in rd.flatten() {
                let cand = e.path().join("bin/llvm-objdump");
                if cand.exists() {
                    return Some(cand);
                }
            }
        }
    }
    for p in ["/opt/homebrew/opt/llvm/bin/llvm-objdump", "/usr/local/opt/llvm/bin/llvm-objdump"] {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<root>/crates/vxn-asm-check`.
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_headers_parse() {
        assert_eq!(
            symbol_header("0000000000002788 <vxn_dsp::poly::ladder::process>:").as_deref(),
            Some("vxn_dsp::poly::ladder::process")
        );
        assert_eq!(symbol_header("   19508: 4f066400  \tmovi.4s\tv0, #0xc0"), None);
        assert_eq!(symbol_header("not a header"), None);
    }

    /// The whole point of the tool. Both syntaxes must count, because which one
    /// appears depends on the disassembler, not on the code.
    #[test]
    fn both_aarch64_syntaxes_count_as_simd() {
        // Apple style: suffix on the mnemonic. This is what llvm-objdump emits
        // here, and what ticket 0223's operand-anchored rule would have missed.
        assert!(is_simd("   19508: 4f066400 \tmovi.4s\tv0, #0xc0, lsl #24"));
        assert!(is_simd("   1952c: 6ea3e404 \tfcmgt.4s\tv4, v0, v3"));
        // Canonical ARM style: suffix on the operands.
        assert!(is_simd("   1953c: 4e617864 \tfadd\tv0.4s, v1.4s, v2.4s"));
        // Mixed, as emitted for widening ops.
        assert!(is_simd("   1953c: 4e617864 \tfcvtl2\tv4.2d, v3.4s"));
    }

    #[test]
    fn scalar_instructions_do_not_count() {
        assert!(!is_simd("   19508: 4f066400 \tfadd\ts0, s1, s2"));
        assert!(!is_simd("   19508: 4f066400 \tldr\tx0, [x1]"));
        assert!(!is_simd("   19508: 4f066400 \tret"));
    }

    /// A `.4s` in a demangled symbol name must not be read as an instruction —
    /// only the text after the first tab is instruction text.
    #[test]
    fn suffix_in_a_symbol_name_is_not_an_instruction() {
        assert!(!is_simd("0000000000002788 <weird::name.4s>:"));
        assert!(!is_simd("some line with .4s but no tab"));
    }

    #[test]
    fn counts_attribute_to_the_enclosing_symbol() {
        let text = "\
0000000000001000 <alpha>:
    1000: 00 \tfadd.4s\tv0, v1, v2
    1004: 00 \tret
0000000000002000 <beta>:
    2000: 00 \tldr\tx0, [x1]
";
        let c = count_per_symbol(text);
        assert_eq!(c.get("alpha"), Some(&1));
        assert_eq!(c.get("beta"), Some(&0));
    }
}
