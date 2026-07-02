//! Event dispatch + state + transport tests.
//!
//! Drives `dispatch_event` against a recording stub engine; rounds
//! state through a fake `ParamModel`.

use std::cell::RefCell;
use std::ops::Bound;

use clack_plugin::events::Pckn;
use clack_plugin::events::event_types::{MidiEvent, NoteOffEvent, NoteOnEvent, ParamValueEvent};
use clack_plugin::events::io::EventBuffer;
use clack_plugin::prelude::ClapId;
use clack_plugin::utils::Cookie;

use vxn_core_app::{ParamDesc, ParamId, ParamKind, ParamModel, Taper};
use vxn_core_clap::{
    EngineNotes, EngineProcess, batch_range, dispatch_event,
};

#[derive(Default, Debug, PartialEq)]
struct EngineLog {
    notes_on: Vec<(u8, f32)>,
    notes_off: Vec<u8>,
    pitch_bend: f32,
    mod_wheel: f32,
    aftertouch: f32,
    block_frames: Vec<usize>,
    reset_count: u32,
    sample_rate: f32,
    tempo: f32,
}

#[derive(Default)]
struct StubEngine {
    log: RefCell<EngineLog>,
}

impl EngineNotes for StubEngine {
    fn note_on(&mut self, key: u8, velocity: f32) {
        self.log.borrow_mut().notes_on.push((key, velocity));
    }
    fn note_off(&mut self, key: u8) {
        self.log.borrow_mut().notes_off.push(key);
    }
    fn pitch_bend(&mut self, value: f32) {
        self.log.borrow_mut().pitch_bend = value;
    }
    fn mod_wheel(&mut self, value: f32) {
        self.log.borrow_mut().mod_wheel = value;
    }
    fn aftertouch(&mut self, value: f32) {
        self.log.borrow_mut().aftertouch = value;
    }
}

impl EngineProcess for StubEngine {
    fn process_block(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.log.borrow_mut().block_frames.push(left.len());
        for s in left.iter_mut() {
            *s = 0.0;
        }
        for s in right.iter_mut() {
            *s = 0.0;
        }
    }
    fn reset(&mut self) {
        self.log.borrow_mut().reset_count += 1;
    }
    fn set_sample_rate(&mut self, sr: f32) {
        self.log.borrow_mut().sample_rate = sr;
    }
    fn set_tempo(&mut self, bpm: f32) {
        self.log.borrow_mut().tempo = bpm;
    }
}

fn pckn(key: u16) -> Pckn {
    Pckn::new(0_u8, 0_u8, key as u8, 0_u8)
}

#[test]
fn batch_range_clamps_to_frame_count() {
    assert_eq!(
        batch_range((Bound::Included(300), Bound::Excluded(500)), 256),
        (256, 256)
    );
    assert_eq!(
        batch_range((Bound::Unbounded, Bound::Unbounded), 64),
        (0, 64)
    );
    assert_eq!(
        batch_range((Bound::Included(10), Bound::Excluded(20)), 64),
        (10, 20)
    );
}

#[test]
fn dispatch_note_on_off_round_trip() {
    let mut engine = StubEngine::default();
    let mut buf = EventBuffer::with_capacity(4);
    buf.push(&NoteOnEvent::new(0, pckn(60), 0.78));
    buf.push(&NoteOffEvent::new(0, pckn(60), 0.0));
    for ev in buf.iter() {
        dispatch_event(&mut engine, &mut |_| {}, ev);
    }
    let log = engine.log.borrow();
    assert_eq!(log.notes_on, vec![(60, 0.78)]);
    assert_eq!(log.notes_off, vec![60]);
}

