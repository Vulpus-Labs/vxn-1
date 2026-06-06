//! Parameter table (ticket 0012): the CLAP-facing surface every prior ticket
//! has been writing into.
//!
//! ## Layout
//!
//! CLAP ids are a stable, flat index space:
//!
//! ```text
//!   0 .. 162   Upper per-layer  (126 op + 1 algo + 5 LFO2 + 9 PEG +
//!                                5 mod-env + 3 assign + 5 stack + 8 mtx)
//! 162 .. 324   Lower per-layer  (same shape — every per-layer param is
//!                                doubled per ADR §8 / PARAMETERS.md "Scope")
//! 324 .. 343   Patch-level      (4 LFO1 + 2 voicing + 6 delay + 5 reverb +
//!                                2 master)
//! ```
//!
//! Total 343. The summary in PARAMETERS.md and the older count of 174 from
//! the ticket body both reflect the same enumeration: the ticket's 174 counts
//! a single layer plus patch-level; doubling per-layer gives the 343 surfaced
//! here.
//!
//! ## What is *not* in the table
//!
//! - Per-op `ratio_mode`, `ks_l_curve`, `ks_r_curve`: discrete topology
//!   selectors. Useful in patches and presets, but automating them mid-note
//!   would re-cook the operator's pitch / KS shape — not a continuous control.
//!   Set via patch state only.
//! - Mod-matrix `source` / `dest` / `curve` and slots 9..=16 `depth`:
//!   matrix topology + extra depths. Slots 1..=8 `depth` are CLAP-automatable
//!   per [`crate::matrix::N_CLAP_DEPTH_SLOTS`]; the rest is patch state.
//! - `edit_layer`: editor-side view state, not a sound parameter.
//! - Mod-matrix slot table itself: see [`crate::matrix::PatchMatrix`].
//!
//! ## Stable IDs?
//!
//! No — per memory `vxn1-id-stability-dropped`. IDs are kebab-case strings
//! for legibility but treat them as freely renameable; the preset format is
//! name-keyed and migrations live in the preset loader, not here.
//!
//! ## Descriptors
//!
//! Each [`ParamDesc`] carries everything a host / UI needs to render and
//! automate the param: kebab-case id, display name, plain `[min, max]`,
//! default, unit, kind (Float / Int / Bool / Enum) and an optional taper.
//! Normalised space is always `[0, 1]` regardless of plain shape; see
//! [`ParamDesc::to_normalised`] / [`ParamDesc::from_normalised`].
//!
//! The table is built as a single `const [ParamDesc; TOTAL_PARAMS]` via
//! macros — the same const-slice approach VXN1's `vxn-engine::params` ships,
//! avoiding a build script.

pub const N_OPS_PER_LAYER: usize = 6;
pub const N_PER_OP: usize = 21;
pub const N_PER_LAYER_REST: usize = 36;
pub const N_PER_LAYER: usize = N_OPS_PER_LAYER * N_PER_OP + N_PER_LAYER_REST; // 162
pub const N_LAYERS: usize = 2;
pub const N_PATCH_LEVEL: usize = 19;
pub const TOTAL_PARAMS: usize = N_PER_LAYER * N_LAYERS + N_PATCH_LEVEL; // 343

/// Start of the Lower-layer block in the flat CLAP id space.
pub const LOWER_BASE: usize = N_PER_LAYER;
/// Start of the patch-level block in the flat CLAP id space.
pub const PATCH_BASE: usize = N_PER_LAYER * N_LAYERS;

// ── Types ───────────────────────────────────────────────────────────────────

/// Taper for `Float` params. `Linear` is the default; `Exp` pins normalised
/// midpoint `0.5` to plain `mid` so a fader feels musical across wide ranges
/// (Hz, ms, etc.).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Taper {
    Linear,
    Exp { mid: f32 },
}

#[derive(Clone, Copy, Debug)]
pub enum ParamKind {
    Float {
        unit: &'static str,
        taper: Taper,
    },
    Int {
        unit: &'static str,
    },
    Bool,
    Enum {
        variants: &'static [&'static str],
    },
}

#[derive(Clone, Copy, Debug)]
pub struct ParamDesc {
    pub id: &'static str,
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub kind: ParamKind,
}

impl ParamDesc {
    #[inline]
    pub fn clamp(&self, v: f32) -> f32 {
        v.clamp(self.min, self.max)
    }

