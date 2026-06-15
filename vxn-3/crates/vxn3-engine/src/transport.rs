//! Host transport clock — the time-base the pattern sequencer consumes.
//!
//! The CLAP shell reads this from the host once per process block and hands it
//! to the engine via [`Engine::set_transport`]. At the 0046 skeleton stage
//! nothing consumes it yet (the engine renders silence); the sequencer in
//! 0047 / 0048 reads `playing` + `song_pos_beats` to schedule sample-accurate
//! trigs against the host clock, rather than block-quantising them.

/// A snapshot of the host transport for one process block.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Transport {
    /// Host is rolling (CLAP `IS_PLAYING`). When `false`, the sequencer holds
    /// its position.
    pub playing: bool,
    /// Tempo in quarter-note beats per minute. Falls back to 120 when the host
    /// supplies no tempo (`HAS_TEMPO` clear).
    pub tempo_bpm: f64,
    /// Song position in quarter-note beats (CLAP `song_pos_beats`). `None` when
    /// the host exposes no beats timeline (`HAS_BEATS_TIMELINE` clear) — the
    /// sequencer then free-runs from its own accumulated position.
    pub song_pos_beats: Option<f64>,
}

impl Default for Transport {
    fn default() -> Self {
        Self {
            playing: false,
            tempo_bpm: 120.0,
            song_pos_beats: None,
        }
    }
}
