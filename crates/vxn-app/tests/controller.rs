//! Controller round-trip tests against an in-memory `MockModel` (0035).
//!
//! No vizia/wry/engine touched — just the trait surface from `vxn_app`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, RwLock};

use vxn_app::{
    Controller, HostEvent, KeyMode, ParamDesc, ParamId, ParamKind, ParamModel, PresetLoad,
    PresetMeta, PresetSource, PresetStore, Taper, UiEvent, UserFolderEntry, ViewEvent,
};

// ── Mock model ──────────────────────────────────────────────────────────────

// Static descriptor table: the trait returns `&'static ParamDesc`.
static MOCK_DESCS: [ParamDesc; 4] = [
    mock_desc("p0"),
    mock_desc("p1"),
    mock_desc("p2"),
    mock_desc("p3"),
];
const fn mock_desc(name: &'static str) -> ParamDesc {
    ParamDesc {
        name,
        label: name,
        min: 0.0,
        max: 1.0,
        default: 0.0,
        kind: ParamKind::Float {
            unit: "",
            taper: Taper::Linear,
        },
    }
}

struct MockModel {
    total: usize,
    values: RwLock<HashMap<ParamId, f32>>,
    gestures: RwLock<HashMap<ParamId, bool>>,
    key_mode: RwLock<KeyMode>,
    split_point: RwLock<u8>,
}

impl MockModel {
    fn new(total: usize) -> Self {
        Self {
            total,
            values: RwLock::new(HashMap::new()),
            gestures: RwLock::new(HashMap::new()),
            key_mode: RwLock::new(KeyMode::Whole),
            split_point: RwLock::new(60),
        }
    }
}

impl ParamModel for MockModel {
    fn total(&self) -> usize {
        self.total
    }
    fn get(&self, id: ParamId) -> f32 {
        *self.values.read().unwrap().get(&id).unwrap_or(&0.0)
    }
    fn set(&self, id: ParamId, plain: f32) {
        self.values.write().unwrap().insert(id, plain);
    }
    fn get_normalized(&self, id: ParamId) -> f32 {
        self.get(id)
    }
    fn set_normalized(&self, id: ParamId, norm: f32) {
        self.set(id, norm);
    }
    fn gesture(&self, id: ParamId) -> bool {
        *self.gestures.read().unwrap().get(&id).unwrap_or(&false)
    }
    fn set_gesture(&self, id: ParamId, on: bool) {
        self.gestures.write().unwrap().insert(id, on);
    }
    fn descriptor(&self, id: ParamId) -> Option<&'static ParamDesc> {
        MOCK_DESCS.get(id.raw())
    }
    fn key_mode(&self) -> KeyMode {
        *self.key_mode.read().unwrap()
    }
    fn set_key_mode(&self, mode: KeyMode) {
        *self.key_mode.write().unwrap() = mode;
    }
    fn set_key_mode_seeded(&self, mode: KeyMode) {
        // Mock: no upper→lower seed; just set.
        *self.key_mode.write().unwrap() = mode;
    }
    fn split_point(&self) -> u8 {
        *self.split_point.read().unwrap()
    }
    fn set_split_point(&self, note: u8) {
        *self.split_point.write().unwrap() = note;
    }
    fn snapshot_bytes(&self) -> Vec<u8> {
        // Trivial format: [total: u32 le] then total × f32 le.
        let vals = self.values.read().unwrap();
        let mut buf = Vec::with_capacity(4 + self.total * 4);
        buf.extend_from_slice(&(self.total as u32).to_le_bytes());
        for i in 0..self.total {
            let v = *vals.get(&ParamId::new(i)).unwrap_or(&0.0);
            buf.extend_from_slice(&v.to_le_bytes());
        }
        buf
    }
    fn restore_from_bytes(&self, blob: &[u8]) -> Result<(), String> {
        if blob.len() < 4 {
            return Err("blob too short".into());
        }
        let n = u32::from_le_bytes(blob[0..4].try_into().unwrap()) as usize;
        if blob.len() < 4 + n * 4 {
            return Err("blob truncated".into());
        }
        let mut vals = self.values.write().unwrap();
        for i in 0..n {
            let off = 4 + i * 4;
            let v = f32::from_le_bytes(blob[off..off + 4].try_into().unwrap());
            vals.insert(ParamId::new(i), v);
        }
        Ok(())
    }
}