    pub fn unit(&self) -> &'static str {
        match self.kind {
            ParamKind::Float { unit, .. } => unit,
            ParamKind::Int { unit } => unit,
            _ => "",
        }
    }

    pub fn taper(&self) -> Taper {
        match self.kind {
            ParamKind::Float { taper, .. } => taper,
            _ => Taper::Linear,
        }
    }

    /// Plain → normalised `[0, 1]`. Honours the param's taper for `Float`;
    /// `Int` / `Bool` / `Enum` use a plain-linear mapping across `[min, max]`.
    pub fn to_normalised(&self, v: f32) -> f32 {
        let v = v.clamp(self.min, self.max);
        match self.kind {
            ParamKind::Float {
                taper: Taper::Exp { mid },
                ..
            } => taper_to_norm_exp(v, self.min, self.max, mid),
            _ => linear_to_norm(v, self.min, self.max),
        }
    }

    /// Normalised `[0, 1]` → plain. Inverse of [`to_normalised`].
    pub fn from_normalised(&self, n: f32) -> f32 {
        let n = n.clamp(0.0, 1.0);
        match self.kind {
            ParamKind::Float {
                taper: Taper::Exp { mid },
                ..
            } => taper_from_norm_exp(n, self.min, self.max, mid),
            _ => linear_from_norm(n, self.min, self.max),
        }
    }

    /// Variant labels for `Enum` params; empty slice otherwise.
    pub fn variants(&self) -> &'static [&'static str] {
        match self.kind {
            ParamKind::Enum { variants } => variants,
            _ => &[],
        }
    }

    /// Format `value` in this descriptor's plain unit. Used by the CLAP
    /// `value_to_text` path (ticket 0015) — no allocation budget concerns
    /// because the host calls it on the main thread.
    ///
    /// - `Enum`: variant label (clamped to range).
    /// - `Bool`: `"On"` / `"Off"`.
    /// - `Int`: integer + unit (or bare integer if no unit).
    /// - `Float`: two decimals + unit (three decimals if no unit).
    ///
    /// Sync-aware display (delay time as `1/8` when `delay_sync` is on) is
    /// out of scope here — the UI epic adds a wrapper that intercepts before
    /// hitting this method.
    pub fn display(&self, value: f32) -> String {
        match self.kind {
            ParamKind::Enum { variants } => {
                let n = variants.len();
                let i = (value.round().max(0.0) as usize).min(n.saturating_sub(1));
                variants[i].to_string()
            }
            ParamKind::Bool => {
                if value >= 0.5 { "On" } else { "Off" }.to_string()
            }
            ParamKind::Int { unit } => {
                let n = value.round() as i64;
                if unit.is_empty() {
                    format!("{n}")
                } else {
                    format!("{n} {unit}")
                }
            }
            ParamKind::Float { unit, .. } => {
                if unit.is_empty() {
                    format!("{value:.3}")
                } else {
                    format!("{value:.2} {unit}")
                }
            }
        }
    }
}

