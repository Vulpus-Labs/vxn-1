//! VXN3 HTML faceplate: bundles the page assets and supplies the
//! `parse_custom_ui` / `serialise_custom_view` hooks that map the lane editor's
//! structured edits to [`Vxn3UiCustom`] and the playhead to the page. Wraps
//! `vxn-core-ui-web`'s wry host (ticket 0052).
//!
//! The editing surface is the **continuous lane strip** of ADR 0007 §1 (0353):
//! one rectangular strip per track with X as time and Y as a modulation value,
//! hits as freely-draggable diamonds. The grid is drawn, not stored into, so the
//! page needs the lane's real geometry rather than a step count — [`build_html`]
//! ships each lane's beat markers, per-beat subdivision counts and swing warp,
//! and the page places every marker through that geometry (never through a
//! pixels-per-step multiplication, which the swung grid would make a lie).
//!
//! The snapped-slot opcodes are kept: [`vxn3_engine::Pattern`] still has the
//! slot-keyed verbs, and they remain the right vocabulary for a caller that has
//! a slot index. The strip does not use them — it is hit-keyed throughout, `hit`
//! being a fire-order index into the lane.
//!
//! Which leaves one gap worth naming here rather than only in the page: the hit
//! index is only meaningful while the page's list matches the engine's, and
//! [`serialise_custom_view`] carries the playhead and nothing else. The page is
//! therefore seeded empty, which is right for a fresh instance and wrong for an
//! editor reopened over a lane that already holds hits. A hit-list readback is
//! the fix and is its own ticket; see the matching note in `app.js`.

use std::any::Any;
use std::ffi::c_void;
use std::sync::Arc;

use serde_json::Value as Json;
use vxn_core_app::{ControllerHandle, CorpusHandle, UiEvent};
use vxn_core_ui_web::{DEFAULT_MAX_BATCH_BYTES, WebEditorConfig, open_editor as core_open_editor};
// Re-exported so the clack shell can name the editor handle / error.
pub use vxn_core_ui_web::{EditorHandle, OpenEditorError};
use vxn3_app::{Vxn3UiCustom, Vxn3ViewCustom};
use vxn3_engine::flavour::{Binding, Curve, Flavour};
use vxn3_engine::sequencer::{Retrig, RetrigCurve, Y_CENTRE};
use vxn3_engine::track_engine::{EngineKind, MACRO_SLOTS};
use vxn3_engine::{
    EngineCommand, Grid, MAX_BEATS, MAX_HITS, MAX_NUDGE_TICKS, MAX_SUBS, N_TRACKS, TICKS_PER_BEAT,
    flavours_for, params_for,
};

pub const EDITOR_WIDTH: u32 = 900;
pub const EDITOR_HEIGHT: u32 = 420;

/// Pitch a hit takes when a payload carries none — C2, [`vxn3_engine::Hit`]'s own
/// default. See the `add_hit` arm of [`parse_custom_ui`] for why this defaults
/// rather than rejecting.
const DEFAULT_NOTE: f32 = 36.0;

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

fn curve_str(c: Curve) -> &'static str {
    match c {
        Curve::Linear => "linear",
        Curve::Exp => "exp",
    }
}

/// A flavour as faceplate JSON: name + base vector + binding table + macro defaults.
/// The editor renders base sliders + binding rows straight from this (0185).
fn flavour_json(name: &str, f: &Flavour) -> Json {
    let bindings: Vec<Json> = f
        .bindings
        .iter()
        .map(|b| {
            serde_json::json!({ "slot": b.slot, "param": b.param, "depth": b.depth, "curve": curve_str(b.curve) })
        })
        .collect();
    serde_json::json!({
        "name": name,
        "base": f.base,
        "bindings": bindings,
        "macro_defaults": f.macro_defaults,
    })
}

/// One engine's faceplate config: id/label + its family param-space metadata + the
/// authored flavours (full data, so the picker + editor render locally).
fn engine_json(id: &str, label: &str, kind: EngineKind) -> Json {
    let params: Vec<Json> = params_for(kind)
        .iter()
        .map(|p| {
            serde_json::json!({ "name": p.name, "unit": p.unit.symbol(), "min": p.min, "max": p.max, "default": p.default })
        })
        .collect();
    let flavours: Vec<Json> = flavours_for(kind).iter().map(|(n, f)| flavour_json(n, f)).collect();
    serde_json::json!({ "id": id, "label": label, "params": params, "flavours": flavours })
}