#[test]
fn dispatch_raw_midi_note_on_off() {
    // Raw-MIDI hosts (the clap-wrapper standalone) send note on/off as
    // channel-voice bytes rather than typed CLAP note events. Note-on with
    // velocity 0 is note-off by MIDI convention.
    let mut engine = StubEngine::default();
    let mut buf = EventBuffer::with_capacity(4);
    buf.push(&MidiEvent::new(0, 0, [0x90, 60, 100])); // note on, vel 100
    buf.push(&MidiEvent::new(0, 0, [0x80, 60, 0])); // note off
    buf.push(&MidiEvent::new(0, 0, [0x90, 64, 0])); // note on vel 0 → off
    for ev in buf.iter() {
        dispatch_event(&mut engine, &mut |_| {}, ev);
    }
    let log = engine.log.borrow();
    assert_eq!(log.notes_on, vec![(60, 100.0 / 127.0)]);
    assert_eq!(log.notes_off, vec![60, 64]);
}

#[test]
fn dispatch_midi_pitch_bend_normalises_to_unit_range() {
    let mut engine = StubEngine::default();
    // Centre + 4096 → +0.5 of bend range.
    let raw: u16 = 8192 + 4096;
    let d1 = (raw & 0x7F) as u8;
    let d2 = ((raw >> 7) & 0x7F) as u8;
    let mut buf = EventBuffer::with_capacity(2);
    buf.push(&MidiEvent::new(0, 0, [0xE0, d1, d2]));
    for ev in buf.iter() {
        dispatch_event(&mut engine, &mut |_| {}, ev);
    }
    assert!((engine.log.borrow().pitch_bend - 0.5).abs() < 1e-4);
}

#[test]
fn dispatch_midi_mod_wheel_and_aftertouch() {
    let mut engine = StubEngine::default();
    let mut buf = EventBuffer::with_capacity(4);
    buf.push(&MidiEvent::new(0, 0, [0xB0, 1, 64])); // CC1 = 64
    buf.push(&MidiEvent::new(0, 0, [0xD0, 32, 0])); // aftertouch = 32
    for ev in buf.iter() {
        dispatch_event(&mut engine, &mut |_| {}, ev);
    }
    let log = engine.log.borrow();
    assert!((log.mod_wheel - 64.0 / 127.0).abs() < 1e-6);
    assert!((log.aftertouch - 32.0 / 127.0).abs() < 1e-6);
}

#[test]
fn dispatch_mod_wheel_deadzone() {
    let mut engine = StubEngine::default();
    let mut buf = EventBuffer::with_capacity(2);
    // CC1 = 1 lands inside the deadzone.
    buf.push(&MidiEvent::new(0, 0, [0xB0, 1, 1]));
    for ev in buf.iter() {
        dispatch_event(&mut engine, &mut |_| {}, ev);
    }
    assert_eq!(engine.log.borrow().mod_wheel, 0.0);
}

#[test]
fn dispatch_param_value_routes_to_on_param_callback() {
    let mut engine = StubEngine::default();
    let mut saw_param: Option<(u32, f64)> = None;
    let mut on_param = |ev: &clack_plugin::events::UnknownEvent| {
        use clack_plugin::events::spaces::CoreEventSpace;
        if let Some(CoreEventSpace::ParamValue(e)) = ev.as_core_event() {
            if let Some(pid) = e.param_id() {
                saw_param = Some((pid.get(), e.value()));
            }
        }
    };
    let mut buf = EventBuffer::with_capacity(2);
    buf.push(&ParamValueEvent::new(
        0,
        ClapId::new(7),
        Pckn::match_all(),
        0.42,
        Cookie::empty(),
    ));
    for ev in buf.iter() {
        dispatch_event(&mut engine, &mut on_param, ev);
    }
    assert_eq!(saw_param, Some((7, 0.42)));
}

#[test]
fn engine_reset_via_process_trait() {
    let mut engine = StubEngine::default();
    engine.reset();
    engine.reset();
    assert_eq!(engine.log.borrow().reset_count, 2);
}

#[test]
fn engine_block_render_writes_silence() {
    let mut engine = StubEngine::default();
    let mut l = [1.0_f32; 32];
    let mut r = [1.0_f32; 32];
    engine.process_block(&mut l, &mut r);
    assert!(l.iter().all(|&s| s == 0.0));
    assert!(r.iter().all(|&s| s == 0.0));
    assert_eq!(engine.log.borrow().block_frames, vec![32]);
}