#[inline]
fn linear_to_norm(v: f32, min: f32, max: f32) -> f32 {
    if max > min {
        ((v - min) / (max - min)).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

#[inline]
fn linear_from_norm(n: f32, min: f32, max: f32) -> f32 {
    min + n.clamp(0.0, 1.0) * (max - min)
}

fn taper_to_norm_exp(value: f32, min: f32, max: f32, mid: f32) -> f32 {
    if !(min > 0.0 && mid > min && max > mid) {
        // min == 0 (or degenerate): exponential pinned at (0, 0),
        // (0.5, mid), (1, max). Same shape VXN1 uses for params whose floor
        // is genuinely zero.
        if !(max > mid && mid > 0.0) {
            return linear_to_norm(value, min, max);
        }
        let r = max / mid - 1.0;
        if r <= 0.0 {
            return linear_to_norm(value, min, max);
        }
        let a = mid / (r - 1.0);
        let k = 2.0 * r.ln();
        if !k.is_finite() {
            return linear_to_norm(value, min, max);
        }
        return ((value / a + 1.0).ln() / k).clamp(0.0, 1.0);
    }
    let v = value.clamp(min, max);
    if v <= mid {
        0.5 * (v / min).ln() / (mid / min).ln()
    } else {
        0.5 + 0.5 * (v / mid).ln() / (max / mid).ln()
    }
}

fn taper_from_norm_exp(n: f32, min: f32, max: f32, mid: f32) -> f32 {
    let n = n.clamp(0.0, 1.0);
    if !(min > 0.0 && mid > min && max > mid) {
        if !(max > mid && mid > 0.0) {
            return linear_from_norm(n, min, max);
        }
        let r = max / mid - 1.0;
        if r <= 0.0 {
            return linear_from_norm(n, min, max);
        }
        let a = mid / (r - 1.0);
        let k = 2.0 * r.ln();
        if !k.is_finite() {
            return linear_from_norm(n, min, max);
        }
        return a * ((k * n).exp() - 1.0);
    }
    if n <= 0.5 {
        min * (mid / min).powf(2.0 * n)
    } else {
        mid * (max / mid).powf(2.0 * n - 1.0)
    }
}

// ── Const constructors (compact macro-friendly) ─────────────────────────────

#[inline]
const fn fl(
    id: &'static str,
    name: &'static str,
    min: f32,
    max: f32,
    default: f32,
    unit: &'static str,
) -> ParamDesc {
    ParamDesc {
        id,
        name,
        min,
        max,
        default,
        kind: ParamKind::Float {
            unit,
            taper: Taper::Linear,
        },
    }
}

#[inline]
const fn flx(
    id: &'static str,
    name: &'static str,
    min: f32,
    max: f32,
    default: f32,
    unit: &'static str,
    mid: f32,
) -> ParamDesc {
    ParamDesc {
        id,
        name,
        min,
        max,
        default,
        kind: ParamKind::Float {
            unit,
            taper: Taper::Exp { mid },
        },
    }
}

#[inline]
const fn it(
    id: &'static str,
    name: &'static str,
    min: i32,
    max: i32,
    default: i32,
    unit: &'static str,
) -> ParamDesc {
    ParamDesc {
        id,
        name,
        min: min as f32,
        max: max as f32,
        default: default as f32,
        kind: ParamKind::Int { unit },
    }
}

#[inline]
const fn bl(id: &'static str, name: &'static str, default: bool) -> ParamDesc {
    ParamDesc {
        id,
        name,
        min: 0.0,
        max: 1.0,
        default: if default { 1.0 } else { 0.0 },
        kind: ParamKind::Bool,
    }
}

#[inline]
const fn en(
    id: &'static str,
    name: &'static str,
    variants: &'static [&'static str],
    default_idx: usize,
) -> ParamDesc {
    ParamDesc {
        id,
        name,
        min: 0.0,
        max: (variants.len() - 1) as f32,
        default: default_idx as f32,
        kind: ParamKind::Enum { variants },
    }
}

// ── Variant tables ──────────────────────────────────────────────────────────

pub const LFO_SHAPES: &[&str] = &["Sine", "Tri", "Saw+", "Saw-", "Pulse", "S&H"];
pub const LFO2_TRIGS: &[&str] = &["Free", "KeySync"];
pub const STACK_DISTRIBS: &[&str] = &["Linear", "Geometric", "Random"];
pub const ADSR_SHAPES: &[&str] = &["Lin", "Exp"];
pub const VOICING_MODES: &[&str] = &["Whole", "Layer", "Split"];
pub const ASSIGN_MODES: &[&str] = &["Poly", "Solo"];

// ── Macros (each yields a single array literal) ─────────────────────────────
//
// macro_rules! expansions are parsed as a single expression in expression
// context, so each macro here returns one `[ParamDesc; N]` array; the section
// arrays are then flattened by `concat_per_layer` / `concat_all` const fns.

macro_rules! op_block_arr {
    ($prefix:literal, $disp:literal, $n:literal) => {
        [
            fl(concat!($prefix, "op", $n, "-ratio"), concat!($disp, "Op ", $n, " Ratio"), 0.5, 31.0, 1.0, ""),
            flx(concat!($prefix, "op", $n, "-fixed-hz"), concat!($disp, "Op ", $n, " Fixed Hz"), 1.0, 9772.0, 440.0, "Hz", 100.0),
            fl(concat!($prefix, "op", $n, "-fine"), concat!($disp, "Op ", $n, " Fine"), 0.0, 0.99, 0.0, ""),
            it(concat!($prefix, "op", $n, "-detune"), concat!($disp, "Op ", $n, " Detune"), -7, 7, 0, ""),
            it(concat!($prefix, "op", $n, "-level"), concat!($disp, "Op ", $n, " Level"), 0, 99, 99, ""),
            it(concat!($prefix, "op", $n, "-vel-sens"), concat!($disp, "Op ", $n, " Vel Sens"), 0, 7, 3, ""),
            it(concat!($prefix, "op", $n, "-amp-sens"), concat!($disp, "Op ", $n, " Amp Sens"), 0, 3, 0, ""),
            it(concat!($prefix, "op", $n, "-eg-r1"), concat!($disp, "Op ", $n, " EG R1"), 0, 99, 99, ""),
            it(concat!($prefix, "op", $n, "-eg-r2"), concat!($disp, "Op ", $n, " EG R2"), 0, 99, 50, ""),
            it(concat!($prefix, "op", $n, "-eg-r3"), concat!($disp, "Op ", $n, " EG R3"), 0, 99, 35, ""),
            it(concat!($prefix, "op", $n, "-eg-r4"), concat!($disp, "Op ", $n, " EG R4"), 0, 99, 60, ""),
            it(concat!($prefix, "op", $n, "-eg-l1"), concat!($disp, "Op ", $n, " EG L1"), 0, 99, 99, ""),
            it(concat!($prefix, "op", $n, "-eg-l2"), concat!($disp, "Op ", $n, " EG L2"), 0, 99, 70, ""),
            it(concat!($prefix, "op", $n, "-eg-l3"), concat!($disp, "Op ", $n, " EG L3"), 0, 99, 50, ""),
            it(concat!($prefix, "op", $n, "-eg-l4"), concat!($disp, "Op ", $n, " EG L4"), 0, 99, 0, ""),
            it(concat!($prefix, "op", $n, "-ks-break-pt"), concat!($disp, "Op ", $n, " KS Break"), 0, 127, 60, ""),
            it(concat!($prefix, "op", $n, "-ks-l-depth"), concat!($disp, "Op ", $n, " KS L Depth"), 0, 99, 0, ""),
            it(concat!($prefix, "op", $n, "-ks-r-depth"), concat!($disp, "Op ", $n, " KS R Depth"), 0, 99, 30, ""),
            it(concat!($prefix, "op", $n, "-ks-rate"), concat!($disp, "Op ", $n, " KS Rate"), 0, 7, 2, ""),
            fl(concat!($prefix, "op", $n, "-pan"), concat!($disp, "Op ", $n, " Pan"), -1.0, 1.0, 0.0, ""),
            it(concat!($prefix, "op", $n, "-feedback"), concat!($disp, "Op ", $n, " Feedback"), 0, 7, 0, ""),
        ]
    };
}

macro_rules! per_layer_rest_arr {
    ($prefix:literal, $disp:literal) => {
        [
            it(concat!($prefix, "algo"), concat!($disp, "Algorithm"), 1, 32, 5, ""),
            en(concat!($prefix, "lfo2-shape"), concat!($disp, "LFO2 Shape"), LFO_SHAPES, 2),
            flx(concat!($prefix, "lfo2-rate"), concat!($disp, "LFO2 Rate"), 0.01, 50.0, 5.1, "Hz", 2.0),
            flx(concat!($prefix, "lfo2-delay"), concat!($disp, "LFO2 Delay"), 0.0, 4000.0, 180.0, "ms", 100.0),
            flx(concat!($prefix, "lfo2-fade"), concat!($disp, "LFO2 Fade"), 0.0, 4000.0, 320.0, "ms", 100.0),
            en(concat!($prefix, "lfo2-trig"), concat!($disp, "LFO2 Trig"), LFO2_TRIGS, 1),
            it(concat!($prefix, "peg-r1"), concat!($disp, "PEG R1"), 0, 99, 99, ""),
            it(concat!($prefix, "peg-r2"), concat!($disp, "PEG R2"), 0, 99, 50, ""),
            it(concat!($prefix, "peg-r3"), concat!($disp, "PEG R3"), 0, 99, 35, ""),
            it(concat!($prefix, "peg-r4"), concat!($disp, "PEG R4"), 0, 99, 60, ""),
            it(concat!($prefix, "peg-l1"), concat!($disp, "PEG L1"), -99, 99, 0, ""),
            it(concat!($prefix, "peg-l2"), concat!($disp, "PEG L2"), -99, 99, 0, ""),
            it(concat!($prefix, "peg-l3"), concat!($disp, "PEG L3"), -99, 99, 0, ""),
            it(concat!($prefix, "peg-l4"), concat!($disp, "PEG L4"), -99, 99, 0, ""),
            fl(concat!($prefix, "peg-depth"), concat!($disp, "PEG Depth"), 0.0, 1.0, 1.0, ""),
            flx(concat!($prefix, "mod-env-a"), concat!($disp, "Mod Env A"), 0.0, 4000.0, 2.0, "ms", 50.0),
            flx(concat!($prefix, "mod-env-d"), concat!($disp, "Mod Env D"), 0.0, 4000.0, 320.0, "ms", 100.0),
            fl(concat!($prefix, "mod-env-s"), concat!($disp, "Mod Env S"), 0.0, 1.0, 0.60, ""),
            flx(concat!($prefix, "mod-env-r"), concat!($disp, "Mod Env R"), 0.0, 4000.0, 180.0, "ms", 100.0),
            en(concat!($prefix, "mod-env-shape"), concat!($disp, "Mod Env Shape"), ADSR_SHAPES, 0),
            en(concat!($prefix, "assign-mode"), concat!($disp, "Assign"), ASSIGN_MODES, 0),
            bl(concat!($prefix, "legato"), concat!($disp, "Legato"), false),
            flx(concat!($prefix, "glide-time"), concat!($disp, "Glide"), 0.0, 2000.0, 12.0, "ms", 100.0),
            it(concat!($prefix, "stack-density"), concat!($disp, "Stack Density"), 1, 8, 4, ""),
            fl(concat!($prefix, "stack-detune"), concat!($disp, "Stack Detune"), 0.0, 100.0, 8.0, "ct"),
            fl(concat!($prefix, "stack-spread"), concat!($disp, "Stack Spread"), 0.0, 1.0, 0.60, ""),
            fl(concat!($prefix, "stack-phase"), concat!($disp, "Stack Phase"), 0.0, 1.0, 0.50, ""),
            en(concat!($prefix, "stack-distrib"), concat!($disp, "Stack Distrib"), STACK_DISTRIBS, 0),
            fl(concat!($prefix, "mtx1-depth"), concat!($disp, "Mtx 1 Depth"), -1.0, 1.0, 0.0, ""),
            fl(concat!($prefix, "mtx2-depth"), concat!($disp, "Mtx 2 Depth"), -1.0, 1.0, 0.0, ""),
            fl(concat!($prefix, "mtx3-depth"), concat!($disp, "Mtx 3 Depth"), -1.0, 1.0, 0.0, ""),
            fl(concat!($prefix, "mtx4-depth"), concat!($disp, "Mtx 4 Depth"), -1.0, 1.0, 0.0, ""),
            fl(concat!($prefix, "mtx5-depth"), concat!($disp, "Mtx 5 Depth"), -1.0, 1.0, 0.0, ""),
            fl(concat!($prefix, "mtx6-depth"), concat!($disp, "Mtx 6 Depth"), -1.0, 1.0, 0.0, ""),
            fl(concat!($prefix, "mtx7-depth"), concat!($disp, "Mtx 7 Depth"), -1.0, 1.0, 0.0, ""),
            fl(concat!($prefix, "mtx8-depth"), concat!($disp, "Mtx 8 Depth"), -1.0, 1.0, 0.0, ""),
        ]
    };
}

// Placeholder descriptor used to initialise the const-built array before
// const-fn flattening writes the real entries. Never observed at runtime.
const PLACEHOLDER: ParamDesc = ParamDesc {
    id: "",
    name: "",
    min: 0.0,
    max: 0.0,
    default: 0.0,
    kind: ParamKind::Bool,
};

const fn concat_per_layer(
    ops: [[ParamDesc; N_PER_OP]; N_OPS_PER_LAYER],
    rest: [ParamDesc; N_PER_LAYER_REST],
) -> [ParamDesc; N_PER_LAYER] {
    let mut out = [PLACEHOLDER; N_PER_LAYER];
    let mut k = 0;
    let mut i = 0;
    while i < N_OPS_PER_LAYER {
        let mut j = 0;
        while j < N_PER_OP {
            out[k] = ops[i][j];
            k += 1;
            j += 1;
        }
        i += 1;
    }
    let mut j = 0;
    while j < N_PER_LAYER_REST {
        out[k] = rest[j];
        k += 1;
        j += 1;
    }
    out
}

const fn concat_all(
    upper: [ParamDesc; N_PER_LAYER],
    lower: [ParamDesc; N_PER_LAYER],
    patch: [ParamDesc; N_PATCH_LEVEL],
) -> [ParamDesc; TOTAL_PARAMS] {
    let mut out = [PLACEHOLDER; TOTAL_PARAMS];
    let mut k = 0;
    let mut i = 0;
    while i < N_PER_LAYER {
        out[k] = upper[i];
        k += 1;
        i += 1;
    }
    let mut i = 0;
    while i < N_PER_LAYER {
        out[k] = lower[i];
        k += 1;
        i += 1;
    }
    let mut i = 0;
    while i < N_PATCH_LEVEL {
        out[k] = patch[i];
        k += 1;
        i += 1;
    }
    out
}

const UPPER: [ParamDesc; N_PER_LAYER] = concat_per_layer(
    [
        op_block_arr!("upper-", "U ", "1"),
        op_block_arr!("upper-", "U ", "2"),
        op_block_arr!("upper-", "U ", "3"),
        op_block_arr!("upper-", "U ", "4"),
        op_block_arr!("upper-", "U ", "5"),
        op_block_arr!("upper-", "U ", "6"),
    ],
    per_layer_rest_arr!("upper-", "U "),
);

const LOWER: [ParamDesc; N_PER_LAYER] = concat_per_layer(
    [
        op_block_arr!("lower-", "L ", "1"),
        op_block_arr!("lower-", "L ", "2"),
        op_block_arr!("lower-", "L ", "3"),
        op_block_arr!("lower-", "L ", "4"),
        op_block_arr!("lower-", "L ", "5"),
        op_block_arr!("lower-", "L ", "6"),
    ],
    per_layer_rest_arr!("lower-", "L "),
);

const PATCH: [ParamDesc; N_PATCH_LEVEL] = [
    en("lfo1-shape", "LFO1 Shape", LFO_SHAPES, 0),
    flx("lfo1-rate", "LFO1 Rate", 0.01, 50.0, 2.4, "Hz", 2.0),
    fl("lfo1-depth", "LFO1 Depth", 0.0, 1.0, 0.30, ""),
    bl("lfo1-sync", "LFO1 Sync", false),
    en("voicing-mode", "Voicing", VOICING_MODES, 1),
    it("split-point", "Split", 0, 127, 60, ""),
    bl("delay-on", "Delay On", true),
    flx("delay-time", "Delay Time", 1.0, 4000.0, 375.0, "ms", 100.0),
    bl("delay-sync", "Delay Sync", true),
    fl("delay-feedback", "Delay FB", 0.0, 0.95, 0.45, ""),
    fl("delay-mix", "Delay Mix", 0.0, 1.0, 0.25, ""),
    bl("delay-pingpong", "Ping-Pong", false),
    bl("reverb-on", "Reverb On", true),
    fl("reverb-size", "Reverb Size", 0.0, 1.0, 0.55, ""),
    flx("reverb-decay", "Reverb Decay", 0.1, 20.0, 2.4, "s", 2.0),
    fl("reverb-damp", "Reverb Damp", 0.0, 1.0, 0.50, ""),
    fl("reverb-mix", "Reverb Mix", 0.0, 1.0, 0.20, ""),
    fl("master-tune", "Master Tune", -100.0, 100.0, 0.0, "ct"),
    fl("master-volume", "Master Vol", -60.0, 6.0, -6.0, "dB"),
];

// ── The table ───────────────────────────────────────────────────────────────

/// All CLAP-automatable parameters. Index = stable CLAP id. Sectioned as
/// `[Upper × 162, Lower × 162, patch × 19]` — same flat ordering described in
/// the module-level layout block.
pub const PARAMS: [ParamDesc; TOTAL_PARAMS] = concat_all(UPPER, LOWER, PATCH);

/// Lookup by stable CLAP id. Const, just a bounds check + slice index.
#[inline]
pub fn desc(id: usize) -> Option<&'static ParamDesc> {
    PARAMS.get(id)
}

