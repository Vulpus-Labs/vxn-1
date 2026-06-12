//! Parameter table (ticket 0012): the CLAP-facing surface every prior ticket
//! has been writing into.
//!
//! ## Layout
//!
//! CLAP ids are a stable, flat index space:
//!
//! ```text
//!   0 .. 163   Per-patch        (126 op + 1 algo + 1 feedback + 5 LFO2 +
//!                                9 PEG + 5 mod-env + 3 assign + 5 stack +
//!                                8 mtx)
//! 163 .. 188   Patch-level      (3 LFO1 + 6 delay + 5 reverb + 2 master +
//!                                9 filter)
//! ```
//!
//! Total 188. Per [ADR 0002] the dual-layer (Whole / Layer / Split) surface
//! is gone — a patch is one parameter set. Each op block is 21 params: the
//! 20 continuous controls plus a trailing `ratio-mode` enum (Ratio / Fixed).
//!
//! ## What is *not* in the table
//!
//! - Per-op `ks_l_curve`, `ks_r_curve`: discrete topology selectors.
//!   Automating them mid-note would re-cook the operator's KS shape — not a
//!   continuous control. Set via patch state only.
//!   (`ratio_mode` was in this group per [ADR 0002] but is now a CLAP enum —
//!   `opN-ratio-mode` — so the editor's Ratio/Fixed selector can drive it.)
//! - Mod-matrix `source` / `dest` / `curve` and slots 9..=16 `depth`:
//!   matrix topology + extra depths. Slots 1..=8 `depth` are CLAP-automatable
//!   per [`crate::matrix::N_CLAP_DEPTH_SLOTS`]; the rest is patch state.
//! - Mod-matrix slot table itself: see [`crate::matrix::MatrixTable`].
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

pub const N_OPS: usize = 6;
pub const N_PER_OP: usize = 21;
pub const N_PER_PATCH_REST: usize = 37;
pub const N_PER_PATCH: usize = N_OPS * N_PER_OP + N_PER_PATCH_REST; // 163
pub const N_PATCH_LEVEL: usize = 25; // 3 LFO1 + 6 delay + 5 reverb + 2 master + 9 filter
pub const TOTAL_PARAMS: usize = N_PER_PATCH + N_PATCH_LEVEL; // 188

/// Start of the patch-level block in the flat CLAP id space.
pub const PATCH_BASE: usize = N_PER_PATCH;

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
/// Per-op tuning mode. Index order matches `vxn2_dsp::op::RatioMode`
/// (`Ratio` = 0, `Fixed` = 1).
pub const RATIO_MODES: &[&str] = &["Ratio", "Fixed"];
pub const STACK_DISTRIBS: &[&str] = &["Linear", "Geometric", "Random"];
pub const ADSR_SHAPES: &[&str] = &["Lin", "Exp"];
pub const ASSIGN_MODES: &[&str] = &["Poly", "Solo"];
/// Filter response. Index order matches `vxn2_dsp::filter::FilterMode`
/// (`Lp` = 0, `Hp`, `Bp`, `Notch`).
pub const FILTER_MODES: &[&str] = &["LP", "HP", "BP", "Notch"];
/// Filter slope. Index order matches `vxn2_dsp::filter::FilterSlope`
/// (`Pole2` = 0, `Pole4` = 1).
pub const FILTER_SLOPES: &[&str] = &["2-Pole", "4-Pole"];
/// Filter oversample factor (1× / 2× / 4× / 8×); the enum index maps to the
/// factor `1 << idx` in the render path (ticket 0084).
pub const FILTER_OVERSAMPLE: &[&str] = &["1×", "2×", "4×", "8×"];

// ── Macros (each yields a single array literal) ─────────────────────────────
//
// macro_rules! expansions are parsed as a single expression in expression
// context, so each macro here returns one `[ParamDesc; N]` array; the section
// arrays are then flattened by `concat_per_patch` const fn.

