//! UI / host / view events.
//!
//! Shared variants come from `vxn-core-app`; per-synth variants
//! (`SetKeyMode`, `ResetLayer`, etc.) ride the `Custom` payload via
//! [`Vxn1UiCustom`] / [`Vxn1ViewCustom`]. Re-exports keep
//! `vxn_app::{UiEvent, ViewEvent, HostEvent, PresetSource}` working
//! for code that already imports these names.

pub use vxn_core_app::{HostEvent, PresetSource, UiEvent, ViewEvent};

use crate::domain::{KeyMode, Layer};

/// Per-synth UI event payloads carried inside [`UiEvent::Custom`].
#[derive(Clone, Debug)]
pub enum Vxn1UiCustom {
    ResetLayer { layer: Layer },
    SetKeyMode { mode: KeyMode },
    SetSplitPoint { note: u8 },
    SetEditLayer { layer: Layer },
}

impl Vxn1UiCustom {
    /// Wrap as `UiEvent::Custom(Box<Self>)`.
    #[inline]
    pub fn into_event(self) -> UiEvent {
        UiEvent::Custom(Box::new(self))
    }
}

/// Per-synth view event payloads carried inside [`ViewEvent::Custom`].
#[derive(Clone, Debug)]
pub enum Vxn1ViewCustom {
    KeyModeChanged { mode: KeyMode },
    SplitPointChanged { note: u8 },
    EditLayerChanged { layer: Layer },
}

impl Vxn1ViewCustom {
    /// Wrap as `ViewEvent::Custom(Box<Self>)`.
    #[inline]
    pub fn into_event(self) -> ViewEvent {
        ViewEvent::Custom(Box::new(self))
    }
}