/// One lane's timing **geometry** as faceplate JSON (ADR 0007 §2).
///
/// Everything the page needs to place a marker itself: the stored beat markers,
/// each beat's resolved subdivision count (and whether that count is an override
/// or the lane default, so a sub-count edit does not silently erase a tuplet),
/// and the swing warp as its tag encoding. Subdivision markers are *derived* and
/// deliberately not sent — they are unevenly spaced under swing, and a list of
/// them would go stale the moment the lane's geometry changed.
fn grid_json(g: &Grid) -> Json {
    let markers: Vec<f64> = (0..=g.n_beats()).map(|i| g.beat_marker(i)).collect();
    let subs: Vec<u32> = (0..g.n_beats()).map(|b| g.subs(b)).collect();
    let overrides: Vec<u32> = (0..g.n_beats()).map(|b| g.sub_override(b).unwrap_or(0)).collect();
    let sw = g.swing();
    serde_json::json!({
        "n_beats": g.n_beats(),
        "len_beats": g.len_beats(),
        "markers": markers,
        "subs": subs,
        "sub_override": overrides,
        "default_subs": g.default_subs(),
        "swing": { "shape": sw.shape.as_u8(), "amount": sw.amount, "period": sw.period.as_u8() },
    })
}

/// Splice CSS, the config JSON, and the app JS into the HTML template.
pub fn build_html() -> String {
    // Per-lane geometry, not one global grid: lanes are independently subdivided
    // and independently long (polymeter, ADR 0001 §2), and the strip draws each
    // one's own markers. They start identical; the page diverges them as the user
    // edits, and the shape is per-lane from the first frame so nothing has to be
    // rebuilt when they do.
    let lanes: Vec<Json> = (0..N_TRACKS).map(|_| grid_json(&Grid::default())).collect();
    let config = serde_json::json!({
        "tracks": N_TRACKS,
        "lanes": lanes,
        "macro_slots": MACRO_SLOTS,
        // Editor-side limits: the strip enforces the hit ceiling itself, with
        // visible feedback, rather than letting an over-capacity add be dropped
        // silently on the audio thread.
        "max_hits": MAX_HITS,
        "max_beats": MAX_BEATS,
        "max_subs": MAX_SUBS,
        "ticks_per_beat": TICKS_PER_BEAT,
        "max_nudge_ticks": MAX_NUDGE_TICKS,
        "y_centre": Y_CENTRE,
        "engines": [
            engine_json("kick", "Kick", EngineKind::KickTone),
            engine_json("metal", "Metal", EngineKind::Metal),
            engine_json("noise", "Noise", EngineKind::Noise),
            engine_json("struck", "Struck", EngineKind::Struck),
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
fn u16_at(v: &Json, key: &str) -> Option<u16> {
    Some(v.get(key)?.as_u64()? as u16)
}
fn f32_at(v: &Json, key: &str) -> Option<f32> {
    Some(v.get(key)?.as_f64()? as f32)
}
/// A signed field — `nudge` is the only one, and it is the half of a hit's
/// position that does *not* scale with the slot, so it must survive as a signed
/// tick count rather than being read through the unsigned helpers above.
fn i16_at(v: &Json, key: &str) -> Option<i16> {
    Some(v.get(key)?.as_i64()? as i16)
}

fn kind_of(s: &str) -> Option<EngineKind> {
    match s {
        "kick" => Some(EngineKind::KickTone),
        "metal" => Some(EngineKind::Metal),
        "noise" => Some(EngineKind::Noise),
        "struck" => Some(EngineKind::Struck),
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
fn flavour_curve_of(s: &str) -> Curve {
    match s {
        "exp" => Curve::Exp,
        _ => Curve::Linear,
    }
}

fn f32_array(v: &Json, key: &str) -> Vec<f32> {
    v.get(key)
        .and_then(|x| x.as_array())
        .map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect())
        .unwrap_or_default()
}

/// Parse an `assign_voice` payload's flavour (base + bindings + macro defaults + names).
fn parse_flavour(v: &Json) -> Option<Flavour> {
    let base = f32_array(v, "base");
    if base.is_empty() {
        return None;
    }
    let mut macro_defaults = [0.5_f32; MACRO_SLOTS];
    for (i, x) in f32_array(v, "macro_defaults").iter().take(MACRO_SLOTS).enumerate() {
        macro_defaults[i] = *x;
    }
    let mut macro_names: [String; MACRO_SLOTS] = Default::default();
    if let Some(arr) = v.get("macro_names").and_then(|x| x.as_array()) {
        for (i, x) in arr.iter().take(MACRO_SLOTS).enumerate() {
            if let Some(s) = x.as_str() {
                macro_names[i] = s.to_string();
            }
        }
    }
    let mut bindings = Vec::new();
    if let Some(arr) = v.get("bindings").and_then(|x| x.as_array()) {
        for b in arr {
            bindings.push(Binding {
                slot: u8_at(b, "slot")?,
                param: u8_at(b, "param")?,
                depth: f32_at(b, "depth")?,
                curve: flavour_curve_of(b.get("curve").and_then(|x| x.as_str()).unwrap_or("linear")),
            });
        }
    }
    Some(Flavour { base, bindings, macro_defaults, macro_names })
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
    // The lane strip's vocabulary (0353). Hit-keyed: `hit` is a fire-order index,
    // which the page mirrors by resolving positions through the same geometry the
    // engine does.
    match op {
        // Everything but the position defaults. An `add_hit` must never be
        // *dropped* for a missing attribute: the page has already drawn the
        // diamond and counted it, so a rejected add puts the two hit lists a
        // whole index out of step for good, where a wrong note is one audibly
        // wrong drum until the lane is reassigned.
        "add_hit" => {
            return Some(edit(EngineCommand::AddHit {
                track,
                beat: u16_at(v, "beat")?,
                sub: u8_at(v, "sub")?,
                f: f32_at(v, "f").unwrap_or(0.0),
                nudge: i16_at(v, "nudge").unwrap_or(0),
                y: f32_at(v, "y").unwrap_or(Y_CENTRE),
                note: f32_at(v, "note").unwrap_or(DEFAULT_NOTE),
                velocity: f32_at(v, "velocity").unwrap_or(1.0),
            }));
        }
        "remove_hit" => {
            return Some(edit(EngineCommand::RemoveHit {
                track,
                hit: u16_at(v, "hit")?,
            }));
        }
        "set_hit_position" => {
            return Some(edit(EngineCommand::SetHitPosition {
                track,
                hit: u16_at(v, "hit")?,
                beat: u16_at(v, "beat")?,
                sub: u8_at(v, "sub")?,
                f: f32_at(v, "f").unwrap_or(0.0),
                nudge: i16_at(v, "nudge").unwrap_or(0),
            }));
        }
        "set_hit_y" => {
            return Some(edit(EngineCommand::SetHitY {
                track,
                hit: u16_at(v, "hit")?,
                y: f32_at(v, "y")?,
            }));
        }
        "set_hit_note" => {
            return Some(edit(EngineCommand::SetHitNote {
                track,
                hit: u16_at(v, "hit")?,
                note: f32_at(v, "note")?,
                velocity: f32_at(v, "velocity").unwrap_or(1.0),
            }));
        }
        "set_hit_probability" => {
            return Some(edit(EngineCommand::SetHitProbability {
                track,
                hit: u16_at(v, "hit")?,
                probability: f32_at(v, "probability")?,
            }));
        }
        "set_hit_retrig" => {
            return Some(edit(EngineCommand::SetHitRetrig {
                track,
                hit: u16_at(v, "hit")?,
                retrig: Retrig {
                    n: u8_at(v, "n")?,
                    m: u8_at(v, "m")?,
                    curve: curve_of(v.get("curve").and_then(|c| c.as_str()).unwrap_or("even")),
                    vel_end: f32_at(v, "vel_end").unwrap_or(1.0),
                },
            }));
        }
        "quantise_hit_x" => {
            return Some(edit(EngineCommand::QuantiseHitX {
                track,
                hit: u16_at(v, "hit")?,
                amount: f32_at(v, "amount")?,
            }));
        }
        "quantise_hit_y" => {
            return Some(edit(EngineCommand::QuantiseHitY {
                track,
                hit: u16_at(v, "hit")?,
                amount: f32_at(v, "amount")?,
            }));
        }
        _ => {}
    }
    match op {
        "set_send" => Some(edit(EngineCommand::SetSend {
            track,
            amount: f32_at(v, "amount")?,
        })),
        "toggle_hit" => Some(edit(EngineCommand::ToggleHit {
            track,
            slot: u16_at(v, "slot")?,
        })),
        "set_hit" => Some(edit(EngineCommand::SetHit {
            track,
            slot: u16_at(v, "slot")?,
            note: f32_at(v, "note")?,
            velocity: f32_at(v, "velocity")?,
        })),
        "set_probability" => Some(edit(EngineCommand::SetProbability {
            track,
            slot: u16_at(v, "slot")?,
            probability: f32_at(v, "probability")?,
        })),
        "set_retrig" => Some(edit(EngineCommand::SetRetrig {
            track,
            slot: u16_at(v, "slot")?,
            retrig: Retrig {
                n: u8_at(v, "n")?,
                m: u8_at(v, "m")?,
                curve: curve_of(v.get("curve").and_then(|c| c.as_str()).unwrap_or("even")),
                vel_end: f32_at(v, "vel_end").unwrap_or(1.0),
            },
        })),
        "set_grid_beats" => Some(edit(EngineCommand::SetGridBeats {
            track,
            beats: u8_at(v, "beats")?,
        })),
        "set_grid_subs" => Some(edit(EngineCommand::SetGridSubs {
            track,
            subs: u8_at(v, "subs")?,
        })),
        "set_gain" => Some(edit(EngineCommand::SetGain {
            track,
            gain: f32_at(v, "gain")?,
        })),
        "set_pan" => Some(edit(EngineCommand::SetPan {
            track,
            pan: f32_at(v, "pan")?,
        })),
        "set_choke_group" => Some(edit(EngineCommand::SetChokeGroup {
            track,
            group: u8_at(v, "group")?,
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
        "assign_voice" => Some(UiEvent::Custom(Box::new(Vxn3UiCustom::AssignVoice {
            track,
            kind: kind_of(v.get("engine")?.as_str()?)?,
            flavour: parse_flavour(v)?,
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
    fn parses_toggle_hit() {
        let ev = parse_custom_ui("toggle_hit", &obj(r#"{"track":2,"slot":5}"#)).unwrap();
        match ev {
            UiEvent::Custom(b) => match *b.downcast::<Vxn3UiCustom>().unwrap() {
                Vxn3UiCustom::Edit(EngineCommand::ToggleHit { track, slot }) => {
                    assert_eq!((track, slot), (2, 5));
                }
                _ => panic!("wrong variant"),
            },
            _ => panic!("not custom"),
        }
    }

    #[test]
    fn parses_grid_geometry_edits() {
        let ev = parse_custom_ui("set_grid_beats", &obj(r#"{"track":1,"beats":3}"#)).unwrap();
        match ev {
            UiEvent::Custom(b) => match *b.downcast::<Vxn3UiCustom>().unwrap() {
                Vxn3UiCustom::Edit(EngineCommand::SetGridBeats { track, beats }) => {
                    assert_eq!((track, beats), (1, 3));
                }
                _ => panic!("wrong variant"),
            },
            _ => panic!("not custom"),
        }
        assert!(parse_custom_ui("set_grid_subs", &obj(r#"{"track":0,"subs":3}"#)).is_some());
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
    fn parses_assign_voice() {
        // A Metal voice (10 params) with one binding + a renamed macro.
        let json = r#"{
            "track": 3, "engine": "metal",
            "base": [1200,1.1,0.08,0.5,44,0,0,6,5000,0],
            "bindings": [{"slot":0,"param":1,"depth":1.9,"curve":"exp"}],
            "macro_defaults": [0.5,0.5,0.5],
            "macro_names": ["Ring","",""]
        }"#;
        let ev = parse_custom_ui("assign_voice", &obj(json)).unwrap();
        match ev {
            UiEvent::Custom(b) => match *b.downcast::<Vxn3UiCustom>().unwrap() {
                Vxn3UiCustom::AssignVoice { track, kind, flavour } => {
                    assert_eq!(track, 3);
                    assert_eq!(kind, EngineKind::Metal);
                    assert_eq!(flavour.base.len(), 10);
                    assert_eq!(flavour.bindings.len(), 1);
                    assert_eq!(flavour.bindings[0].param, 1);
                    assert_eq!(flavour.bindings[0].curve, Curve::Exp);
                    assert_eq!(flavour.macro_names[0], "Ring");
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
        // The strip draws its markers from real per-lane geometry (0353), so the
        // page must be able to place one without a step count to multiply by.
        assert!(html.contains("\"markers\":[0.0,1.0,2.0,3.0,4.0]"));
        assert!(html.contains("\"subs\":[4,4,4,4]"));
        assert!(html.contains("\"max_hits\":64"));
        assert!(html.contains("\"swing\":{\"amount\":0.0,\"period\":2,\"shape\":0}"));
    }

    /// Every lane ships its own geometry — the strip is per-lane, and polymeter
    /// means the lanes diverge the moment the user edits one.
    #[test]
    fn config_ships_geometry_for_every_lane() {
        let config = serde_json::json!({ "lanes": (0..N_TRACKS).map(|_| grid_json(&Grid::default())).collect::<Vec<_>>() });
        let lanes = config["lanes"].as_array().unwrap();
        assert_eq!(lanes.len(), N_TRACKS);
        for l in lanes {
            assert_eq!(l["n_beats"], 4);
            assert_eq!(l["len_beats"], 4.0);
            assert_eq!(l["markers"].as_array().unwrap().len(), 5, "n_beats + 1 markers");
            assert_eq!(l["subs"].as_array().unwrap().len(), 4);
            assert_eq!(l["sub_override"][0], 0);
        }
    }

    /// A tuplet beat rides on the sub-count override, not on the resolved count —
    /// the page has to be able to tell one from the other or a lane-wide sub edit
    /// would silently erase it.
    #[test]
    fn grid_json_reports_tuplet_overrides_separately() {
        let mut g = Grid::default();
        g.set_beat_subs(2, Some(3));
        let j = grid_json(&g);
        assert_eq!(j["subs"], serde_json::json!([4, 4, 3, 4]));
        assert_eq!(j["sub_override"], serde_json::json!([0, 0, 3, 0]));
    }

    #[test]
    fn parses_the_lane_strip_vocabulary() {
        let ev = parse_custom_ui(
            "add_hit",
            &obj(r#"{"track":1,"beat":2,"sub":3,"f":0.5,"nudge":-12,"y":0.25,"note":36,"velocity":0.8}"#),
        )
        .unwrap();
        match ev {
            UiEvent::Custom(b) => match *b.downcast::<Vxn3UiCustom>().unwrap() {
                Vxn3UiCustom::Edit(EngineCommand::AddHit {
                    track, beat, sub, f, nudge, y, ..
                }) => {
                    assert_eq!((track, beat, sub), (1, 2, 3));
                    assert_eq!((f, nudge, y), (0.5, -12, 0.25));
                }
                _ => panic!("wrong variant"),
            },
            _ => panic!("not custom"),
        }
        // The two quantise verbs are separate opcodes, so a selection can be
        // corrected in one axis without touching the other.
        for op in ["quantise_hit_x", "quantise_hit_y"] {
            assert!(parse_custom_ui(op, &obj(r#"{"track":0,"hit":3,"amount":0.5}"#)).is_some());
        }
        for op in ["remove_hit", "set_hit_y", "set_hit_position", "set_hit_probability"] {
            let json = r#"{"track":0,"hit":1,"beat":0,"sub":1,"f":0.2,"y":0.3,"probability":0.5}"#;
            assert!(parse_custom_ui(op, &obj(json)).is_some(), "{op}");
        }
        // A missing offset reads as a welded hit rather than failing the parse.
        let ev =
            parse_custom_ui("set_hit_position", &obj(r#"{"track":0,"hit":0,"beat":1,"sub":0}"#))
                .unwrap();
        match ev {
            UiEvent::Custom(b) => match *b.downcast::<Vxn3UiCustom>().unwrap() {
                Vxn3UiCustom::Edit(EngineCommand::SetHitPosition { f, nudge, .. }) => {
                    assert_eq!((f, nudge), (0.0, 0));
                }
                _ => panic!("wrong variant"),
            },
            _ => panic!("not custom"),
        }
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