macro_rules! op_block_arr {
    ($n:literal) => {
        [
            it(concat!("op", $n, "-num"), concat!("Op ", $n, " Num"), 1, 32, 1, ""),
            it(concat!("op", $n, "-denom"), concat!("Op ", $n, " Denom"), 1, 8, 1, ""),
            flx(concat!("op", $n, "-fixed-hz"), concat!("Op ", $n, " Fixed Hz"), 1.0, 9772.0, 440.0, "Hz", 100.0),
            it(concat!("op", $n, "-fine"), concat!("Op ", $n, " Fine"), -100, 100, 0, ""),
            it(concat!("op", $n, "-detune"), concat!("Op ", $n, " Detune"), -100, 100, 0, "ct"),
            it(concat!("op", $n, "-level"), concat!("Op ", $n, " Level"), 0, 99, 99, ""),
            it(concat!("op", $n, "-vel-sens"), concat!("Op ", $n, " Vel Sens"), 0, 7, 3, ""),
            it(concat!("op", $n, "-eg-r1"), concat!("Op ", $n, " EG R1"), 0, 99, 99, ""),
            it(concat!("op", $n, "-eg-r2"), concat!("Op ", $n, " EG R2"), 0, 99, 50, ""),
            it(concat!("op", $n, "-eg-r3"), concat!("Op ", $n, " EG R3"), 0, 99, 35, ""),
            it(concat!("op", $n, "-eg-r4"), concat!("Op ", $n, " EG R4"), 0, 99, 60, ""),
            it(concat!("op", $n, "-eg-l1"), concat!("Op ", $n, " EG L1"), 0, 99, 99, ""),
            it(concat!("op", $n, "-eg-l2"), concat!("Op ", $n, " EG L2"), 0, 99, 70, ""),
            it(concat!("op", $n, "-eg-l3"), concat!("Op ", $n, " EG L3"), 0, 99, 50, ""),
            it(concat!("op", $n, "-eg-l4"), concat!("Op ", $n, " EG L4"), 0, 99, 0, ""),
            it(concat!("op", $n, "-ks-break-pt"), concat!("Op ", $n, " KS Break"), 0, 127, 60, ""),
            it(concat!("op", $n, "-ks-l-depth"), concat!("Op ", $n, " KS L Depth"), 0, 99, 0, ""),
            it(concat!("op", $n, "-ks-r-depth"), concat!("Op ", $n, " KS R Depth"), 0, 99, 30, ""),
            it(concat!("op", $n, "-ks-rate"), concat!("Op ", $n, " KS Rate"), 0, 7, 2, ""),
            fl(concat!("op", $n, "-pan"), concat!("Op ", $n, " Pan"), -1.0, 1.0, 0.0, ""),
            // Tuning mode (Ratio / Fixed). Per ADR 0002 this was patch-only
            // "discrete topology"; exposed as a CLAP enum so the editor's
            // Ratio/Fixed selector can drive it. Appended at the end of the
            // op block so the existing per-op offsets (read_op) are unchanged.
            en(concat!("op", $n, "-ratio-mode"), concat!("Op ", $n, " Ratio Mode"), RATIO_MODES, 0),
        ]
    };
}