// ── Mock preset store ───────────────────────────────────────────────────────

#[derive(Default)]
struct MockPresetStore {
    factory: Vec<(PresetMeta, Vec<u8>)>,
    saves: Mutex<Vec<(String, Option<String>, PresetMeta, Vec<u8>)>>,
}

impl MockPresetStore {
    fn with_factory(entries: Vec<(PresetMeta, Vec<u8>)>) -> Self {
        Self {
            factory: entries,
            ..Default::default()
        }
    }
}

impl PresetStore for MockPresetStore {
    fn factory_len(&self) -> usize {
        self.factory.len()
    }
    fn factory_load(&self, index: usize) -> Result<PresetLoad, String> {
        let (meta, blob) = self.factory.get(index).ok_or("oob")?;
        Ok(PresetLoad {
            meta: meta.clone(),
            blob: blob.clone(),
            warnings: Vec::new(),
        })
    }
    fn factory_meta(&self, index: usize) -> Option<PresetMeta> {
        self.factory.get(index).map(|(m, _)| m.clone())
    }
    fn user_load(&self, _path: &Path) -> Result<PresetLoad, String> {
        Err("not implemented".into())
    }
    fn user_save(
        &self,
        name: &str,
        folder: Option<&str>,
        meta: &PresetMeta,
        blob: &[u8],
    ) -> Result<PathBuf, String> {
        self.saves.lock().unwrap().push((
            name.to_string(),
            folder.map(str::to_string),
            meta.clone(),
            blob.to_vec(),
        ));
        Ok(PathBuf::from(format!("/mock/{name}.toml")))
    }
    fn user_delete(&self, _path: &Path) -> Result<(), String> {
        Ok(())
    }
    fn user_rename(&self, _path: &Path, _new_name: &str) -> Result<PathBuf, String> {
        Ok(PathBuf::new())
    }
    fn user_move(&self, _path: &Path, _dest: Option<&str>) -> Result<PathBuf, String> {
        Ok(PathBuf::new())
    }
    fn user_create_folder(&self, _suggested: &str) -> Result<(PathBuf, String), String> {
        Ok((PathBuf::new(), String::new()))
    }
    fn user_rename_folder(
        &self,
        _old: &str,
        _new: &str,
    ) -> Result<(PathBuf, String), String> {
        Ok((PathBuf::new(), String::new()))
    }
    fn user_delete_folder(&self, _name: &str) -> Result<(), String> {
        Ok(())
    }
    fn list_user_tree(&self) -> Vec<UserFolderEntry> {
        Vec::new()
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn build(total: usize) -> (Controller<MockModel>, Arc<MockModel>, Receiver<ViewEvent>) {
    build_with(total, Box::<MockPresetStore>::default())
}

fn build_with(
    total: usize,
    store: Box<dyn PresetStore>,
) -> (Controller<MockModel>, Arc<MockModel>, Receiver<ViewEvent>) {
    let model = Arc::new(MockModel::new(total));
    let (ctrl, view_rx, _corpus) = Controller::new(model.clone(), store);
    (ctrl, model, view_rx)
}

fn drain(rx: &Receiver<ViewEvent>) -> Vec<ViewEvent> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push(ev);
    }
    out
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[test]
fn ui_set_param_emits_view_event() {
    let (mut ctrl, model, view_rx) = build(4);
    let id = ParamId::new(2);

    ctrl.ui_sender()
        .send(UiEvent::SetParam { id, plain: 0.5 })
        .unwrap();
    ctrl.tick();

    assert_eq!(model.get(id), 0.5);

    let events = drain(&view_rx);
    let changed = events
        .iter()
        .find_map(|ev| match ev {
            ViewEvent::ParamChanged { id: i, plain, norm, display } if *i == id => {
                Some((*plain, *norm, display.clone()))
            }
            _ => None,
        })
        .expect("expected ParamChanged for the edited id");
    assert_eq!(changed.0, 0.5);
    assert_eq!(changed.1, 0.5);
    assert_eq!(changed.2, "0.500");
}

#[test]
fn host_automation_echo_suppressed_during_gesture() {
    let (mut ctrl, model, view_rx) = build(4);
    let id = ParamId::new(1);

    // UI grabs the knob.
    ctrl.ui_sender()
        .send(UiEvent::BeginGesture { id })
        .unwrap();
    ctrl.tick();
    let _ = drain(&view_rx);

    // Host automation arrives mid-gesture.
    ctrl.host_sender()
        .send(HostEvent::ParamAutomation { id, plain: 0.7 })
        .unwrap();
    ctrl.tick();

    // Model adopts the value (audio path must see it)…
    assert_eq!(model.get(id), 0.7);
    // …but the view doesn't, until the gesture ends.
    let mid_events = drain(&view_rx);
    assert!(
        !mid_events.iter().any(|ev| matches!(
            ev,
            ViewEvent::ParamChanged { id: i, .. } if *i == id
        )),
        "ParamChanged echoed mid-gesture: {mid_events:?}"
    );

    // After gesture end + next host automation, echo resumes.
    ctrl.ui_sender().send(UiEvent::EndGesture { id }).unwrap();
    ctrl.tick();
    let _ = drain(&view_rx);

    ctrl.host_sender()
        .send(HostEvent::ParamAutomation { id, plain: 0.9 })
        .unwrap();
    ctrl.tick();
    assert_eq!(model.get(id), 0.9);
    let post_events = drain(&view_rx);
    assert!(
        post_events.iter().any(|ev| matches!(
            ev,
            ViewEvent::ParamChanged { id: i, plain, .. } if *i == id && *plain == 0.9
        )),
        "expected resumed echo after gesture end: {post_events:?}"
    );
}

#[test]
fn preset_load_emits_per_param_view_events() {
    // Build a blob the MockModel can restore: [n=3 u32] [0.1, 0.2, 0.3].
    let mut blob = Vec::new();
    blob.extend_from_slice(&3u32.to_le_bytes());
    for v in [0.1_f32, 0.2, 0.3] {
        blob.extend_from_slice(&v.to_le_bytes());
    }
    let meta = PresetMeta {
        name: "Test".to_string(),
        ..Default::default()
    };
    let store = Box::new(MockPresetStore::with_factory(vec![(meta.clone(), blob)]));
    let (mut ctrl, model, view_rx) = build_with(3, store);

    ctrl.ui_sender()
        .send(UiEvent::LoadPreset {
            source: PresetSource::Factory { index: 0 },
        })
        .unwrap();
    ctrl.tick();

    assert_eq!(model.get(ParamId::new(0)), 0.1);
    assert_eq!(model.get(ParamId::new(1)), 0.2);
    assert_eq!(model.get(ParamId::new(2)), 0.3);

    let events = drain(&view_rx);

    // PresetLoaded first.
    assert!(
        matches!(events.first(), Some(ViewEvent::PresetLoaded { meta: m, .. }) if m.name == "Test"),
        "first event should be PresetLoaded: {events:?}"
    );

    // One ParamChanged per param.
    let changed: Vec<(usize, f32)> = events
        .iter()
        .filter_map(|ev| match ev {
            ViewEvent::ParamChanged { id, plain, .. } => Some((id.raw(), *plain)),
            _ => None,
        })
        .collect();
    assert_eq!(changed, vec![(0, 0.1), (1, 0.2), (2, 0.3)]);

    // KeyModeChanged closes out the load.
    assert!(
        events
            .iter()
            .any(|ev| matches!(ev, ViewEvent::KeyModeChanged { .. })),
        "missing KeyModeChanged in {events:?}"
    );
}