// ── State save / load round-trip ────────────────────────────────────────────

static DESC: ParamDesc = ParamDesc {
    name: "p",
    label: "P",
    min: 0.0,
    max: 1.0,
    default: 0.0,
    kind: ParamKind::Float { unit: "", taper: Taper::Linear },
};

struct FakeModel {
    values: std::sync::RwLock<Vec<f32>>,
}

impl FakeModel {
    fn new(values: Vec<f32>) -> Self {
        Self { values: std::sync::RwLock::new(values) }
    }
}

impl ParamModel for FakeModel {
    fn total(&self) -> usize {
        self.values.read().unwrap().len()
    }
    fn get(&self, id: ParamId) -> f32 {
        self.values.read().unwrap()[id.raw()]
    }
    fn set(&self, id: ParamId, plain: f32) {
        self.values.write().unwrap()[id.raw()] = plain;
    }
    fn get_normalized(&self, _id: ParamId) -> f32 {
        0.0
    }
    fn set_normalized(&self, _id: ParamId, _norm: f32) {}
    fn gesture(&self, _id: ParamId) -> bool {
        false
    }
    fn set_gesture(&self, _id: ParamId, _on: bool) {}
    fn descriptor(&self, _id: ParamId) -> Option<&'static ParamDesc> {
        Some(&DESC)
    }
    fn snapshot_bytes(&self) -> Vec<u8> {
        let v = self.values.read().unwrap();
        let mut blob = (v.len() as u32).to_le_bytes().to_vec();
        for &x in v.iter() {
            blob.extend_from_slice(&x.to_le_bytes());
        }
        blob
    }
    fn restore_from_bytes(&self, blob: &[u8]) -> Result<(), String> {
        if blob.len() < 4 {
            return Err("blob too short".into());
        }
        let n = u32::from_le_bytes(blob[..4].try_into().unwrap()) as usize;
        let body = &blob[4..];
        if body.len() < n * 4 {
            return Err("blob truncated".into());
        }
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let bytes: [u8; 4] = body[i * 4..i * 4 + 4].try_into().unwrap();
            out.push(f32::from_le_bytes(bytes));
        }
        *self.values.write().unwrap() = out;
        Ok(())
    }
}

#[test]
fn state_blob_round_trips_via_param_model() {
    let src = FakeModel::new(vec![0.25, -0.5, 1.5, 0.0]);
    let blob = src.snapshot_bytes();

    let dst = FakeModel::new(vec![0.0; 4]);
    dst.restore_from_bytes(&blob).expect("restore");

    assert_eq!(*dst.values.read().unwrap(), *src.values.read().unwrap());
}

// ── Transport ──────────────────────────────────────────────────────────────

#[test]
fn tempo_from_transport_returns_some_when_flag_set() {
    use clack_plugin::events::EventFlags;
    use clack_plugin::events::event_types::{TransportEvent, TransportFlags};
    let mut t = TransportEvent {
        header: clack_plugin::events::EventHeader::new_core(0, EventFlags::empty()),
        flags: TransportFlags::HAS_TEMPO,
        song_pos_beats: clack_plugin::utils::BeatTime::from_int(0),
        song_pos_seconds: clack_plugin::utils::SecondsTime::from_int(0),
        tempo: 140.0,
        tempo_inc: 0.0,
        loop_start_beats: clack_plugin::utils::BeatTime::from_int(0),
        loop_end_beats: clack_plugin::utils::BeatTime::from_int(0),
        loop_start_seconds: clack_plugin::utils::SecondsTime::from_int(0),
        loop_end_seconds: clack_plugin::utils::SecondsTime::from_int(0),
        bar_start: clack_plugin::utils::BeatTime::from_int(0),
        bar_number: 0,
        time_signature_numerator: 4,
        time_signature_denominator: 4,
    };
    assert_eq!(vxn_core_clap::tempo_from_transport(&t), Some(140.0));

    t.flags = TransportFlags::empty();
    assert_eq!(vxn_core_clap::tempo_from_transport(&t), None);
}