macro_rules! per_patch_rest_arr {
    () => {
        [
            it("algo", "Algorithm", 1, 32, 5, ""),
            fl("feedback", "Feedback", 0.0, 7.0, 0.0, ""),
            en("lfo2-shape", "LFO2 Shape", LFO_SHAPES, 2),
            flx("lfo2-rate", "LFO2 Rate", 0.01, 50.0, 5.1, "Hz", 2.0),
            flx("lfo2-delay", "LFO2 Delay", 0.0, 4000.0, 180.0, "ms", 100.0),
            flx("lfo2-fade", "LFO2 Fade", 0.0, 4000.0, 320.0, "ms", 100.0),
            bl("lfo2-sync", "LFO2 Sync", false),
            it("peg-r1", "PEG R1", 0, 99, 99, ""),
            it("peg-r2", "PEG R2", 0, 99, 50, ""),
            it("peg-r3", "PEG R3", 0, 99, 35, ""),
            it("peg-r4", "PEG R4", 0, 99, 60, ""),
            it("peg-l1", "PEG L1", -99, 99, 0, ""),
            it("peg-l2", "PEG L2", -99, 99, 0, ""),
            it("peg-l3", "PEG L3", -99, 99, 0, ""),
            it("peg-l4", "PEG L4", -99, 99, 0, ""),
            fl("peg-depth", "PEG Depth", 0.0, 1.0, 1.0, ""),
            flx("mod-env-a", "Mod Env A", 0.0, 4000.0, 2.0, "ms", 50.0),
            flx("mod-env-d", "Mod Env D", 0.0, 4000.0, 320.0, "ms", 100.0),
            fl("mod-env-s", "Mod Env S", 0.0, 1.0, 0.60, ""),
            flx("mod-env-r", "Mod Env R", 0.0, 4000.0, 180.0, "ms", 100.0),
            en("mod-env-shape", "Mod Env Shape", ADSR_SHAPES, 0),
            en("assign-mode", "Assign", ASSIGN_MODES, 0),
            bl("legato", "Legato", false),
            flx("glide-time", "Glide", 0.0, 2000.0, 12.0, "ms", 100.0),
            it("stack-density", "Stack Density", 1, 8, 4, ""),
            flx("stack-detune", "Stack Detune", 0.0, 50.0, 8.0, "ct", 10.0),
            fl("stack-spread", "Stack Spread", 0.0, 1.0, 0.60, ""),
            fl("stack-phase", "Stack Phase", 0.0, 1.0, 0.50, ""),
            en("stack-distrib", "Stack Distrib", STACK_DISTRIBS, 0),
            fl("mtx1-depth", "Mtx 1 Depth", -1.0, 1.0, 0.0, ""),
            fl("mtx2-depth", "Mtx 2 Depth", -1.0, 1.0, 0.0, ""),
            fl("mtx3-depth", "Mtx 3 Depth", -1.0, 1.0, 0.0, ""),
            fl("mtx4-depth", "Mtx 4 Depth", -1.0, 1.0, 0.0, ""),
            fl("mtx5-depth", "Mtx 5 Depth", -1.0, 1.0, 0.0, ""),
            fl("mtx6-depth", "Mtx 6 Depth", -1.0, 1.0, 0.0, ""),
            fl("mtx7-depth", "Mtx 7 Depth", -1.0, 1.0, 0.0, ""),
            fl("mtx8-depth", "Mtx 8 Depth", -1.0, 1.0, 0.0, ""),
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

const fn concat_per_patch(
    ops: [[ParamDesc; N_PER_OP]; N_OPS],
    rest: [ParamDesc; N_PER_PATCH_REST],
) -> [ParamDesc; N_PER_PATCH] {
    let mut out = [PLACEHOLDER; N_PER_PATCH];
    let mut k = 0;
    let mut i = 0;
    while i < N_OPS {
        let mut j = 0;
        while j < N_PER_OP {
            out[k] = ops[i][j];
            k += 1;
            j += 1;
        }
        i += 1;
    }
    let mut j = 0;
    while j < N_PER_PATCH_REST {
        out[k] = rest[j];
        k += 1;
        j += 1;
    }
    out
}

const fn concat_all(
    per_patch: [ParamDesc; N_PER_PATCH],
    patch: [ParamDesc; N_PATCH_LEVEL],
) -> [ParamDesc; TOTAL_PARAMS] {
    let mut out = [PLACEHOLDER; TOTAL_PARAMS];
    let mut k = 0;
    let mut i = 0;
    while i < N_PER_PATCH {
        out[k] = per_patch[i];
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

const PER_PATCH: [ParamDesc; N_PER_PATCH] = concat_per_patch(
    [
        op_block_arr!("1"),
        op_block_arr!("2"),
        op_block_arr!("3"),
        op_block_arr!("4"),
        op_block_arr!("5"),
        op_block_arr!("6"),
    ],
    per_patch_rest_arr!(),
);

const PATCH: [ParamDesc; N_PATCH_LEVEL] = [
    en("lfo1-shape", "LFO1 Shape", LFO_SHAPES, 0),
    flx("lfo1-rate", "LFO1 Rate", 0.01, 50.0, 2.4, "Hz", 2.0),
    bl("lfo1-sync", "LFO1 Sync", false),
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
    // ── Filter (E007 / ADR 0004) ──────────────────────────────────────────
    // Optional per-voice oversampled OTA-C ladder, off by default so an
    // unchanged patch stays bit-identical. `enable`/`mode`/`slope`/`oversample`
    // are structural selectors — automatable like `delay-on`/`algo`/`lfo2-shape`
    // (the codebase has no non-automatable flag), but they reconfigure topology
    // rather than sweeping. `cutoff`/`resonance` are matrix dests
    // (`DestId::Cutoff` / `DestId::Resonance`).
    bl("filter-enable", "Filter Enable", false),
    flx("filter-cutoff", "Filter Cutoff", 16.3516, 20000.0, 12000.0, "Hz", 1000.0),
    fl("filter-resonance", "Filter Reso", 0.0, 1.0, 0.0, ""),
    en("filter-mode", "Filter Mode", FILTER_MODES, 0),
    en("filter-slope", "Filter Slope", FILTER_SLOPES, 1),
    flx("filter-drive", "Filter Drive", 0.1, 16.0, 1.0, "", 1.0),
    en("filter-oversample", "Filter OS", FILTER_OVERSAMPLE, 2),
    // Dedicated filter key-tracking amount (VXN-1 `FilterKeyTrack`): cutoff
    // shifts `(note − 12)/12 × amount` octaves, centred on C0 (MIDI 12). At
    // 1.0 the cutoff tracks the played pitch exactly (1 oct/oct); with the
    // cutoff fader at its C0 floor, 100 % key-track lands cutoff on the note
    // pitch. Applied engine-side, not via the matrix. Appended at the very
    // end of the flat space so the blob v7→v8 migration stays a 1:1 prefix.
    fl("filter-keytrack", "Filter KeyTrk", 0.0, 1.0, 0.0, ""),
    // Cutoff "Tuned" toggle (VXN-1 parity): UI-only. When on, the cutoff
    // fader is read/displayed as a musical note (C0..C4, semitone-snapped);
    // the stored value stays Hz, so the DSP and automation are unaffected.
    bl("filter-cutoff-tuned", "Cutoff Tuned", false),
];

// ── The table ───────────────────────────────────────────────────────────────

/// All CLAP-automatable parameters. Index = stable CLAP id. Sectioned as
/// `[per-patch × 163, patch × 23]` — same flat ordering described in
/// the module-level layout block.
pub const PARAMS: [ParamDesc; TOTAL_PARAMS] = concat_all(PER_PATCH, PATCH);

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

// ── Section offsets (per-patch + patch) ─────────────────────────────────────
//
// Sourced here, not in `shared.rs`, because they describe the layout of the
// param table itself — `module_for_clap_id` and `EngineParams::snapshot_from`
// both read them.

pub(crate) const N_OP_BLOCK: usize = N_PER_OP * N_OPS; // 126
pub(crate) const OFF_ALGO: usize = N_OP_BLOCK;        // 126
pub(crate) const OFF_FEEDBACK: usize = OFF_ALGO + 1;  // 127 (patch-level FB applied to algo's structural FB op)
pub(crate) const OFF_LFO2: usize = OFF_FEEDBACK + 1;  // 128
pub(crate) const OFF_PEG: usize = OFF_LFO2 + 5;       // 133
pub(crate) const OFF_MOD_ENV: usize = OFF_PEG + 9;    // 142
pub(crate) const OFF_ASSIGN: usize = OFF_MOD_ENV + 5; // 147
pub(crate) const OFF_STACK: usize = OFF_ASSIGN + 3;   // 150
pub(crate) const OFF_MTX: usize = OFF_STACK + 5;      // 155

pub(crate) const OFF_LFO1: usize = 0;
pub(crate) const OFF_DELAY: usize = 3;
pub(crate) const OFF_REVERB: usize = 9;
pub(crate) const OFF_MASTER: usize = 14;
pub(crate) const OFF_FILTER: usize = 16; // after master-tune + master-volume

/// Human-readable module path for the host's automation tree. `/`-separated:
/// the host renders nested folders. Per-patch ids resolve to e.g. `Op 3`,
/// `LFO 2`; patch-level ids to `Global / Delay` etc.
///
/// Strings are `&'static` — no allocation in the hot CLAP path.
pub fn module_for_clap_id(idx: usize) -> &'static str {
    if idx < PATCH_BASE {
        module_for_per_patch(idx)
    } else {
        module_for_patch(idx - PATCH_BASE)
    }
}

const PER_PATCH_MODULES: [&str; 14] = [
    "Op 1",
    "Op 2",
    "Op 3",
    "Op 4",
    "Op 5",
    "Op 6",
    "Algorithm",
    "Feedback",
    "LFO 2",
    "PEG",
    "Mod Env",
    "Assign",
    "Stack",
    "Matrix",
];

fn module_for_per_patch(off: usize) -> &'static str {
    let section = if off < N_OP_BLOCK {
        off / N_PER_OP
    } else if off == OFF_ALGO {
        6
    } else if off == OFF_FEEDBACK {
        7
    } else if off < OFF_PEG {
        8
    } else if off < OFF_MOD_ENV {
        9
    } else if off < OFF_ASSIGN {
        10
    } else if off < OFF_STACK {
        11
    } else if off < OFF_MTX {
        12
    } else if off < N_PER_PATCH {
        13
    } else {
        return "";
    };
    PER_PATCH_MODULES[section]
}

fn module_for_patch(off: usize) -> &'static str {
    if off < OFF_DELAY {
        "Global / LFO 1"
    } else if off < OFF_REVERB {
        "Global / Delay"
    } else if off < OFF_MASTER {
        "Global / Reverb"
    } else if off < OFF_FILTER {
        "Global / Master"
    } else if off < N_PATCH_LEVEL {
        "Global / Filter"
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
        assert_eq!(TOTAL_PARAMS, 188);
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
    fn no_upper_or_lower_prefix() {
        for d in PARAMS.iter() {
            assert!(!d.id.starts_with("upper-"), "stale upper- prefix: {}", d.id);
            assert!(!d.id.starts_with("lower-"), "stale lower- prefix: {}", d.id);
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
    fn filter_section_is_at_table_tail() {
        // The Filter section (7 params, E007) is appended after Master at the
        // very end of the flat space, so blob v6→v7 migration is a 1:1 prefix.
        let tune = id_of("master-tune").expect("master-tune");
        let vol = id_of("master-volume").expect("master-volume");
        assert_eq!(tune, TOTAL_PARAMS - 11);
        assert_eq!(vol, TOTAL_PARAMS - 10);
        assert_eq!(id_of("filter-enable"), Some(TOTAL_PARAMS - 9));
        assert_eq!(id_of("filter-cutoff"), Some(TOTAL_PARAMS - 8));
        assert_eq!(id_of("filter-resonance"), Some(TOTAL_PARAMS - 7));
        assert_eq!(id_of("filter-mode"), Some(TOTAL_PARAMS - 6));
        assert_eq!(id_of("filter-slope"), Some(TOTAL_PARAMS - 5));
        assert_eq!(id_of("filter-drive"), Some(TOTAL_PARAMS - 4));
        assert_eq!(id_of("filter-oversample"), Some(TOTAL_PARAMS - 3));
        assert_eq!(id_of("filter-keytrack"), Some(TOTAL_PARAMS - 2));
        assert_eq!(id_of("filter-cutoff-tuned"), Some(TOTAL_PARAMS - 1));
        // `filter-enable` defaults off → migrated patches stay bit-identical.
        assert_eq!(PARAMS[id_of("filter-enable").unwrap()].default, 0.0);
    }

    #[test]
    fn patch_block_begins_at_patch_base() {
        let lfo1 = id_of("lfo1-shape").expect("lfo1-shape");
        assert_eq!(lfo1, PATCH_BASE);
    }

    #[test]
    fn desc_for_clap_id_is_o1_and_total_bounded() {
        assert!(desc_for_clap_id(0).is_some());
        assert!(desc_for_clap_id(TOTAL_PARAMS - 1).is_some());
        assert!(desc_for_clap_id(TOTAL_PARAMS).is_none());
    }

    #[test]
    fn module_for_clap_id_routes_each_section() {
        let cases = [
            ("op1-num", "Op 1"),
            ("op6-pan", "Op 6"),
            ("algo", "Algorithm"),
            ("feedback", "Feedback"),
            ("lfo2-shape", "LFO 2"),
            ("peg-r1", "PEG"),
            ("mod-env-a", "Mod Env"),
            ("assign-mode", "Assign"),
            ("stack-density", "Stack"),
            ("mtx1-depth", "Matrix"),
            ("lfo1-shape", "Global / LFO 1"),
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

        let algo = desc(id_of("algo").unwrap()).unwrap();
        assert_eq!(algo.display(5.0), "5");
        assert_eq!(algo.display(32.4), "32");

        let lfo1 = desc(id_of("lfo1-shape").unwrap()).unwrap();
        assert_eq!(lfo1.display(0.0), "Sine");
        assert_eq!(lfo1.display(4.0), "Pulse");
        assert_eq!(lfo1.display(99.0), "S&H");

        let legato = desc(id_of("legato").unwrap()).unwrap();
        assert_eq!(legato.display(0.0), "Off");
        assert_eq!(legato.display(1.0), "On");

        let detune = desc(id_of("op1-detune").unwrap()).unwrap();
        assert_eq!(detune.display(0.0), "0 ct");
        assert_eq!(detune.display(-50.0), "-50 ct");

        let stack_detune = desc(id_of("stack-detune").unwrap()).unwrap();
        assert_eq!(stack_detune.display(8.0), "8.00 ct");

        let fine = desc(id_of("op1-fine").unwrap()).unwrap();
        assert_eq!(fine.display(0.0), "0");
        assert_eq!(fine.display(50.0), "50");

        let num = desc(id_of("op1-num").unwrap()).unwrap();
        assert_eq!(num.display(1.0), "1");
        assert_eq!(num.display(32.0), "32");

        let denom = desc(id_of("op1-denom").unwrap()).unwrap();
        assert_eq!(denom.display(1.0), "1");
        assert_eq!(denom.display(8.0), "8");
    }

    #[test]
    fn range_fidelity_spot_checks() {
        let cases: &[(&str, f32, f32, f32)] = &[
            ("op3-num", 1.0, 32.0, 1.0),
            ("op3-denom", 1.0, 8.0, 1.0),
            ("algo", 1.0, 32.0, 5.0),
            ("lfo2-shape", 0.0, 5.0, 2.0),
            ("mod-env-shape", 0.0, 1.0, 0.0),
            ("stack-distrib", 0.0, 2.0, 0.0),
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
