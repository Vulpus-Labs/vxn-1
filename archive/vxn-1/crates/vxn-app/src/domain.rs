//! Domain enums shared across the controller, engine, and editors.
//!
//! These live in `vxn-app` (not `vxn-engine`) so the trait crate stays free
//! of any engine dependency — the source-of-truth direction the ADR mandates
//! (`vxn-engine` depends on `vxn-app`, not the reverse). `vxn-engine`
//! re-exports them so existing call sites keep their paths.

/// Which of the two always-present patches a per-patch param belongs to.
/// Discriminant doubles as the index into the engine's per-layer arrays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Layer {
    Upper = 0,
    Lower = 1,
}

impl Layer {
    pub const COUNT: usize = 2;
    pub const ALL: [Layer; Self::COUNT] = [Layer::Upper, Layer::Lower];
}

/// Key mode. Non-automatable shared state (ADR 0003 §3): it travels
/// in the plugin-state blob, not the CLAP param table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum KeyMode {
    #[default]
    Whole = 0,
    Dual = 1,
    Split = 2,
}

impl KeyMode {
    pub const COUNT: usize = 3;
    pub const ALL: [KeyMode; Self::COUNT] = [KeyMode::Whole, KeyMode::Dual, KeyMode::Split];

    pub fn from_u8(v: u8) -> KeyMode {
        match v {
            1 => KeyMode::Dual,
            2 => KeyMode::Split,
            _ => KeyMode::Whole,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            KeyMode::Whole => "Whole",
            KeyMode::Dual => "Dual",
            KeyMode::Split => "Split",
        }
    }

    pub fn from_label(label: &str) -> Option<KeyMode> {
        KeyMode::ALL
            .into_iter()
            .find(|m| m.label().eq_ignore_ascii_case(label))
    }
}

/// Default split point (MIDI note) when none has been set — middle C.
pub const DEFAULT_SPLIT_POINT: u8 = 60;

/// Display label for the virtual root group of the user preset corpus: presets
/// living directly under the user preset dir, not in a real subfolder.
pub const UNCATEGORIZED: &str = "Uncategorised";

pub use vxn_core_app::PresetMeta;