/// Alias of [`desc`] under the name the CLAP shell (ticket 0015) wires to.
/// Matches the VXN1 surface so the bridge code stays structurally identical.
#[inline]
pub fn desc_for_clap_id(idx: usize) -> Option<&'static ParamDesc> {
    PARAMS.get(idx)
}

/// Linear scan over [`PARAMS`] to resolve a kebab-case id. Used at preset-
/// load / debug time; not on the audio thread.
pub fn id_of(name: &str) -> Option<usize> {
    PARAMS.iter().position(|p| p.id == name)
}

// ── Section offsets (per-layer + patch) ─────────────────────────────────────
//
// Sourced here, not in `shared.rs`, because they describe the layout of the
// param table itself — `module_for_clap_id` and `EngineParams::snapshot_from`
// both read them.

pub(crate) const N_OP_BLOCK: usize = N_PER_OP * N_OPS_PER_LAYER; // 126
pub(crate) const OFF_ALGO: usize = N_OP_BLOCK;        // 126
pub(crate) const OFF_LFO2: usize = OFF_ALGO + 1;      // 127
pub(crate) const OFF_PEG: usize = OFF_LFO2 + 5;       // 132
pub(crate) const OFF_MOD_ENV: usize = OFF_PEG + 9;    // 141
pub(crate) const OFF_ASSIGN: usize = OFF_MOD_ENV + 5; // 146
pub(crate) const OFF_STACK: usize = OFF_ASSIGN + 3;   // 149
pub(crate) const OFF_MTX: usize = OFF_STACK + 5;      // 154

