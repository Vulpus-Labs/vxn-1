//! VXN3 HTML faceplate: bundles the page assets and supplies the
//! `parse_custom_ui` / `serialise_custom_view` hooks that map the grid's
//! structured edits to [`Vxn3UiCustom`] and the playhead to the page. Wraps
//! `vxn-core-ui-web`'s wry host (ticket 0052).

use std::any::Any;
use std::ffi::c_void;
use std::sync::Arc;

use serde_json::Value as Json;
use vxn_core_app::{ControllerHandle, CorpusHandle, UiEvent};
use vxn_core_ui_web::{DEFAULT_MAX_BATCH_BYTES, WebEditorConfig, open_editor as core_open_editor};
// Re-exported so the clack shell can name the editor handle / error.
pub use vxn_core_ui_web::{EditorHandle, OpenEditorError};
use vxn3_app::{Vxn3UiCustom, Vxn3ViewCustom};
use vxn3_engine::sequencer::{Retrig, RetrigCurve};
use vxn3_engine::track_engine::EngineKind;
use vxn3_engine::{EngineCommand, MAX_STEPS, N_TRACKS};

pub const EDITOR_WIDTH: u32 = 900;
pub const EDITOR_HEIGHT: u32 = 420;

const HTML_TEMPLATE: &str = include_str!("../assets/index.html");
const APP_JS: &str = include_str!("../assets/app.js");
const STYLE_CSS: &str = include_str!("../assets/style.css");

/// Open the VXN3 faceplate under `parent` (the raw NSView/HWND/xcb handle the
/// clack shell extracts in `gui::set_parent`). Never panics — a bad parent or
/// wry build failure returns `OpenEditorError`, which the shell maps to
/// `PluginError`.
pub fn open_editor(
    parent: *mut c_void,
    ctrl: ControllerHandle,
    corpus: CorpusHandle,
) -> Result<EditorHandle, OpenEditorError> {
    let mut config = WebEditorConfig::new(build_html(), EDITOR_WIDTH, EDITOR_HEIGHT);
    config.max_batch_bytes = DEFAULT_MAX_BATCH_BYTES;
    config.webview2_vendor = Some("Vulpus");
    config.webview2_product = Some("VXN3");
    config.parse_custom_ui = Some(Arc::new(parse_custom_ui));
    config.serialise_custom_view = Some(Arc::new(serialise_custom_view));
    core_open_editor(parent, ctrl, corpus, config)
}

/// Splice CSS, the config JSON, and the app JS into the HTML template.
fn build_html() -> String {
    let config = serde_json::json!({
        "tracks": N_TRACKS,
        "steps": MAX_STEPS,
        "engines": [
            { "id": "kick", "label": "Kick" },
            { "id": "metal", "label": "Metal" },
            { "id": "noise", "label": "Noise" },
        ],
    });
    HTML_TEMPLATE
        .replace("__CSS__", STYLE_CSS)
        .replace("__CONFIG_JSON__", &config.to_string())
        .replace("__APP_JS__", APP_JS)
}

fn u8_at(v: &Json, key: &str) -> Option<u8> {
    Some(v.get(key)?.as_u64()? as u8)
}
fn f32_at(v: &Json, key: &str) -> Option<f32> {
    Some(v.get(key)?.as_f64()? as f32)
}

fn kind_of(s: &str) -> Option<EngineKind> {
    match s {
        "kick" => Some(EngineKind::KickTone),
        "metal" => Some(EngineKind::Metal),
        "noise" => Some(EngineKind::Noise),
        _ => None,
    }
}
fn curve_of(s: &str) -> RetrigCurve {
    match s {
        "accel" => RetrigCurve::Accel,
        "decel" => RetrigCurve::Decel,
        _ => RetrigCurve::Even,
    }
}

#[inline]
fn edit(cmd: EngineCommand) -> UiEvent {
    UiEvent::Custom(Box::new(Vxn3UiCustom::Edit(cmd)))
}

