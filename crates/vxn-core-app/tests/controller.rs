//! Controller event-loop dispatch tests.
//!
//! Drives the controller with a fake `ParamModel` + `PresetStore` and
//! verifies the UiEvent → ViewEvent ordering vxn-1's WebView depends on:
//! - `SetParam` → model.set + `ParamChanged` echo
//! - host `ParamAutomation` during a gesture writes the model but
//!   suppresses the view echo
//! - `EditorReady` triggers a full param re-broadcast
//! - `Custom(...)` payload reaches the user closure
//! - `LoadPreset` calls restore_from_bytes and re-broadcasts

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use vxn_core_app::{
    ParamDesc, ParamId, ParamKind, ParamModel, PresetLoad, PresetMeta, PresetSource, PresetStore,
    Taper, UiEvent, UserFolderEntry, ViewEvent,
};
use vxn_core_app::events::HostEvent;

static DESC: ParamDesc = ParamDesc {
    name: "p",
    label: "P",
    min: 0.0,
    max: 1.0,
    default: 0.0,
    kind: ParamKind::Float { unit: "", taper: Taper::Linear },
};

struct FakeModel {
    values: RwLock<Vec<f32>>,
    gestures: RwLock<Vec<bool>>,
    restored: Mutex<Vec<u8>>,
}

impl FakeModel {
    fn new(n: usize) -> Arc<Self> {
        Arc::new(Self {
            values: RwLock::new(vec![0.0; n]),
            gestures: RwLock::new(vec![false; n]),
            restored: Mutex::new(Vec::new()),
        })
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
    fn get_normalized(&self, id: ParamId) -> f32 {
        DESC.to_normalized(self.get(id))
    }
    fn set_normalized(&self, id: ParamId, norm: f32) {
        self.set(id, DESC.from_normalized(norm));
    }
    fn gesture(&self, id: ParamId) -> bool {
        self.gestures.read().unwrap()[id.raw()]
    }
    fn set_gesture(&self, id: ParamId, on: bool) {
        self.gestures.write().unwrap()[id.raw()] = on;
    }
    fn descriptor(&self, _id: ParamId) -> Option<&'static ParamDesc> {
        Some(&DESC)
    }
    fn snapshot_bytes(&self) -> Vec<u8> {
        b"snapshot".to_vec()
    }
    fn restore_from_bytes(&self, blob: &[u8]) -> Result<(), String> {
        *self.restored.lock().unwrap() = blob.to_vec();
        Ok(())
    }
}

struct FakeStore {
    factory: Vec<PresetMeta>,
}

impl PresetStore for FakeStore {
    fn factory_len(&self) -> usize {
        self.factory.len()
    }
    fn factory_load(&self, index: usize) -> Result<PresetLoad, String> {
        Ok(PresetLoad {
            meta: self.factory[index].clone(),
            blob: format!("factory:{index}").into_bytes(),
            warnings: Vec::new(),
        })
    }
    fn factory_meta(&self, index: usize) -> Option<PresetMeta> {
        self.factory.get(index).cloned()
    }
    fn user_load(&self, _path: &Path) -> Result<PresetLoad, String> {
        Err("no user".into())
    }
    fn user_save(
        &self,
        name: &str,
        _folder: Option<&str>,
        _meta: &PresetMeta,
        _blob: &[u8],
    ) -> Result<PathBuf, String> {
        Ok(PathBuf::from(format!("/fake/{name}")))
    }
    fn user_delete(&self, _path: &Path) -> Result<(), String> {
        Ok(())
    }
    fn user_rename(&self, _path: &Path, new_name: &str) -> Result<PathBuf, String> {
        Ok(PathBuf::from(format!("/fake/{new_name}")))
    }
    fn user_move(&self, _path: &Path, _dest_folder: Option<&str>) -> Result<PathBuf, String> {
        Ok(PathBuf::from("/fake/moved"))
    }
    fn user_create_folder(&self, suggested: &str) -> Result<(PathBuf, String), String> {
        Ok((PathBuf::from(format!("/fake/{suggested}")), suggested.into()))
    }
    fn user_rename_folder(&self, _old: &str, new: &str) -> Result<(PathBuf, String), String> {
        Ok((PathBuf::from(format!("/fake/{new}")), new.into()))
    }
    fn user_delete_folder(&self, _name: &str) -> Result<(), String> {
        Ok(())
    }
    fn list_user_tree(&self) -> Vec<UserFolderEntry> {
        Vec::new()
    }
}

fn drain(rx: &std::sync::mpsc::Receiver<ViewEvent>) -> Vec<ViewEvent> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push(ev);
    }
    out
}