pub(crate) const OFF_LFO1: usize = 0;
pub(crate) const OFF_VOICING: usize = 4;
pub(crate) const OFF_DELAY: usize = 6;
pub(crate) const OFF_REVERB: usize = 12;
pub(crate) const OFF_MASTER: usize = 17;

/// Human-readable module path for the host's automation tree. `/`-separated:
/// the host renders nested folders. Per-layer ids resolve to e.g. `Upper /
/// Op 3`, `Lower / LFO 2`; patch-level ids to `Global / Delay` etc.
///
/// Strings are `&'static` — no allocation in the hot CLAP path.
pub fn module_for_clap_id(idx: usize) -> &'static str {
    if idx < LOWER_BASE {
        module_for_layer(idx, false)
    } else if idx < PATCH_BASE {
        module_for_layer(idx - LOWER_BASE, true)
    } else {
        module_for_patch(idx - PATCH_BASE)
    }
}

const UPPER_MODULES: [&str; 13] = [
    "Upper / Op 1",
    "Upper / Op 2",
    "Upper / Op 3",
    "Upper / Op 4",
    "Upper / Op 5",
    "Upper / Op 6",
    "Upper / Algorithm",
    "Upper / LFO 2",
    "Upper / PEG",
    "Upper / Mod Env",
    "Upper / Assign",
    "Upper / Stack",
    "Upper / Matrix",
];