/// Map a faceplate opcode to a [`Vxn3UiCustom`] UI event. Unknown opcodes return
/// `None` (the core then tries its built-in vocabulary).
fn parse_custom_ui(op: &str, v: &Json) -> Option<UiEvent> {
    // Master-bus ops carry no track.
    match op {
        "set_delay_feedback" => {
            return Some(edit(EngineCommand::SetDelayFeedback {
                value: f32_at(v, "value")?,
            }));
        }
        "set_delay_sync" => {
            return Some(edit(EngineCommand::SetDelaySyncBeats {
                beats: f32_at(v, "beats")?,
            }));
        }
        "set_delay_return" => {
            return Some(edit(EngineCommand::SetDelayReturn {
                value: f32_at(v, "value")?,
            }));
        }
        _ => {}
    }
    let track = u8_at(v, "track")?;
    match op {
        "set_send" => Some(edit(EngineCommand::SetSend {
            track,
            amount: f32_at(v, "amount")?,
        })),
        "toggle_step" => Some(edit(EngineCommand::ToggleStep {
            track,
            step: u8_at(v, "step")?,
        })),
        "set_step" => Some(edit(EngineCommand::SetStep {
            track,
            step: u8_at(v, "step")?,
            note: f32_at(v, "note")?,
            velocity: f32_at(v, "velocity")?,
        })),
        "set_probability" => Some(edit(EngineCommand::SetProbability {
            track,
            step: u8_at(v, "step")?,
            probability: f32_at(v, "probability")?,
        })),
        "set_retrig" => Some(edit(EngineCommand::SetRetrig {
            track,
            step: u8_at(v, "step")?,
            retrig: Retrig {
                n: u8_at(v, "n")?,
                m: u8_at(v, "m")?,
                curve: curve_of(v.get("curve").and_then(|c| c.as_str()).unwrap_or("even")),
                vel_end: f32_at(v, "vel_end").unwrap_or(1.0),
            },
        })),
        "set_length" => Some(edit(EngineCommand::SetLength {
            track,
            len: u8_at(v, "len")?,
        })),
        "set_step_beats" => Some(edit(EngineCommand::SetStepBeats {
            track,
            beats: f32_at(v, "beats")?,
        })),
        "set_gain" => Some(edit(EngineCommand::SetGain {
            track,
            gain: f32_at(v, "gain")?,
        })),
        "set_pan" => Some(edit(EngineCommand::SetPan {
            track,
            pan: f32_at(v, "pan")?,
        })),
        "set_macro" => Some(edit(EngineCommand::SetMacro {
            track,
            slot: u8_at(v, "slot")?,
            value: f32_at(v, "value")?,
        })),
        "set_engine" => Some(UiEvent::Custom(Box::new(Vxn3UiCustom::SetEngine {
            track,
            kind: kind_of(v.get("kind")?.as_str()?)?,
        }))),
        _ => None,
    }
}

/// Serialise a [`Vxn3ViewCustom`] for the page.
fn serialise_custom_view(payload: &dyn Any) -> Option<Json> {
    let custom = payload.downcast_ref::<Vxn3ViewCustom>()?;
    match custom {
        Vxn3ViewCustom::Playhead { steps, playing } => Some(serde_json::json!({
            "kind": "playhead",
            "steps": steps.to_vec(),
            "playing": playing,
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(s: &str) -> Json {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn parses_toggle_step() {
        let ev = parse_custom_ui("toggle_step", &obj(r#"{"track":2,"step":5}"#)).unwrap();
        match ev {
            UiEvent::Custom(b) => match *b.downcast::<Vxn3UiCustom>().unwrap() {
                Vxn3UiCustom::Edit(EngineCommand::ToggleStep { track, step }) => {
                    assert_eq!((track, step), (2, 5));
                }
                _ => panic!("wrong variant"),
            },
            _ => panic!("not custom"),
        }
    }

    #[test]
    fn parses_set_engine() {
        let ev = parse_custom_ui("set_engine", &obj(r#"{"track":1,"kind":"metal"}"#)).unwrap();
        match ev {
            UiEvent::Custom(b) => match *b.downcast::<Vxn3UiCustom>().unwrap() {
                Vxn3UiCustom::SetEngine { track, kind } => {
                    assert_eq!(track, 1);
                    assert_eq!(kind, EngineKind::Metal);
                }
                _ => panic!("wrong variant"),
            },
            _ => panic!("not custom"),
        }
    }

    #[test]
    fn unknown_op_is_none() {
        assert!(parse_custom_ui("explode", &obj(r#"{"track":0}"#)).is_none());
    }

    #[test]
    fn html_has_assets_spliced() {
        let html = build_html();
        assert!(html.contains("VXN3"));
        assert!(!html.contains("__CSS__"));
        assert!(!html.contains("__APP_JS__"));
        assert!(!html.contains("__CONFIG_JSON__"));
        assert!(html.contains("\"tracks\":8"));
    }

    #[test]
    fn serialises_playhead() {
        let mut steps = [u32::MAX; N_TRACKS];
        steps[0] = 3;
        let j = serialise_custom_view(&Vxn3ViewCustom::Playhead { steps, playing: true }).unwrap();
        assert_eq!(j["kind"], "playhead");
        assert_eq!(j["playing"], true);
        assert_eq!(j["steps"][0], 3);
    }
}
