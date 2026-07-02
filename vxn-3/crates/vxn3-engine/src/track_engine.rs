//! The `TrackEngine` trait — the load-bearing abstraction (ADR 0001 §4/§5).
//!
//! A track holds **one** active engine behind a `Box<dyn TrackEngine>`. Dispatch
//! is **per block, per track** — one vtable call into [`TrackEngine::render`],
//! which then runs its own monomorphic SoA lane loop with *no* further dispatch
//! (the vxn-1/vxn-2 "no enum match inside the lane loop" lesson). What a lane
//! *means* is the engine's choice: voices for a poly engine, modes for a
//! resonator (0049). Same trait, different voicing — so the resonator slots in
//! without reshaping this surface.
//!
//! Sample accuracy is the *host's* job, not a per-sample parameter here: the
//! instrument [`crate::engine::Engine`] slices each block at trig boundaries and
//! calls `render` on the contiguous sub-spans, with [`TrackEngine::on_trig`]
//! between them. So `render` only ever sees a plain contiguous span and an
//! engine never needs to reason about frame offsets.

/// Lane budget ceiling. A poly engine uses lanes as voices (≤ 4, the agreed
/// poly cap → one NEON `f32x4`); a resonator uses them as modes. Engines store
/// their SoA state in `[_; LANES]` arrays.
pub const LANES: usize = 4;

/// The per-track voice/resonator engine.
///
/// `Send` so a freshly-built engine can be handed from the main thread to the
/// audio thread over the [`crate::swap`] channel.
pub trait TrackEngine: Send {
    /// Render `out.len()` mono samples, **overwriting** the span, advancing
    /// voice/resonator state. Allocation-free.
    fn render(&mut self, out: &mut [f32]);

    /// Trigger the engine. Poly: allocate/steal a voice at `note` (equal-tempered
    /// MIDI, fractional allowed) and `velocity` (0..1). Resonator: inject
    /// excitation into the persistent state. Called by the host between render
    /// sub-spans, so it is sample-accurate.
    fn on_trig(&mut self, note: f32, velocity: f32);

    /// Silence all voices / collapse decaying state (transport stop, reset).
    fn reset(&mut self);

    /// Re-cook sample-rate-dependent coefficients.
    fn set_sample_rate(&mut self, sample_rate: f32);

    /// A short identifier for the active engine kind (UI / introspection / swap
    /// assertions). Stable across instances of the same engine.
    fn kind(&self) -> EngineKind;

    /// Number of generic macro slots this engine maps (≤ [`MACRO_SLOTS`]). The
    /// host declares a fixed budget (ADR 0003 §2); slots ≥ this render inert.
    fn macro_count(&self) -> usize {
        MACRO_SLOTS
    }

    /// Apply a normalised (`0..1`) value to macro `slot`, reinterpreted onto this
    /// engine's own patch (ADR 0003 §2). Out-of-range slots are ignored; the
    /// default is a no-op. The value formula per `(kind, slot)` is shared with
    /// [`macro_display`] via [`macro_map`], so the readout always matches what
    /// was set. Keeps the host macro surface uniform without a per-engine table.
    fn set_macro(&mut self, _slot: usize, _value: f32) {}
}

/// Fixed budget of generic host-facing macro slots per track (ADR 0003 §2). Each
/// engine reinterprets slot `0..MACRO_SLOTS` onto its patch; the slot's *id/name*
/// is generic and host-fixed while its *meaning + readout* are engine-defined.
pub const MACRO_SLOTS: usize = 3;

/// The physical unit a macro slot maps to, for engine-aware value-text.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MacroUnit {
    Seconds,
    Semitones,
    Hertz,
    Percent,
}

/// A macro slot's engine-aware readout: the mapped physical `value` plus how to
/// `label` and unit-format it. Pure — computed from `(kind, slot, norm)` with no
/// engine instance, so it is callable on the **main thread** for CLAP
/// `value_to_text` (0172) where the live engine sits on the audio thread.
#[derive(Copy, Clone, Debug)]
pub struct MacroReadout {
    pub value: f32,
    pub label: &'static str,
    pub unit: MacroUnit,
}