const LOWER_MODULES: [&str; 13] = [
    "Lower / Op 1",
    "Lower / Op 2",
    "Lower / Op 3",
    "Lower / Op 4",
    "Lower / Op 5",
    "Lower / Op 6",
    "Lower / Algorithm",
    "Lower / LFO 2",
    "Lower / PEG",
    "Lower / Mod Env",
    "Lower / Assign",
    "Lower / Stack",
    "Lower / Matrix",
];

fn module_for_layer(off: usize, lower: bool) -> &'static str {
    let section = if off < N_OP_BLOCK {
        off / N_PER_OP
    } else if off == OFF_ALGO {
        6
    } else if off < OFF_PEG {
        7
    } else if off < OFF_MOD_ENV {
        8
    } else if off < OFF_ASSIGN {
        9
    } else if off < OFF_STACK {
        10
    } else if off < OFF_MTX {
        11
    } else if off < N_PER_LAYER {
        12
    } else {
        return "";
    };
    if lower {
        LOWER_MODULES[section]
    } else {
        UPPER_MODULES[section]
    }
}

fn module_for_patch(off: usize) -> &'static str {
    if off < OFF_VOICING {
        "Global / LFO 1"
    } else if off < OFF_DELAY {
        "Global / Voicing"
    } else if off < OFF_REVERB {
        "Global / Delay"
    } else if off < OFF_MASTER {
        "Global / Reverb"
    } else if off < N_PATCH_LEVEL {
        "Global / Master"
    } else {
        ""
    }
}

// ── Core-app ParamDesc bridge ───────────────────────────────────────────────
//
// `vxn_core_app::ParamDesc` is the shape the controller / editor surface
// programs against (ticket 0022). Field-by-field same layout as the local
// `ParamDesc` above with one rename: engine's `id` (kebab-case machine id)
// → core-app's `name`, and engine's `name` (display label) → core-app's
// `label`. Built as a const at compile time so `descriptor()` lookups stay
// O(1) with no allocation.

