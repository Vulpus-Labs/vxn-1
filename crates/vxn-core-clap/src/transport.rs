//! Host transport helpers.

use clack_plugin::events::event_types::{TransportEvent, TransportFlags};

/// Pull the BPM out of a CLAP transport event, or `None` when the host
/// hasn't supplied one (`HAS_TEMPO` flag clear).
pub fn tempo_from_transport(t: &TransportEvent) -> Option<f64> {
    if t.flags.contains(TransportFlags::HAS_TEMPO) {
        Some(t.tempo)
    } else {
        None
    }
}
