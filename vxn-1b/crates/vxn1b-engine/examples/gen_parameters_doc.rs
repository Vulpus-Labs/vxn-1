//! Generate `vxn-1b/PARAMETERS.md` from the live param table (0213).
//!
//! Run from the repo root:
//!
//!   cargo run --release --example gen_parameters_doc -p vxn1b-engine \
//!     > vxn-1b/PARAMETERS.md
//!
//! The doc is generated rather than hand-written so it cannot drift from
//! [`PARAMS`]: ranges, defaults, units and enum variants are read straight out
//! of the descriptors the host and the faceplate both use.
//!
//! Group headings are the one thing not derivable from the table — `PARAMS` is
//! a flat array whose grouping lives in source comments. They are listed below
//! as explicit spans, and `emit` asserts the spans tile the bank exactly, so
//! adding a param without placing it in a group fails the run loudly instead of
//! silently landing under whichever heading happened to precede it.

use vxn1b_engine::matrix::{
    CURVE_LABELS, DEST_LABELS, DEST_NAMES, SOURCE_LABELS, SOURCE_NAMES, SourceId,
};
use vxn1b_engine::params::{
    GLOBAL_PARAMS, Layer, MATRIX_SLOTS, PATCH_COUNT, PATCH_PARAMS, ParamId, TOTAL_PARAMS,
    global_clap_id, patch_clap_id,
};
use vxn_core_app::{ParamKind, Taper};