#[test]
fn set_param_emits_param_changed() {
    let model = FakeModel::new(4);
    let store = FakeStore { factory: Vec::new() };
    let (mut ctrl, rx, _corpus) =
        vxn_core_app::Controller::<FakeModel>::new(model.clone(), Box::new(store));

    ctrl.ui_sender()
        .try_send(UiEvent::SetParam {
            id: ParamId::new(2),
            plain: 0.75,
        })
        .unwrap();
    ctrl.tick_no_custom();

    let events = drain(&rx);
    assert_eq!(events.len(), 1, "got {events:?}");
    match &events[0] {
        ViewEvent::ParamChanged { id, plain, .. } => {
            assert_eq!(id.raw(), 2);
            assert!((plain - 0.75).abs() < 1e-6);
        }
        other => panic!("expected ParamChanged, got {other:?}"),
    }
    assert!((model.get(ParamId::new(2)) - 0.75).abs() < 1e-6);
}

#[test]
fn host_automation_during_gesture_suppresses_view_echo() {
    let model = FakeModel::new(2);
    let store = FakeStore { factory: Vec::new() };
    let (mut ctrl, rx, _corpus) =
        vxn_core_app::Controller::<FakeModel>::new(model.clone(), Box::new(store));

    // Start a gesture on param 0.
    ctrl.ui_sender()
        .try_send(UiEvent::BeginGesture { id: ParamId::new(0) })
        .unwrap();
    // Host pushes automation while gesture is live.
    ctrl.host_sender()
        .try_send(HostEvent::ParamAutomation {
            id: ParamId::new(0),
            plain: 0.42,
        })
        .unwrap();
    ctrl.tick_no_custom();

    // Model received the value (audio path needs it) but no ParamChanged
    // was sent (knob being dragged owns the visual).
    assert!((model.get(ParamId::new(0)) - 0.42).abs() < 1e-6);
    let events = drain(&rx);
    assert!(
        events.is_empty(),
        "view echoes were not suppressed during gesture: {events:?}"
    );

    // End gesture and push automation again — now the echo lands.
    ctrl.ui_sender()
        .try_send(UiEvent::EndGesture { id: ParamId::new(0) })
        .unwrap();
    ctrl.host_sender()
        .try_send(HostEvent::ParamAutomation {
            id: ParamId::new(0),
            plain: 0.9,
        })
        .unwrap();
    ctrl.tick_no_custom();

    let events = drain(&rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ViewEvent::ParamChanged { .. })),
        "missing ParamChanged after gesture end: {events:?}"
    );
}

#[test]
fn editor_ready_rebroadcasts_every_param() {
    let model = FakeModel::new(8);
    let store = FakeStore { factory: Vec::new() };
    let (mut ctrl, rx, _corpus) =
        vxn_core_app::Controller::<FakeModel>::new(model.clone(), Box::new(store));

    ctrl.ui_sender().try_send(UiEvent::EditorReady).unwrap();
    ctrl.tick_no_custom();

    let events = drain(&rx);
    let param_changes = events
        .iter()
        .filter(|e| matches!(e, ViewEvent::ParamChanged { .. }))
        .count();
    assert_eq!(param_changes, 8);
    // EditorReady also emits a benign PresetCorpusChanged for the
    // webview-init race (0050).
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ViewEvent::PresetCorpusChanged { .. }))
    );
}

#[test]
fn custom_payload_reaches_handler() {
    let model = FakeModel::new(2);
    let store = FakeStore { factory: Vec::new() };
    let (mut ctrl, _rx, _corpus) =
        vxn_core_app::Controller::<FakeModel>::new(model.clone(), Box::new(store));

    ctrl.ui_sender()
        .try_send(UiEvent::Custom(Box::new(42u32)))
        .unwrap();

    let seen = Arc::new(Mutex::new(None::<u32>));
    let seen_cb = seen.clone();
    ctrl.tick(
        &mut |c, payload| {
            if let Ok(v) = payload.downcast::<u32>() {
                *seen_cb.lock().unwrap() = Some(*v);
                // Verify the handler can call public helpers.
                c.broadcast_all_params();
            }
        },
        &mut |_, _| {},
        &mut |_| {},
    );

    assert_eq!(*seen.lock().unwrap(), Some(42));
}

#[test]
fn load_preset_restores_model_and_broadcasts() {
    let model = FakeModel::new(3);
    let store = FakeStore {
        factory: vec![PresetMeta {
            name: "Init".into(),
            ..Default::default()
        }],
    };
    let (mut ctrl, rx, _corpus) =
        vxn_core_app::Controller::<FakeModel>::new(model.clone(), Box::new(store));

    ctrl.ui_sender()
        .try_send(UiEvent::LoadPreset {
            source: PresetSource::Factory { index: 0 },
        })
        .unwrap();
    ctrl.tick_no_custom();

    assert_eq!(*model.restored.lock().unwrap(), b"factory:0");
    let events = drain(&rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ViewEvent::PresetLoaded { .. }))
    );
    let n_changed = events
        .iter()
        .filter(|e| matches!(e, ViewEvent::ParamChanged { .. }))
        .count();
    assert_eq!(n_changed, 3, "expected full broadcast, got {n_changed}");
}