const fn to_core(d: &ParamDesc) -> vxn_core_app::ParamDesc {
    let kind = match d.kind {
        ParamKind::Float { unit, taper } => vxn_core_app::ParamKind::Float {
            unit,
            taper: match taper {
                Taper::Linear => vxn_core_app::Taper::Linear,
                Taper::Exp { mid } => vxn_core_app::Taper::Exp { mid },
            },
        },
        ParamKind::Int { unit } => vxn_core_app::ParamKind::Int { unit },
        ParamKind::Bool => vxn_core_app::ParamKind::Bool,
        ParamKind::Enum { variants } => vxn_core_app::ParamKind::Enum { variants },
    };
    vxn_core_app::ParamDesc {
        name: d.id,
        label: d.name,
        min: d.min,
        max: d.max,
        default: d.default,
        kind,
    }
}

const CORE_PLACEHOLDER: vxn_core_app::ParamDesc = vxn_core_app::ParamDesc {
    name: "",
    label: "",
    min: 0.0,
    max: 0.0,
    default: 0.0,
    kind: vxn_core_app::ParamKind::Bool,
};

/// Mirror of [`PARAMS`] in [`vxn_core_app::ParamDesc`] shape. Same length,
/// same index ordering; only `name` ↔ `label` differs in field semantics
/// (engine's `id` becomes core-app's `name`).
pub const CORE_PARAMS: [vxn_core_app::ParamDesc; TOTAL_PARAMS] = {
    let mut out = [CORE_PLACEHOLDER; TOTAL_PARAMS];
    let mut i = 0;
    while i < TOTAL_PARAMS {
        out[i] = to_core(&PARAMS[i]);
        i += 1;
    }
    out
};

