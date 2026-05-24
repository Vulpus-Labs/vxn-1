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
use vizia::context::TreeProps;
use vizia::prelude::*;
use vizia::vg;
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
.value-pop { background-color: #0e0e0e; border-width: 1px; border-color: #d9701b; corner-radius: 3px; padding-left: 4px; padding-right: 4px; font-size: 10; color: #f6f6f6; }
.fader .track { background-color: #555555; width: 6px; corner-radius: 2px; }
.fader .range { background-color: #d9701b; corner-radius: 2px; }
.fader .thumb { background-color: #e8e8e8; border-width: 1px; border-color: #141414; corner-radius: 1px; width: 20px; height: 8px; }
.wave-glyph { color: #888888; }
.wave-glyph.active { color: #e8902f; }
.wave-txt { font-size: 8; color: #888888; }
.wave-txt.active { color: #e8902f; }
"#;

const FADER_H: f32 = 66.0;
const COL_H: f32 = 98.0;
const PANEL_H: f32 = 124.0;
/// Square area framing a selector knob, sized to fit the variant glyphs/labels
/// arranged around its arc.
const DIAL: f32 = 62.0;

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
                ParamId::Osc1Wave | ParamId::Osc2Wave | ParamId::LfoShape
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

/// Polyline (in a `[0, 1]²` box, y down) approximating one cycle of a named
/// waveform, for the little icons drawn around a waveform selector knob. Returns
/// empty for labels that aren't waveforms (e.g. noise colours), which fall back to
/// text labels instead.
fn wave_points(label: &str) -> Vec<(f32, f32)> {
    match label {
        "Sine" => (0..=16)
            .map(|k| {
                let t = k as f32 / 16.0;
                (t, 0.5 - 0.38 * (t * std::f32::consts::TAU).sin())
            })
            .collect(),
        "Triangle" | "Tri" => vec![(0.0, 0.85), (0.5, 0.15), (1.0, 0.85)],
        // Rising ramp with a vertical reset (one and a bit cycles reads clearly small).
        "Saw" | "Saw+" => vec![(0.0, 0.85), (0.5, 0.15), (0.5, 0.85), (1.0, 0.15)],
        "Saw-" => vec![(0.0, 0.15), (0.5, 0.85), (0.5, 0.15), (1.0, 0.85)],
        "Pulse" | "Square" => vec![
            (0.0, 0.85),
            (0.0, 0.15),
            (0.5, 0.15),
            (0.5, 0.85),
            (1.0, 0.85),
        ],
        "S&H" => vec![
            (0.0, 0.6),
            (0.28, 0.6),
            (0.28, 0.2),
            (0.56, 0.2),
            (0.56, 0.8),
            (0.82, 0.8),
            (0.82, 0.45),
            (1.0, 0.45),
        ],
        _ => Vec::new(),
    }
}

/// A small waveform icon, stroked in the view's current `color` so a `.active`
/// class can light it up. Used as a glyph "label" around a waveform selector knob.
struct WaveGlyph {
    label: &'static str,
}

impl WaveGlyph {
    fn new<'a>(cx: &'a mut Context, label: &'static str) -> Handle<'a, Self> {
        Self { label }.build(cx, |_| {})
    }
}

impl View for WaveGlyph {
    fn element(&self) -> Option<&'static str> {
        Some("waveglyph")
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &Canvas) {
        let pts = wave_points(self.label);
        if pts.is_empty() {
            return;
        }
        let b = cx.bounds();
        let s = cx.scale_factor();
        let pad = 2.0 * s;
        let (w, h) = (b.w - 2.0 * pad, b.h - 2.0 * pad);
        let mut path = vg::PathBuilder::new();
        for (k, (t, y)) in pts.iter().enumerate() {
            let p = (b.x + pad + t * w, b.y + pad + y * h);
            if k == 0 {
                path.move_to(p);
            } else {
                path.line_to(p);
            }
        }
        let mut paint = vg::Paint::default();
        paint.set_color(cx.font_color());
        paint.set_stroke_width(1.3 * s);
        paint.set_style(vg::PaintStyle::Stroke);
        paint.set_stroke_cap(vg::PaintCap::Round);
        paint.set_stroke_join(vg::PaintJoin::Round);
        paint.set_anti_alias(true);
        let path = path.detach();
        canvas.draw_path(&path, &paint);
    }
}

/// Cursor Y as a top offset (logical px) within the control cell, clamped to the
/// cell so the readout can't drift above it. Used to pin the value popup to the
/// point where the pointer entered/grabbed the control.
fn cursor_top(cx: &EventContext) -> f32 {
    let cell_y = cx.cache.get_bounds(cx.parent()).y;
    (((cx.mouse().cursor_y - cell_y) / cx.scale_factor()) - 8.0).max(0.0)
}

/// Floating value readout shown over a fader/knob while it is hovered or being
/// dragged. Absolutely positioned so it never reserves layout space, rendered only
/// while `show` is set, and pinned to `posy` (the cursor Y at hover/grab) so it
/// sits beside the pointer rather than over the control's label. Non-hoverable so
/// it doesn't steal the pointer and make the control flicker. The faceplate's
/// overflow stays visible so the readout can spill past the narrow control cell.
fn value_popup<T: ToStringLocalized + 'static>(
    cx: &mut Context,
    text: impl Res<T> + Clone + 'static,
    show: SyncSignal<bool>,
    posy: SyncSignal<f32>,
    x_off: f32,
) {
    Label::new(cx, text)
        .class("value-pop")
        .position_type(PositionType::Absolute)
        .top(posy.map(|y: &f32| Pixels(*y)))
        .left(Stretch(1.0))
        .right(Stretch(1.0))
        .width(Auto)
        .height(Auto)
        // Nudge sideways (faders) so the readout sits beside the thumb rather than
        // on top of it, keeping the thumb visible while dragging.
        .translate((Pixels(x_off), Pixels(0.0)))
        .z_index(100)
        .hoverable(false)
        .display(show);
}

fn control_view(cx: &mut Context, ctl: Ctl, shared: &Arc<SharedParams>, short: &'static str) {
    VStack::new(cx, |cx| {
        Label::new(cx, short)
            .class("ctl-label")
            .height(Pixels(11.0));
        match ctl {
            Ctl::Fader(i, sig) => {
                let (hover, drag, show, posy) = (
                    SyncSignal::new(false),
                    SyncSignal::new(false),
                    SyncSignal::new(false),
                    SyncSignal::new(0.0f32),
                );
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
                    .on_over(move |cx| {
                        posy.set(cursor_top(cx));
                        hover.set(true);
                        show.set(true);
                    })
                    .on_over_out(move |_cx| {
                        hover.set(false);
                        show.set(drag.get());
                    })
                    .on_mouse_down(move |cx, _btn| {
                        posy.set(cursor_top(cx));
                        drag.set(true);
                        show.set(true);
                        sh_down.set_gesture(i, true);
                    })
                    .on_mouse_up(move |_cx, _btn| {
                        drag.set(false);
                        show.set(hover.get());
                        sh_up.set_gesture(i, false);
                    });
                value_popup(
                    cx,
                    sig.map(move |n: &f32| PARAMS[i].display(fader_from_ui(i, *n))),
                    show,
                    posy,
                    22.0,
                );
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
                let variants = match PARAMS[i].kind {
                    ParamKind::Enum { variants } => variants,
                    _ => &[][..],
                };
                // Waveform selectors get drawn glyphs around the arc; other enums
                // (e.g. noise colour) get small text labels at the same positions.
                let use_glyphs = !variants.is_empty() && variants.iter().all(|l| !wave_points(l).is_empty());
                let (hover, drag, show, posy) = (
                    SyncSignal::new(false),
                    SyncSignal::new(false),
                    SyncSignal::new(false),
                    SyncSignal::new(0.0f32),
                );
                let (sh_set, sh_down, sh_up) =
                    (Arc::clone(shared), Arc::clone(shared), Arc::clone(shared));
                // Dial: knob centred, variant glyphs/labels arranged around its
                // 300° sweep (value 0..1 -> -150°..+150°, gap at the bottom). The
                // popup lives here too so its cursor-pinned offset shares the knob's
                // coordinate space.
                ZStack::new(cx, move |cx| {
                    const C: f32 = DIAL / 2.0;
                    const R: f32 = 25.0;
                    for (n, label) in variants.iter().enumerate() {
                        let value = if cnt > 1 { n as f32 / (cnt - 1) as f32 } else { 0.5 };
                        let theta = (value * 300.0 - 150.0).to_radians();
                        let active = sig.map(move |v: &f32| {
                            cnt > 1 && (*v * (cnt - 1) as f32).round() as usize == n
                        });
                        if use_glyphs {
                            const G: f32 = 14.0;
                            WaveGlyph::new(cx, label)
                                .class("wave-glyph")
                                .toggle_class("active", active)
                                .position_type(PositionType::Absolute)
                                .left(Pixels(C + R * theta.sin() - G / 2.0))
                                .top(Pixels(C - R * theta.cos() - G / 2.0))
                                .width(Pixels(G))
                                .height(Pixels(G))
                                .hoverable(false);
                        } else {
                            const GW: f32 = 24.0;
                            const GH: f32 = 10.0;
                            Label::new(cx, *label)
                                .class("wave-txt")
                                .toggle_class("active", active)
                                .position_type(PositionType::Absolute)
                                .left(Pixels(C + R * theta.sin() - GW / 2.0))
                                .top(Pixels(C - R * theta.cos() - GH / 2.0))
                                .width(Pixels(GW))
                                .height(Pixels(GH))
                                .alignment(Alignment::Center)
                                .hoverable(false);
                        }
                    }
                    Knob::new(cx, default_norm, sig, false)
                        .on_change(move |_cx, v| {
                            // Snap to the nearest variant.
                            let idx = snap(v);
                            sig.set(if cnt > 1 { idx / (cnt - 1) as f32 } else { 0.0 });
                            sh_set.set(i, idx);
                        })
                        .on_over(move |cx| {
                            posy.set(cursor_top(cx));
                            hover.set(true);
                            show.set(true);
                        })
                        .on_over_out(move |_cx| {
                            hover.set(false);
                            show.set(drag.get());
                        })
                        .on_mouse_down(move |cx, _btn| {
                            posy.set(cursor_top(cx));
                            drag.set(true);
                            show.set(true);
                            sh_down.set_gesture(i, true);
                        })
                        .on_mouse_up(move |_cx, _btn| {
                            drag.set(false);
                            show.set(hover.get());
                            sh_up.set_gesture(i, false);
                        })
                        .size(Pixels(26.0));
                    value_popup(
                        cx,
                        sig.map(move |n: &f32| PARAMS[i].display(snap(*n))),
                        show,
                        posy,
                        0.0,
                    );
                })
                .size(Pixels(DIAL))
                .alignment(Alignment::Center);
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
    .vertical_gap(Pixels(8.0))
    .alignment(Alignment::TopCenter);
}