/// A heading and the number of consecutive params it covers.
type Group = (&'static str, usize);

const PATCH_GROUPS: &[Group] = &[
    ("Oscillators, sub and noise", 19),
    ("Filter", 8),
    ("Envelopes", 10),
    ("Amp", 1),
    ("Layer mix", 4),
    ("LFO 1", 6),
    ("LFO 2", 3),
    ("Voice", 8),
    ("Mod-matrix depths", 16),
];

const GLOBAL_GROUPS: &[Group] = &[
    ("Pitch bend", 1),
    ("Master", 5),
    ("Chorus", 4),
    ("Phaser", 6),
    ("Delay", 6),
    ("Reverb", 5),
    ("Dynamics", 8),
];

fn main() {
    println!("# VXN1b — Parameter reference");
    println!();
    println!(
        "**Generated** from the `vxn1b-engine` param table — do not edit by hand. Regenerate with:"
    );
    println!();
    println!("```sh");
    println!("cargo run --release --example gen_parameters_doc -p vxn1b-engine \\");
    println!("  > vxn-1b/PARAMETERS.md");
    println!("```");
    println!();
    println!(
        "The host sees **{TOTAL_PARAMS} parameters**: {PATCH_COUNT} patch params per layer × 2, \
         plus {} globals shared by both layers. A Layer 1 control and its Layer 2 twin are \
         separate automation targets; Layer 2's CLAP id is its Layer 1 id + {PATCH_COUNT}.",
        GLOBAL_PARAMS.len()
    );
    println!();
    println!("Columns:");
    println!();
    println!("- **CLAP id** — the automation id. For patch params this is the Layer 1 id.");
    println!("- **Name** — the preset key, and the `data-param` the faceplate binds to.");
    println!("- **Type** — `f` float, `i` integer, `b` bool, `e` enum (variants listed).");
    println!(
        "- **Range** — the plain, user-facing range. CLAP normalisation to [0, 1] is linear \
         across it; a *taper* (noted where present) warps only where the fader sits, not the \
         host range."
    );
    println!();
    println!("---");
    println!();

    println!("## Patch parameters (per layer)");
    println!();
    println!(
        "Each of these exists twice — once for Layer 1, once for Layer 2 — with independent \
         values and independent automation."
    );
    println!();
    emit(PATCH_PARAMS.as_slice(), PATCH_GROUPS, |p| {
        patch_clap_id(Layer::L1, p).expect("patch param has a Layer 1 clap id")
    });

    println!("## Global parameters");
    println!();
    println!(
        "One instance each, applied to both layers: tuning, master level, the limiter, \
         oversampling and the whole FX chain."
    );
    println!();
    emit(GLOBAL_PARAMS.as_slice(), GLOBAL_GROUPS, |p| {
        global_clap_id(p).expect("global param has a clap id")
    });

    emit_matrix();
}

/// Emit one bank as grouped markdown tables, checking the group spans tile it.
fn emit(bank: &[ParamId], groups: &[Group], clap_id: impl Fn(ParamId) -> usize) {
    let covered: usize = groups.iter().map(|(_, n)| n).sum();
    assert_eq!(
        covered,
        bank.len(),
        "group spans cover {covered} params but the bank has {} — a param was added or moved \
         without updating the group table in this generator",
        bank.len()
    );

    let mut at = 0;
    for (heading, n) in groups {
        println!("### {heading}");
        println!();
        println!("| CLAP id | Name | Label | Type | Range | Default |");
        println!("|--------:|------|-------|------|-------|---------|");
        for &p in &bank[at..at + n] {
            let d = p.desc();
            println!(
                "| {} | `{}` | {} | {} | {} | {} |",
                clap_id(p),
                d.name,
                d.label,
                type_col(&d.kind),
                range_col(d.min, d.max, &d.kind),
                default_col(d.default, &d.kind),
            );
        }
        println!();
        at += n;
    }
}

fn type_col(kind: &ParamKind) -> String {
    match kind {
        ParamKind::Float { .. } => "f".into(),
        ParamKind::Int { .. } => "i".into(),
        ParamKind::Bool => "b".into(),
        ParamKind::Enum { variants } => {
            format!("e {{{}}}", variants.join(", "))
        }
    }
}

fn range_col(min: f32, max: f32, kind: &ParamKind) -> String {
    match kind {
        ParamKind::Bool => "off / on".into(),
        ParamKind::Enum { variants } => format!("{} variants", variants.len()),
        ParamKind::Int { unit } => format!("{} .. {}{}", min as i64, max as i64, unit_suffix(unit)),
        ParamKind::Float { unit, taper } => {
            let t = match taper {
                Taper::Linear => String::new(),
                Taper::Exp { mid } => format!(" *(exp, mid {})*", trim(*mid)),
                Taper::BipolarExp { mid } => format!(" *(bipolar exp, mid ±{})*", trim(*mid)),
            };
            format!("{} .. {}{}{}", trim(min), trim(max), unit_suffix(unit), t)
        }
    }
}

fn default_col(default: f32, kind: &ParamKind) -> String {
    match kind {
        ParamKind::Bool => if default >= 0.5 { "on" } else { "off" }.into(),
        ParamKind::Enum { variants } => variants
            .get(default as usize)
            .map(|v| format!("`{v}`"))
            .unwrap_or_else(|| trim(default)),
        ParamKind::Int { unit } => format!("{}{}", default as i64, unit_suffix(unit)),
        ParamKind::Float { unit, .. } => format!("{}{}", trim(default), unit_suffix(unit)),
    }
}

fn unit_suffix(unit: &str) -> String {
    if unit.is_empty() {
        String::new()
    } else {
        format!(" {unit}")
    }
}

/// Format a float without a trailing `.0`, so integral ranges read as integers.
fn trim(v: f32) -> String {
    if v == v.trunc() && v.abs() < 1e7 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// The matrix is not a parameter bank — only the depths are automatable — so its
/// source/destination/curve sets are enumerated separately.
fn emit_matrix() {
    println!("## Mod matrix");
    println!();
    println!(
        "{MATRIX_SLOTS} slots per layer. A slot is a `source → destination` pair with a depth, a \
         curve, and a secondary *scale* source acting as a per-route VCA."
    );
    println!();
    println!(
        "Only the **depths** are host parameters (listed above). The topology — which source \
         feeds which destination, through which curve — lives in the patch state and is edited \
         in the faceplate's matrix overlay. That split is what lets a depth be automated without \
         the routing changing underneath it."
    );
    println!();

    println!("### Sources");
    println!();
    println!("| Label | Wire name | Polarity |");
    println!("|-------|-----------|----------|");
    // Index 0 is the empty-slot sentinel (`none` / `—`), not a routable source.
    for i in 1..SOURCE_NAMES.len() {
        let polarity = if SourceId::from_u8(i as u8).is_bipolar() {
            "bipolar"
        } else {
            "unipolar"
        };
        println!("| {} | `{}` | {} |", SOURCE_LABELS[i], SOURCE_NAMES[i], polarity);
    }
    println!();

    println!("### Destinations");
    println!();
    println!("| Label | Wire name |");
    println!("|-------|-----------|");
    for i in 1..DEST_NAMES.len() {
        println!("| {} | `{}` |", DEST_LABELS[i], DEST_NAMES[i]);
    }
    println!();

    println!("### Curves");
    println!();
    println!("{}", CURVE_LABELS.join(", "));
    println!();
}