/// Map a normalised (`0..1`) macro slot value to its engine-specific physical
/// value + label. `None` if the engine doesn't map that slot. **The single
/// source of truth** shared by each engine's `set_macro` (which assigns
/// `value` to the matching patch field) and [`macro_display`] (which formats it),
/// so a slot's readout can never drift from what it sets.
pub fn macro_map(kind: EngineKind, slot: usize, norm: f32) -> Option<MacroReadout> {
    use EngineKind::*;
    use MacroUnit::*;
    let v = norm.clamp(0.0, 1.0);
    let (value, label, unit) = match (kind, slot) {
        // Kick/Tone: body length, "donk" sweep length, pitch-sweep depth.
        (KickTone, 0) => (0.05 + v * 1.45, "Decay", Seconds), // 50 ms .. 1.5 s
        (KickTone, 1) => (0.005 + v * 0.195, "Donk", Seconds), // 5 .. 200 ms
        (KickTone, 2) => (v * 48.0, "Depth", Semitones),      // 0 .. 4 oct
        // Metal: open-ring length, excitation energy, body pitch.
        (Metal, 0) => (0.1 + v * 2.9, "Ring", Seconds),   // 100 ms .. 3 s
        (Metal, 1) => (0.1 + v * 0.9, "Excite", Percent), // 10 .. 100 %
        (Metal, 2) => (400.0 + v * 2_600.0, "Body", Hertz), // 400 .. 3000 Hz
        // Noise: burst length, noise↔body mix, output brightness.
        (Noise, 0) => (0.02 + v * 0.48, "Decay", Seconds), // 20 ms .. 0.5 s
        (Noise, 1) => (v, "Mix", Percent),                 // 0 .. 100 %
        (Noise, 2) => (400.0 + v * 7_600.0, "Bright", Hertz), // 400 .. 8000 Hz
        _ => return None,
    };
    Some(MacroReadout { value, label, unit })
}

/// Format macro `slot`'s value engine-aware (e.g. "Decay 0.42 s", "Body 1.80 kHz",
/// "Excite 65%"). Pure + allocation-free into the caller's writer; renders "—"
/// for a slot the engine doesn't map. The shared formatter for CLAP
/// `value_to_text` (0172) and any faceplate readout.
pub fn macro_display(
    kind: EngineKind,
    slot: usize,
    norm: f32,
    out: &mut impl core::fmt::Write,
) -> core::fmt::Result {
    let Some(r) = macro_map(kind, slot, norm) else {
        return out.write_str("—");
    };
    match r.unit {
        MacroUnit::Seconds if r.value < 1.0 => write!(out, "{} {:.0} ms", r.label, r.value * 1e3),
        MacroUnit::Seconds => write!(out, "{} {:.2} s", r.label, r.value),
        MacroUnit::Semitones => write!(out, "{} {:.1} st", r.label, r.value),
        MacroUnit::Hertz if r.value >= 1_000.0 => {
            write!(out, "{} {:.2} kHz", r.label, r.value / 1e3)
        }
        MacroUnit::Hertz => write!(out, "{} {:.0} Hz", r.label, r.value),
        MacroUnit::Percent => write!(out, "{} {:.0}%", r.label, r.value * 100.0),
    }
}

/// The closed engine roster (ADR 0001 §6). `Metal` / `Noise` land in 0049.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EngineKind {
    KickTone,
    Metal,
    Noise,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show(kind: EngineKind, slot: usize, v: f32) -> String {
        let mut s = String::new();
        macro_display(kind, slot, v, &mut s).unwrap();
        s
    }

    #[test]
    fn every_engine_maps_all_slots() {
        // No inert slot on any engine (the 0072 dead-mapping fix): all three
        // roster engines map slot 0..MACRO_SLOTS to a real, labelled control.
        for kind in [EngineKind::KickTone, EngineKind::Metal, EngineKind::Noise] {
            for slot in 0..MACRO_SLOTS {
                assert!(macro_map(kind, slot, 0.5).is_some(), "{kind:?} slot {slot} inert");
            }
        }
    }

    #[test]
    fn display_is_engine_aware() {
        // Same generic slot, different engine → different readout (ADR 0003 §2).
        assert!(show(EngineKind::KickTone, 0, 0.5).starts_with("Decay"));
        assert!(show(EngineKind::Metal, 0, 0.5).starts_with("Ring"));
        assert!(show(EngineKind::Noise, 2, 0.5).starts_with("Bright"));
        // Slot beyond the budget renders a sentinel, not garbage.
        assert_eq!(show(EngineKind::KickTone, MACRO_SLOTS, 0.5), "—");
    }

    #[test]
    fn map_is_monotonic_and_clamped() {
        // Value formula is shared with set_macro; check ends + clamp.
        let lo = macro_map(EngineKind::KickTone, 0, -1.0).unwrap().value;
        let hi = macro_map(EngineKind::KickTone, 0, 2.0).unwrap().value;
        assert!((lo - 0.05).abs() < 1e-6, "clamped low end");
        assert!((hi - 1.5).abs() < 1e-6, "clamped high end");
    }

    #[test]
    fn unit_formatting() {
        assert_eq!(show(EngineKind::KickTone, 1, 0.0), "Donk 5 ms"); // sub-second → ms
        assert_eq!(show(EngineKind::Noise, 1, 1.0), "Mix 100%");
        assert_eq!(show(EngineKind::Metal, 2, 1.0), "Body 3.00 kHz"); // ≥1k → kHz
    }
}
