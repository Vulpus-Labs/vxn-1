//! VXN1 editor (Vizia), embedded into the host window via baseview.
//!
//! Laid out as a Jupiter-8-style faceplate: bordered, header-labelled panels
//! arranged in rows, mostly small vertical faders, with compact labels. Each
//! parameter picks a widget: a vertical [`Slider`] for continuous (float/int)
//! params; a rotary [`Knob`] selector for waveform/colour/shape enums; a
//! [`ButtonGroup`] for the oversample enum; a (vertical) [`Switch`] for bools
//! and two-option enums; a [`Select`] dropdown for any remaining enum. Value
//! readouts use the shared [`vxn_engine::ParamDesc::display`] so the editor and
//! the host's generic UI read identically.
//!
//! Parameter flow (see `vxn_engine::SharedParams` and `vxn-clap`'s
//! `LocalParams`):
//!
//! - **UI → host:** a control's callback writes the new value into the shared
//!   store; faders raise a gesture on pointer down/up. The plugin's audio thread
//!   diffs the store each `process` and emits the change (bracketed by the
//!   gesture) to the host, so DAW automation recording stays in sync.
//! - **host → UI:** [`Application::on_idle`] emits a poll; [`UiModel`] reads the
//!   shared store back into the reactive [`SyncSignal`]s so controls track live
//!   automation. Signals are created on the UI thread (scoped to the view tree)
//!   and reached from `on_idle` via the model, avoiding leaks.
//!
//! Fader signals hold the *normalized* `[0, 1]` value; the shared store converts
//! to/from plain units via the parameter descriptors.
//!
//! The 5×4 modulation matrix is surfaced economically: only the musically
//! useful routes get dedicated faders, placed in context (filter mods in the
//! Filter panel, vibrato/pitch-env/PWM in VCO Mod, velocity/tremolo in Amp).
//! The remaining cells stay engine-only but host-automatable.

use std::ffi::c_void;
use std::sync::Arc;

use vizia::ParentWindow;
use vizia::prelude::*;
use vxn_engine::{PARAMS, ParamId, ParamKind, SharedParams};

/// Handle to the live editor window. Call [`WindowHandle::close`] when the host
/// destroys the GUI.
pub type EditorHandle = WindowHandle;

pub const EDITOR_WIDTH: u32 = 820;
pub const EDITOR_HEIGHT: u32 = 470;