/// Core-app descriptor for `idx` — the lookup the `ParamModel::descriptor`
/// impl (ticket 0022) wires to.
#[inline]
pub fn core_desc_for_clap_id(idx: usize) -> Option<&'static vxn_core_app::ParamDesc> {
    CORE_PARAMS.get(idx)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn total_count_matches_layout() {
        // 162 * 2 + 19 = 343.
        assert_eq!(TOTAL_PARAMS, 343);
        assert_eq!(PARAMS.len(), TOTAL_PARAMS);
    }

    #[test]
    fn ids_are_unique_and_kebab_case() {
        let mut seen = HashSet::new();
        for d in PARAMS.iter() {
            assert!(seen.insert(d.id), "duplicate id: {}", d.id);
            for c in d.id.chars() {
                assert!(
                    c == '-' || c.is_ascii_lowercase() || c.is_ascii_digit(),
                    "non-kebab character {c:?} in id {}",
                    d.id
                );
            }
        }
    }

    #[test]
    fn upper_lower_have_matching_suffix_ids() {
        for i in 0..N_PER_LAYER {
            let u = PARAMS[i].id;
            let l = PARAMS[LOWER_BASE + i].id;
            assert!(u.starts_with("upper-"), "{u}");
            assert!(l.starts_with("lower-"), "{l}");
            assert_eq!(&u[6..], &l[6..], "upper / lower id mismatch at {i}");
        }
    }

    #[test]
    fn defaults_are_in_range() {
        for d in PARAMS.iter() {
            assert!(
                d.default >= d.min && d.default <= d.max,
                "{}: default {} not in [{}, {}]",
                d.id,
                d.default,
                d.min,
                d.max
            );
        }
    }

    #[test]
    fn normalised_round_trips_at_endpoints_and_default() {
        // Endpoints + default must survive plain → norm → plain without drift
        // worth caring about (1e-3 for Exp tapers; 1e-6 for linear).
        for d in PARAMS.iter() {
            let eps = match d.kind {
                ParamKind::Float {
                    taper: Taper::Exp { .. },
                    ..
                } => 1e-3,
                _ => 1e-5,
            };
            for &v in &[d.min, d.default, d.max] {
                let n = d.to_normalised(v);
                let back = d.from_normalised(n);
                let scale = (d.max - d.min).abs().max(1.0);
                assert!(
                    (back - v).abs() / scale < eps,
                    "{}: roundtrip {} → {} → {}",
                    d.id,
                    v,
                    n,
                    back
                );
            }
        }
    }

    #[test]
    fn normalised_midpoint_lands_at_mid_for_exp_taper() {
        for d in PARAMS.iter() {
            if let ParamKind::Float {
                taper: Taper::Exp { mid },
                ..
            } = d.kind
            {
                let v = d.from_normalised(0.5);
                let scale = (d.max - d.min).abs().max(1.0);
                assert!(
                    (v - mid).abs() / scale < 1e-3,
                    "{}: midpoint {} != mid {}",
                    d.id,
                    v,
                    mid
                );
            }
        }
    }

    #[test]
    fn out_of_range_inputs_clamp() {
        for d in PARAMS.iter() {
            assert!(
                d.to_normalised(d.min - 999.0) <= 1e-6,
                "{}: under-min did not clamp to 0",
                d.id
            );
            assert!(
                (d.to_normalised(d.max + 999.0) - 1.0).abs() <= 1e-6,
                "{}: over-max did not clamp to 1",
                d.id
            );
        }
    }

    #[test]
    fn enum_variants_resolved_via_descriptor() {
        let voicing = desc(id_of("voicing-mode").expect("voicing-mode present")).unwrap();
        assert_eq!(voicing.variants(), VOICING_MODES);
    }

    #[test]
    fn master_section_is_at_table_tail() {
        let tune = id_of("master-tune").expect("master-tune");
        let vol = id_of("master-volume").expect("master-volume");
        assert_eq!(tune, TOTAL_PARAMS - 2);
        assert_eq!(vol, TOTAL_PARAMS - 1);
    }

    #[test]
    fn patch_block_begins_at_patch_base() {
        let lfo1 = id_of("lfo1-shape").expect("lfo1-shape");
        assert_eq!(lfo1, PATCH_BASE);
    }

    #[test]
    fn desc_for_clap_id_is_o1_and_total_bounded() {
        // O(1) bounds + slice indexing — no scan. Just sanity-check both
        // ends + an out-of-range miss.
        assert!(desc_for_clap_id(0).is_some());
        assert!(desc_for_clap_id(TOTAL_PARAMS - 1).is_some());
        assert!(desc_for_clap_id(TOTAL_PARAMS).is_none());
    }

    #[test]
    fn module_for_clap_id_routes_each_section() {
        let cases = [
            ("upper-op1-ratio", "Upper / Op 1"),
            ("upper-op6-feedback", "Upper / Op 6"),
            ("upper-algo", "Upper / Algorithm"),
            ("upper-lfo2-shape", "Upper / LFO 2"),
            ("upper-peg-r1", "Upper / PEG"),
            ("upper-mod-env-a", "Upper / Mod Env"),
            ("upper-assign-mode", "Upper / Assign"),
            ("upper-stack-density", "Upper / Stack"),
            ("upper-mtx1-depth", "Upper / Matrix"),
            ("lower-op3-pan", "Lower / Op 3"),
            ("lower-stack-distrib", "Lower / Stack"),
            ("lfo1-shape", "Global / LFO 1"),
            ("voicing-mode", "Global / Voicing"),
            ("delay-time", "Global / Delay"),
            ("reverb-decay", "Global / Reverb"),
            ("master-volume", "Global / Master"),
        ];
        for (id, expected) in cases {
            let idx = id_of(id).unwrap_or_else(|| panic!("missing id {id}"));
            assert_eq!(
                module_for_clap_id(idx),
                expected,
                "module for {id} (idx {idx})"
            );
        }
        assert_eq!(module_for_clap_id(TOTAL_PARAMS), "");
    }

    #[test]
    fn display_formats_each_kind() {
        let vol = desc(id_of("master-volume").unwrap()).unwrap();
        assert_eq!(vol.display(-6.0), "-6.00 dB");
        assert_eq!(vol.display(0.0), "0.00 dB");

        let algo = desc(id_of("upper-algo").unwrap()).unwrap();
        assert_eq!(algo.display(5.0), "5");
        assert_eq!(algo.display(32.4), "32");

        let lfo1 = desc(id_of("lfo1-shape").unwrap()).unwrap();
        assert_eq!(lfo1.display(0.0), "Sine");
        assert_eq!(lfo1.display(4.0), "Pulse");
        // Out-of-range clamps to the last variant rather than panicking.
        assert_eq!(lfo1.display(99.0), "S&H");

        let legato = desc(id_of("upper-legato").unwrap()).unwrap();
        assert_eq!(legato.display(0.0), "Off");
        assert_eq!(legato.display(1.0), "On");

        let detune = desc(id_of("upper-op1-detune").unwrap()).unwrap();
        assert_eq!(detune.display(0.0), "0");

        let stack_detune = desc(id_of("upper-stack-detune").unwrap()).unwrap();
        assert_eq!(stack_detune.display(8.0), "8.00 ct");

        let fine = desc(id_of("upper-op1-fine").unwrap()).unwrap();
        // Float with empty unit → 3-decimal bare format.
        assert_eq!(fine.display(0.0), "0.000");
    }

    #[test]
    fn range_fidelity_spot_checks() {
        // One descriptor per ParamKind shape per PARAMETERS.md.
        let cases: &[(&str, f32, f32, f32)] = &[
            ("upper-op3-ratio", 0.5, 31.0, 1.0),
            ("upper-algo", 1.0, 32.0, 5.0),
            ("upper-lfo2-shape", 0.0, 5.0, 2.0),
            ("upper-mod-env-shape", 0.0, 1.0, 0.0),
            ("upper-stack-distrib", 0.0, 2.0, 0.0),
            ("master-volume", -60.0, 6.0, -6.0),
        ];
        for (id, min, max, default) in cases {
            let d = desc(id_of(id).unwrap()).unwrap();
            assert_eq!(d.min, *min, "{id} min");
            assert_eq!(d.max, *max, "{id} max");
            assert_eq!(d.default, *default, "{id} default");
        }
    }

    #[test]
    fn stepped_classification_matches_kind() {
        for d in PARAMS.iter() {
            let stepped = !matches!(d.kind, ParamKind::Float { .. });
            match d.kind {
                ParamKind::Float { .. } => assert!(!stepped, "{}", d.id),
                ParamKind::Int { .. } | ParamKind::Bool | ParamKind::Enum { .. } => {
                    assert!(stepped, "{}", d.id)
                }
            }
        }
    }
}