/// A control entry: parameter id plus a short faceplate label (the panel header
/// supplies the context, so per-control labels stay terse).
type Entry = (ParamId, &'static str);

/// Faceplate layout: rows of panels, each panel a titled group of controls.
/// Mod-matrix routes appear as dedicated faders in context (VCO Mod / Filter /
/// Amp panels), not as a generic grid.
const ROWS: &[&[(&str, &[Entry])]] = {
    use ParamId::*;
    &[
        &[
            (
                "Osc 1",
                &[
                    (Osc1Wave, "Wave"),
                    (Osc1Coarse, "Coarse"),
                    (Osc1Fine, "Fine"),
                    (Osc1Level, "Level"),
                    (Osc1PulseWidth, "PW"),
                ],
            ),
            (
                "Osc 2",
                &[
                    (Osc2Wave, "Wave"),
                    (Osc2Coarse, "Coarse"),
                    (Osc2Fine, "Fine"),
                    (Osc2Level, "Level"),
                    (Osc2PulseWidth, "PW"),
                ],
            ),
            ("Noise", &[(NoiseColor, "Color"), (NoiseLevel, "Level")]),
            (
                "VCO Mod",
                &[(LfoPitch, "Vib"), (Env1Pitch, "P.Env"), (LfoPwm, "PWM")],
            ),
        ],
        &[
            (
                "Filter",
                &[
                    (Cutoff, "Cutoff"),
                    (Resonance, "Reso"),
                    (Drive, "Drive"),
                    (Env1Cutoff, "Env"),
                    (KeyCutoff, "Key"),
                    (LfoCutoff, "LFO"),
                    (VelCutoff, "Vel"),
                    (FilterVariant, "Type"),
                ],
            ),
            (
                "Env 1",
                &[
                    (Env1Attack, "A"),
                    (Env1Decay, "D"),
                    (Env1Sustain, "S"),
                    (Env1Release, "R"),
                    (Env1Shape, "Shape"),
                ],
            ),
            (
                "Env 2",
                &[
                    (Env2Attack, "A"),
                    (Env2Decay, "D"),
                    (Env2Sustain, "S"),
                    (Env2Release, "R"),
                    (Env2Shape, "Shape"),
                ],
            ),
        ],
        &[
            ("LFO", &[(LfoShape, "Shape"), (LfoRate, "Rate")]),
            ("Amp", &[(VelAmp, "Vel"), (LfoAmp, "Trem")]),
            (
                "Master",
                &[
                    (MasterTune, "Tune"),
                    (MasterVolume, "Volume"),
                    (Oversample, "OvSmp"),
                ],
            ),
            (
                "Chorus",
                &[
                    (ChorusOn, "On"),
                    (ChorusRate, "Rate"),
                    (ChorusDepth, "Depth"),
                    (ChorusMix, "Mix"),
                ],
            ),
            (
                "Delay",
                &[
                    (DelayOn, "On"),
                    (DelayTime, "Time"),
                    (DelayFeedback, "FB"),
                    (DelayMix, "Mix"),
                    (DelayPingPong, "Ping"),
                ],
            ),
        ],
    ]
};

/// Stylesheet: dark faceplate, orange panel headers, small text.
const STYLE: &str = r#"
:root { background-color: #2b2b2b; font-family: "IBM Plex Sans Condensed Medium"; }
label { font-size: 11; color: #d6d6d6; }
.panel { background-color: #1c1c1c; border-width: 1px; border-color: #0e0e0e; corner-radius: 4px; }
.panel-header { background-color: #d9701b; color: #141414; corner-radius: 2px; }
.ctl-label { font-size: 9; color: #aeaeae; }
.ctl-value { font-size: 9; color: #d9701b; }
.vswitch { rotate: 270deg; top: 20px; }
.ovsmp { gap: 2px; }
.ovsmp toggle-button { background-color: #555555; padding: 3px; }
.ovsmp toggle-button:checked { background-color: #2e9e3f; }
.ovsmp toggle-button label { color: #ffffff; font-size: 9; }
"#;

const FADER_H: f32 = 66.0;
const COL_H: f32 = 98.0;
const PANEL_H: f32 = 124.0;

/// UI value range for a fader. Bipolar routes (env→cutoff, env→pitch) use the
/// full descriptor range, centred at zero; the unipolar mod amounts
/// (key/LFO/velocity→cutoff, vibrato, LFO→PWM, velocity/LFO→amp) are shown
/// positive-only (`0..max`) even though the underlying depth param is bipolar.
fn ui_range(idx: usize) -> (f32, f32) {
    use ParamId::*;
    let d = &PARAMS[idx];
    match ParamId::from_index(idx) {
        Some(KeyCutoff | LfoCutoff | VelCutoff | LfoPitch | LfoPwm | VelAmp | LfoAmp) => {
            (0.0, d.max)
        }
        _ => (d.min, d.max),
    }
}

/// Plain value → fader position `[0, 1]` over the UI range.
fn fader_to_ui(idx: usize, value: f32) -> f32 {
    let (lo, hi) = ui_range(idx);
    if hi > lo {
        ((value - lo) / (hi - lo)).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Fader position `[0, 1]` → plain value over the UI range.
fn fader_from_ui(idx: usize, n: f32) -> f32 {
    let (lo, hi) = ui_range(idx);
    lo + n.clamp(0.0, 1.0) * (hi - lo)
}

/// A bound control and its reactive value signal, kept so `on_idle` can sync
/// the signal from host-side automation.
#[derive(Clone, Copy)]
enum Ctl {
    /// Continuous (float/int) → vertical fader; signal holds the normalized value.
    Fader(usize, SyncSignal<f32>),
    /// Osc waveform → rotary selector; signal holds the normalized value, snapped
    /// to the nearest variant on change.
    Rotary(usize, SyncSignal<f32>),
    /// Bool or two-variant enum → vertical switch; signal holds the on/off state.
    Switch(usize, SyncSignal<bool>),
    /// Enum → exclusive button group; signal holds the selected variant index.
    Buttons(usize, SyncSignal<Option<usize>>),
    /// Enum → dropdown; signal holds the selected variant index.
    Select(usize, SyncSignal<Option<usize>>),
}

impl Ctl {
    fn idx(self) -> usize {
        match self {
            Ctl::Fader(i, _)
            | Ctl::Rotary(i, _)
            | Ctl::Switch(i, _)
            | Ctl::Buttons(i, _)
            | Ctl::Select(i, _) => i,
        }
    }
}

fn make_ctl(id: ParamId, shared: &SharedParams) -> Ctl {
    let i = id.index();
    match id.desc().kind {
        ParamKind::Bool => Ctl::Switch(i, SyncSignal::new(shared.get(i) >= 0.5)),
        // Waveform / colour / shape selectors are rotary; Oversample is a button
        // group; two-option enums are switches; anything else a dropdown.
        ParamKind::Enum { variants } => {
            if matches!(
                id,
                ParamId::Osc1Wave | ParamId::Osc2Wave | ParamId::NoiseColor | ParamId::LfoShape
            ) {
                Ctl::Rotary(i, SyncSignal::new(shared.get_normalized(i)))
            } else if matches!(id, ParamId::Oversample) {
                Ctl::Buttons(i, SyncSignal::new(Some(shared.get(i).round() as usize)))
            } else if variants.len() == 2 {
                Ctl::Switch(i, SyncSignal::new(shared.get(i) >= 0.5))
            } else {
                Ctl::Select(i, SyncSignal::new(Some(shared.get(i).round() as usize)))
            }
        }
        _ => Ctl::Fader(i, SyncSignal::new(fader_to_ui(i, shared.get(i)))),
    }
}

/// Poll message emitted from `on_idle`: re-read the shared store into signals.
struct PollAutomation;

/// Bridges `on_idle` polling to the control signals so DAW automation playback
/// moves the controls. Edits flow the other way directly via each callback.
struct UiModel {
    controls: Vec<Ctl>,
    shared: Arc<SharedParams>,
}

impl Model for UiModel {
    fn event(&mut self, _cx: &mut EventContext, event: &mut Event) {
        event.map(|_msg: &PollAutomation, _meta| {
            for ctl in &self.controls {
                match *ctl {
                    Ctl::Fader(i, sig) => {
                        let n = fader_to_ui(i, self.shared.get(i));
                        if (sig.get() - n).abs() > f32::EPSILON {
                            sig.set(n);
                        }
                    }
                    Ctl::Rotary(i, sig) => {
                        let n = self.shared.get_normalized(i);
                        if (sig.get() - n).abs() > f32::EPSILON {
                            sig.set(n);
                        }
                    }
                    Ctl::Switch(i, sig) => {
                        let b = self.shared.get(i) >= 0.5;
                        if sig.get() != b {
                            sig.set(b);
                        }
                    }
                    Ctl::Buttons(i, sig) | Ctl::Select(i, sig) => {
                        let s = Some(self.shared.get(i).round() as usize);
                        if sig.get() != s {
                            sig.set(s);
                        }
                    }
                }
            }
        });
    }
}

/// Open the editor parented to `parent` (on macOS the host `NSView`).
pub fn open_editor(parent: *mut c_void, shared: Arc<SharedParams>) -> EditorHandle {
    let parent = ParentWindow(parent);
    Application::new(move |cx| build_editor(cx, Arc::clone(&shared)))
        .on_idle(|cx| cx.emit(PollAutomation))
        .inner_size((EDITOR_WIDTH, EDITOR_HEIGHT))
        .title("VXN1")
        .open_parented(&parent)
}

fn build_editor(cx: &mut Context, shared: Arc<SharedParams>) {
    // Bundle the faceplate font so it renders identically on any host/OS. Each
    // weight is its own family ("IBM Plex Sans Condensed {Thin|ExtraLight|
    // Medium}"); referenced by name in STYLE.
    cx.add_font_mem(include_bytes!("../fonts/IBMPlexSansCondensed-Thin.ttf"));
    cx.add_font_mem(include_bytes!(
        "../fonts/IBMPlexSansCondensed-ExtraLight.ttf"
    ));
    cx.add_font_mem(include_bytes!("../fonts/IBMPlexSansCondensed-Medium.ttf"));
    let _ = cx.add_stylesheet(STYLE);

    // One control per parameter (panels look them up by index; mod-matrix cells
    // not surfaced on the faceplate stay engine-only but remain host-automatable).
    // The model syncs every control from host automation on idle.
    let controls: Vec<Ctl> = ParamId::all().map(|id| make_ctl(id, &shared)).collect();

    UiModel {
        controls: controls.clone(),
        shared: Arc::clone(&shared),
    }
    .build(cx);

    ScrollView::new(cx, move |cx| {
        VStack::new(cx, |cx| {
            for row in ROWS {
                HStack::new(cx, |cx| {
                    for (title, entries) in *row {
                        panel_view(cx, title, entries, &controls, &shared);
                    }
                })
                .height(Pixels(PANEL_H))
                .horizontal_gap(Pixels(8.0));
            }
        })
        .vertical_gap(Pixels(8.0))
        .padding(Pixels(10.0));
    });
}

fn panel_view(
    cx: &mut Context,
    title: &'static str,
    entries: &'static [Entry],
    controls: &[Ctl],
    shared: &Arc<SharedParams>,
) {
    VStack::new(cx, |cx| {
        Label::new(cx, title)
            .class("panel-header")
            .width(Stretch(1.0))
            .height(Pixels(16.0))
            .alignment(Alignment::Center);
        HStack::new(cx, |cx| {
            for (id, short) in entries {
                let ctl = controls
                    .iter()
                    .copied()
                    .find(|c| c.idx() == id.index())
                    .unwrap();
                control_view(cx, ctl, shared, short);
            }
        })
        .height(Pixels(COL_H))
        .horizontal_gap(Pixels(6.0));
    })
    .class("panel")
    .height(Pixels(PANEL_H))
    .padding(Pixels(5.0))
    .vertical_gap(Pixels(4.0));
}

fn control_view(cx: &mut Context, ctl: Ctl, shared: &Arc<SharedParams>, short: &'static str) {
    VStack::new(cx, |cx| {
        Label::new(cx, short)
            .class("ctl-label")
            .height(Pixels(11.0));
        match ctl {
            Ctl::Fader(i, sig) => {
                let (sh_set, sh_down, sh_up) =
                    (Arc::clone(shared), Arc::clone(shared), Arc::clone(shared));
                Slider::new(cx, sig)
                    .vertical(true)
                    .class("fader")
                    .width(Pixels(16.0))
                    .height(Pixels(FADER_H))
                    .on_change(move |_cx, v| {
                        sig.set(v);
                        sh_set.set(i, fader_from_ui(i, v));
                    })
                    .on_mouse_down(move |_cx, _btn| sh_down.set_gesture(i, true))
                    .on_mouse_up(move |_cx, _btn| sh_up.set_gesture(i, false));
                Label::new(
                    cx,
                    sig.map(move |n: &f32| PARAMS[i].display(fader_from_ui(i, *n))),
                )
                .class("ctl-value")
                .height(Pixels(11.0));
            }
            Ctl::Rotary(i, sig) => {
                let cnt = match PARAMS[i].kind {
                    ParamKind::Enum { variants } => variants.len(),
                    _ => 1,
                };
                let snap = move |n: f32| {
                    if cnt > 1 {
                        (n * (cnt - 1) as f32).round()
                    } else {
                        0.0
                    }
                };
                let default_norm = PARAMS[i].to_normalized(PARAMS[i].default);
                let (sh_set, sh_down, sh_up) =
                    (Arc::clone(shared), Arc::clone(shared), Arc::clone(shared));
                Knob::new(cx, default_norm, sig, false)
                    .on_change(move |_cx, v| {
                        // Snap to the nearest variant.
                        let idx = snap(v);
                        sig.set(if cnt > 1 { idx / (cnt - 1) as f32 } else { 0.0 });
                        sh_set.set(i, idx);
                    })
                    .on_mouse_down(move |_cx, _btn| sh_down.set_gesture(i, true))
                    .on_mouse_up(move |_cx, _btn| sh_up.set_gesture(i, false))
                    .size(Pixels(34.0));
                Label::new(cx, sig.map(move |n: &f32| PARAMS[i].display(snap(*n))))
                    .class("ctl-value")
                    .height(Pixels(11.0));
            }
            Ctl::Switch(i, sig) => {
                let sh = Arc::clone(shared);
                Switch::new(cx, sig).class("vswitch").on_toggle(move |_cx| {
                    let on = !sig.get();
                    sig.set(on);
                    sh.set(i, if on { 1.0 } else { 0.0 });
                });
                Label::new(
                    cx,
                    sig.map(move |b: &bool| PARAMS[i].display(if *b { 1.0 } else { 0.0 })),
                )
                .class("ctl-value")
                .height(Pixels(11.0));
            }
            Ctl::Buttons(i, sig) => {
                let variants = match PARAMS[i].kind {
                    ParamKind::Enum { variants } => variants,
                    _ => &[],
                };
                let shared = Arc::clone(shared);
                ButtonGroup::new(cx, move |cx| {
                    for (n, label) in variants.iter().enumerate() {
                        let sh = Arc::clone(&shared);
                        ToggleButton::new(
                            cx,
                            sig.map(move |s: &Option<usize>| *s == Some(n)),
                            move |cx| Label::new(cx, *label),
                        )
                        .on_press(move |_cx| {
                            sig.set(Some(n));
                            sh.set(i, n as f32);
                        });
                    }
                })
                .class("ovsmp");
            }
            Ctl::Select(i, sig) => {
                let variants = match PARAMS[i].kind {
                    ParamKind::Enum { variants } => variants,
                    _ => &[],
                };
                let options = Signal::new(
                    variants
                        .iter()
                        .copied()
                        .map(Localized::new)
                        .collect::<Vec<_>>(),
                );
                let sh = Arc::clone(shared);
                Select::new(cx, options, sig, true)
                    .on_select(move |_cx, choice| {
                        sig.set(Some(choice));
                        sh.set(i, choice as f32);
                    })
                    .width(Pixels(62.0));
            }
        }
    })
    .height(Pixels(COL_H))
    .vertical_gap(Pixels(3.0))
    .alignment(Alignment::TopCenter);
}
