//! Controller round-trip tests against an in-memory `MockModel` (0035).
//!
//! No vizia/wry/engine touched — just the trait surface from `vxn_app`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, RwLock};

use vxn_app::{
    Controller, HostEvent, KeyMode, Layer, ParamDesc, ParamId, ParamKind, ParamModel, PresetLoad,
    PresetMeta, PresetSource, PresetStore, Taper, UiEvent, UserFolderEntry, UserPresetEntry,
    ViewEvent, Vxn1Params, Vxn1UiCustom, Vxn1ViewCustom,
};

// ── Test helpers ────────────────────────────────────────────────────────────
//
// Vxn1ViewCustom payloads ride `ViewEvent::Custom(Box<dyn Any + Send>)`.
// `matches!` can't downcast, so these wrappers match the common shapes.

fn as_vxn1_view(ev: &ViewEvent) -> Option<&Vxn1ViewCustom> {
    if let ViewEvent::Custom(p) = ev {
        p.downcast_ref::<Vxn1ViewCustom>()
    } else {
        None
    }
}

fn is_keymode_changed(ev: &ViewEvent) -> bool {
    matches!(as_vxn1_view(ev), Some(Vxn1ViewCustom::KeyModeChanged { .. }))
}

fn is_splitpoint_changed(ev: &ViewEvent) -> bool {
    matches!(as_vxn1_view(ev), Some(Vxn1ViewCustom::SplitPointChanged { .. }))
}

fn is_splitpoint_changed_with(ev: &ViewEvent, want: u8) -> bool {
    matches!(as_vxn1_view(ev), Some(Vxn1ViewCustom::SplitPointChanged { note }) if *note == want)
}

fn is_editlayer_changed_any(ev: &ViewEvent) -> bool {
    matches!(as_vxn1_view(ev), Some(Vxn1ViewCustom::EditLayerChanged { .. }))
}

fn is_editlayer_changed(ev: &ViewEvent, want: Layer) -> bool {
    matches!(as_vxn1_view(ev), Some(Vxn1ViewCustom::EditLayerChanged { layer }) if *layer == want)
}

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

impl Vxn1Params for MockModel {
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
}

// ── Shared test preset store ─────────────────────────────────────────────────
//
// Single configurable disk-backed store used by all tests.
//
// * `factory`: in-memory bank of factory entries (may be empty).
// * `root`: when `Some`, user operations hit real disk under that path; when
//   `None`, saves are recorded in `saves` in memory and other user ops are
//   no-ops.
// * `saves`: records `user_save` calls when `root` is `None`, so tests that
//   only care about the save side-effect don't need a tempdir.
//
// Formerly three separate structs (`MockPresetStore` / `TempPresetStore` /
// inline `MixedStore`); merged here since `TempPresetStore` and `MixedStore`
// duplicated `user_load` / `list_user_tree` verbatim.

type SaveRecord = (String, Option<String>, PresetMeta, Vec<u8>);

struct TestPresetStore {
    factory: Vec<(PresetMeta, Vec<u8>)>,
    root: Option<PathBuf>,
    saves: Mutex<Vec<SaveRecord>>,
}

impl TestPresetStore {
    /// In-memory store: no factory, saves are recorded but not written to disk.
    fn memory() -> Self {
        Self { factory: Vec::new(), root: None, saves: Mutex::new(Vec::new()) }
    }

    /// In-memory store pre-populated with factory entries.
    fn with_factory(entries: Vec<(PresetMeta, Vec<u8>)>) -> Self {
        Self { factory: entries, root: None, saves: Mutex::new(Vec::new()) }
    }

    /// Disk-backed store rooted at `root` with optional factory entries.
    fn disk(root: PathBuf, factory: Vec<(PresetMeta, Vec<u8>)>) -> Self {
        Self { factory, root: Some(root), saves: Mutex::new(Vec::new()) }
    }

    fn dir_for(&self, folder: Option<&str>) -> Option<PathBuf> {
        self.root.as_ref().map(|r| match folder {
            Some(f) => r.join(f),
            None => r.clone(),
        })
    }
}

impl Default for TestPresetStore {
    fn default() -> Self { Self::memory() }
}

impl PresetStore for TestPresetStore {
    fn factory_len(&self) -> usize { self.factory.len() }
    fn factory_load(&self, index: usize) -> Result<PresetLoad, String> {
        let (meta, blob) = self.factory.get(index).ok_or("oob")?;
        Ok(PresetLoad { meta: meta.clone(), blob: blob.clone(), warnings: Vec::new() })
    }
    fn factory_meta(&self, index: usize) -> Option<PresetMeta> {
        self.factory.get(index).map(|(m, _)| m.clone())
    }

    fn user_load(&self, path: &Path) -> Result<PresetLoad, String> {
        match &self.root {
            Some(_) => {
                let blob = fs::read(path).map_err(|e| e.to_string())?;
                let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                Ok(PresetLoad { meta: PresetMeta { name, ..Default::default() }, blob, warnings: Vec::new() })
            }
            None => Err("not implemented".into()),
        }
    }

    fn user_save(
        &self,
        name: &str,
        folder: Option<&str>,
        meta: &PresetMeta,
        blob: &[u8],
    ) -> Result<PathBuf, String> {
        if let Some(dir) = self.dir_for(folder) {
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let path = dir.join(format!("{name}.preset"));
            fs::write(&path, blob).map_err(|e| e.to_string())?;
            Ok(path)
        } else {
            self.saves.lock().unwrap().push((
                name.to_string(),
                folder.map(str::to_string),
                meta.clone(),
                blob.to_vec(),
            ));
            Ok(PathBuf::from(format!("/mock/{name}.toml")))
        }
    }

    fn user_delete(&self, path: &Path) -> Result<(), String> {
        if self.root.is_some() {
            fs::remove_file(path).map_err(|e| e.to_string())
        } else {
            Ok(())
        }
    }

    fn user_rename(&self, path: &Path, new_name: &str) -> Result<PathBuf, String> {
        if let Some(dir) = self.dir_for(None) {
            let parent = path.parent().unwrap_or(dir.as_path());
            let new_path = parent.join(format!("{new_name}.preset"));
            fs::rename(path, &new_path).map_err(|e| e.to_string())?;
            Ok(new_path)
        } else {
            Ok(PathBuf::new())
        }
    }

    fn user_move(&self, path: &Path, dest_folder: Option<&str>) -> Result<PathBuf, String> {
        if let Some(dir) = self.dir_for(dest_folder) {
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let name = path.file_name().ok_or("no name")?;
            let new_path = dir.join(name);
            fs::rename(path, &new_path).map_err(|e| e.to_string())?;
            Ok(new_path)
        } else {
            Ok(PathBuf::new())
        }
    }

    fn user_create_folder(&self, suggested: &str) -> Result<(PathBuf, String), String> {
        if let Some(root) = &self.root {
            let p = root.join(suggested);
            fs::create_dir_all(&p).map_err(|e| e.to_string())?;
            Ok((p, suggested.to_string()))
        } else {
            Ok((PathBuf::new(), String::new()))
        }
    }

    fn user_rename_folder(&self, old: &str, new: &str) -> Result<(PathBuf, String), String> {
        if let Some(root) = &self.root {
            let from = root.join(old);
            let to = root.join(new);
            fs::rename(&from, &to).map_err(|e| e.to_string())?;
            Ok((to, new.to_string()))
        } else {
            Ok((PathBuf::new(), String::new()))
        }
    }

    fn user_delete_folder(&self, name: &str) -> Result<(), String> {
        if let Some(root) = &self.root {
            fs::remove_dir_all(root.join(name)).map_err(|e| e.to_string())
        } else {
            Ok(())
        }
    }

    fn list_user_tree(&self) -> Vec<UserFolderEntry> {
        let Some(root) = &self.root else {
            return Vec::new();
        };
        let mut root_presets: Vec<UserPresetEntry> = Vec::new();
        let mut subs: Vec<(String, Vec<UserPresetEntry>)> = Vec::new();
        let Ok(rd) = fs::read_dir(root) else {
            return vec![UserFolderEntry { name: None, presets: Vec::new() }];
        };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_file() {
                if let Some(n) = p.file_stem().and_then(|s| s.to_str()) {
                    root_presets.push(UserPresetEntry {
                        path: p.clone(),
                        meta: PresetMeta { name: n.to_string(), ..Default::default() },
                        folder: None,
                    });
                }
            } else if ft.is_dir() {
                let Some(fname) = e.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                let mut presets = Vec::new();
                if let Ok(srd) = fs::read_dir(&p) {
                    for se in srd.flatten() {
                        let sp = se.path();
                        if let Some(n) = sp.file_stem().and_then(|s| s.to_str()) {
                            presets.push(UserPresetEntry {
                                path: sp.clone(),
                                meta: PresetMeta { name: n.to_string(), ..Default::default() },
                                folder: Some(fname.clone()),
                            });
                        }
                    }
                }
                presets.sort_by_key(|p| p.meta.name.to_lowercase());
                subs.push((fname, presets));
            }
        }
        root_presets.sort_by_key(|p| p.meta.name.to_lowercase());
        subs.sort_by_key(|s| s.0.to_lowercase());
        let mut out = vec![UserFolderEntry { name: None, presets: root_presets }];
        for (n, presets) in subs {
            out.push(UserFolderEntry { name: Some(n), presets });
        }
        out
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn build(total: usize) -> (Controller<MockModel>, Arc<MockModel>, Receiver<ViewEvent>) {
    build_with(total, Box::<TestPresetStore>::default())
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

/// Drain `rx` and collect the name from every `PresetLoaded` event.
/// Replaces the `drain(&view_rx).into_iter().filter_map(|ev| match ev {
/// ViewEvent::PresetLoaded { meta, .. } => Some(meta.name), _ => None }).collect()`
/// pattern used across several step-preset tests.
fn loaded_names(rx: &Receiver<ViewEvent>) -> Vec<String> {
    drain(rx)
        .into_iter()
        .filter_map(|ev| match ev {
            ViewEvent::PresetLoaded { meta, .. } => Some(meta.name),
            _ => None,
        })
        .collect()
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
        !mid_events.iter().any(|ev| matches!(ev,
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
        post_events.iter().any(|ev| matches!(ev,
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
    let store = Box::new(TestPresetStore::with_factory(vec![(meta.clone(), blob)]));
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
            .any(|ev| is_keymode_changed(ev)),
        "missing KeyModeChanged in {events:?}"
    );
    // 0053: the HTML keys panel needs the split-point echo so its
    // slider reseeds after a preset/state load.
    assert!(
        events
            .iter()
            .any(|ev| is_splitpoint_changed(ev)),
        "missing SplitPointChanged in {events:?}"
    );
}

#[test]
fn set_edit_layer_echoes_as_view_event() {
    // 0045: SetEditLayer is pure view state — controller mutates nothing,
    // but echoes EditLayerChanged so editors that don't own the layer-toggle
    // widget (HTML faceplate) can rebind per-patch panels.
    let (mut ctrl, _model, view_rx) = build(2);
    ctrl.ui_sender()
        .send(Vxn1UiCustom::SetEditLayer { layer: Layer::Lower }.into_event())
        .unwrap();
    ctrl.tick();
    let events = drain(&view_rx);
    assert!(
        events.iter().any(|ev| is_editlayer_changed(ev, Layer::Lower)),
        "missing EditLayerChanged(Lower) in {events:?}"
    );
}

#[test]
fn preset_load_snaps_edit_layer_to_upper_in_whole() {
    // Whole-mode preset load while the view sits on Lower would otherwise
    // leave the faceplate bound to Lower CLAP ids that the engine ignores
    // (engine reads Upper-only in Whole). Controller must echo
    // EditLayerChanged{Upper} to snap the view back.
    let mut blob = Vec::new();
    blob.extend_from_slice(&3u32.to_le_bytes());
    for v in [0.1_f32, 0.2, 0.3] {
        blob.extend_from_slice(&v.to_le_bytes());
    }
    let meta = PresetMeta {
        name: "Whole Preset".into(),
        ..Default::default()
    };
    let store = Box::new(TestPresetStore::with_factory(vec![(meta, blob)]));
    let (mut ctrl, model, view_rx) = build_with(3, store);
    model.set_key_mode(KeyMode::Whole);

    ctrl.ui_sender()
        .send(UiEvent::LoadPreset {
            source: PresetSource::Factory { index: 0 },
        })
        .unwrap();
    ctrl.tick();
    let events = drain(&view_rx);
    assert!(
        events.iter().any(|ev| is_editlayer_changed(ev, Layer::Upper)),
        "missing EditLayerChanged(Upper) snap after Whole-mode preset load: {events:?}"
    );
}

#[test]
fn preset_load_in_dual_mode_does_not_snap_edit_layer() {
    // In Dual/Split the user's edit-layer choice is theirs — controller
    // must not force it.
    let mut blob = Vec::new();
    blob.extend_from_slice(&3u32.to_le_bytes());
    for v in [0.1_f32, 0.2, 0.3] {
        blob.extend_from_slice(&v.to_le_bytes());
    }
    let store = Box::new(TestPresetStore::with_factory(vec![(
        PresetMeta::default(),
        blob,
    )]));
    let (mut ctrl, model, view_rx) = build_with(3, store);
    model.set_key_mode(KeyMode::Dual);

    ctrl.ui_sender()
        .send(UiEvent::LoadPreset {
            source: PresetSource::Factory { index: 0 },
        })
        .unwrap();
    ctrl.tick();
    let events = drain(&view_rx);
    assert!(
        !events
            .iter()
            .any(|ev| is_editlayer_changed_any(ev)),
        "unexpected EditLayerChanged in Dual-mode preset load: {events:?}"
    );
}

#[test]
fn set_key_mode_to_whole_snaps_edit_layer_to_upper() {
    let (mut ctrl, model, view_rx) = build(2);
    model.set_key_mode(KeyMode::Dual);
    ctrl.ui_sender()
        .send(Vxn1UiCustom::SetKeyMode { mode: KeyMode::Whole }.into_event())
        .unwrap();
    ctrl.tick();
    let events = drain(&view_rx);
    assert!(
        events.iter().any(|ev| is_editlayer_changed(ev, Layer::Upper)),
        "missing EditLayerChanged(Upper) on entry to Whole: {events:?}"
    );
}

#[test]
fn set_key_mode_to_dual_does_not_snap_edit_layer() {
    let (mut ctrl, model, view_rx) = build(2);
    model.set_key_mode(KeyMode::Whole);
    ctrl.ui_sender()
        .send(Vxn1UiCustom::SetKeyMode { mode: KeyMode::Dual }.into_event())
        .unwrap();
    ctrl.tick();
    let events = drain(&view_rx);
    assert!(
        !events
            .iter()
            .any(|ev| is_editlayer_changed_any(ev)),
        "unexpected EditLayerChanged on entry to Dual: {events:?}"
    );
}

#[test]
fn request_text_input_relays_to_open_text_input() {
    // 0048: faceplate posts `RequestTextInput`; controller relays
    // verbatim as `OpenTextInput` for the editor backend to intercept
    // and pop a native NSWindow. No model mutation.
    let (mut ctrl, _model, view_rx) = build(2);
    ctrl.ui_sender()
        .send(UiEvent::RequestTextInput {
            id: "ti7".into(),
            title: "Rename Preset".into(),
            initial: "Pad 1".into(),
        })
        .unwrap();
    ctrl.tick();
    let events = drain(&view_rx);
    assert!(
        events.iter().any(|ev| matches!(ev,
            ViewEvent::OpenTextInput { id, title, initial }
                if id == "ti7" && title == "Rename Preset" && initial == "Pad 1"
        )),
        "missing OpenTextInput(ti7) in {events:?}"
    );
}

#[test]
fn text_input_result_relays_back_to_page() {
    // Commit and cancel both round-trip through the controller so the
    // page's pending-callback map can fire from one dispatcher branch.
    let (mut ctrl, _model, view_rx) = build(2);
    ctrl.ui_sender()
        .send(UiEvent::TextInputResult {
            id: "ti7".into(),
            value: Some("Pad 2".into()),
        })
        .unwrap();
    ctrl.ui_sender()
        .send(UiEvent::TextInputResult {
            id: "ti8".into(),
            value: None,
        })
        .unwrap();
    ctrl.tick();
    let events = drain(&view_rx);
    assert!(
        events.iter().any(|ev| matches!(ev,
            ViewEvent::TextInputResult { id, value: Some(v) }
                if id == "ti7" && v == "Pad 2"
        )),
        "missing commit echo: {events:?}"
    );
    assert!(
        events.iter().any(|ev| matches!(ev,
            ViewEvent::TextInputResult { id, value: None } if id == "ti8"
        )),
        "missing cancel echo: {events:?}"
    );
}

#[test]
fn controller_save_then_list_round_trip() {
    // Real disk IO through `PresetStore` — proves the controller's
    // SavePreset → refresh_user_corpus → PresetCorpusChanged path actually
    // touches the filesystem the way the engine adapter expects.
    let tmp = tempfile::TempDir::new().unwrap();
    let store = Box::new(TestPresetStore::disk(tmp.path().to_path_buf(), Vec::new()));
    let model = Arc::new(MockModel::new(3));
    let (mut ctrl, view_rx, corpus) = Controller::new(model.clone(), store);

    // Initial corpus: empty user side, root folder slot present.
    {
        let c = corpus.lock().unwrap();
        assert_eq!(c.factory.len(), 0);
        assert_eq!(c.user.len(), 1);
        assert_eq!(c.user[0].name, None);
        assert!(c.user[0].presets.is_empty());
    }

    // Save into root.
    ctrl.ui_sender()
        .send(UiEvent::SavePreset {
            name: "Init".into(),
            folder: None,
        })
        .unwrap();
    // And one into a subfolder (creating it implicitly).
    ctrl.ui_sender()
        .send(UiEvent::SavePreset {
            name: "Brassy".into(),
            folder: Some("Lead".into()),
        })
        .unwrap();
    ctrl.tick();

    // Disk has both files.
    assert!(tmp.path().join("Init.preset").is_file());
    assert!(tmp.path().join("Lead/Brassy.preset").is_file());

    // Corpus reseeded: root has Init, Lead has Brassy.
    {
        let c = corpus.lock().unwrap();
        assert_eq!(c.user.len(), 2);
        assert_eq!(c.user[0].name, None);
        assert_eq!(c.user[0].presets.len(), 1);
        assert_eq!(c.user[0].presets[0].meta.name, "Init");
        assert_eq!(c.user[1].name.as_deref(), Some("Lead"));
        assert_eq!(c.user[1].presets.len(), 1);
        assert_eq!(c.user[1].presets[0].meta.name, "Brassy");
    }

    // One PresetCorpusChanged per save, each with a Some(follow) at the new
    // path (the cursor target the view jumps to).
    let events = drain(&view_rx);
    let follows: Vec<PathBuf> = events
        .iter()
        .filter_map(|ev| match ev {
            ViewEvent::PresetCorpusChanged { follow } => follow.clone(),
            _ => None,
        })
        .collect();
    assert_eq!(
        follows,
        vec![
            tmp.path().join("Init.preset"),
            tmp.path().join("Lead/Brassy.preset"),
        ]
    );
}

#[test]
fn step_preset_walks_combined_list_factory_then_user_alpha() {
    // 0049: prev/next walker. Combined order = factory alpha-by-name then
    // user alpha-by-name across all folders; wraps at either end. With no
    // prior preset, `delta=+1` seeds at index 0 and `delta=-1` at the last
    // entry — matches the vizia `step_index` semantics so the walker is
    // backend-agnostic.
    let blob_for = |v: f32| {
        let mut b = Vec::new();
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&v.to_le_bytes());
        b
    };
    // Factory entries deliberately not alpha-sorted in the bank so the
    // walker has to do the sort itself.
    let factory = vec![
        (PresetMeta { name: "Brass".into(), ..Default::default() }, blob_for(0.10)),
        (PresetMeta { name: "Aether".into(), ..Default::default() }, blob_for(0.20)),
        (PresetMeta { name: "Choir".into(), ..Default::default() }, blob_for(0.30)),
    ];
    let store = Box::new(TestPresetStore::with_factory(factory));
    let (mut ctrl, model, view_rx) = build_with(1, store);

    // No prior preset, delta=+1 → first by alpha order ("Aether" → 0.20).
    ctrl.ui_sender()
        .send(UiEvent::StepPreset { delta: 1 })
        .unwrap();
    ctrl.tick();
    assert!((model.get(ParamId::new(0)) - 0.20).abs() < 1e-6);
    assert_eq!(loaded_names(&view_rx), vec!["Aether"]);

    // Step forward → "Brass" → "Choir" → wrap to "Aether".
    for expected in ["Brass", "Choir", "Aether"] {
        ctrl.ui_sender()
            .send(UiEvent::StepPreset { delta: 1 })
            .unwrap();
        ctrl.tick();
        assert_eq!(loaded_names(&view_rx), vec![expected.to_string()]);
    }

    // Step backward from "Aether" wraps to "Choir".
    ctrl.ui_sender()
        .send(UiEvent::StepPreset { delta: -1 })
        .unwrap();
    ctrl.tick();
    assert_eq!(loaded_names(&view_rx), vec!["Choir".to_string()]);
}

#[test]
fn step_preset_spans_factory_into_user() {
    // 0049: factory entries come first, then user entries, both alpha. A
    // forward step from the last factory entry lands on the first user
    // entry — proves the walker treats the two halves as one ordered list.
    //
    // Uses `TestPresetStore::disk` with one factory entry and one user preset
    // on disk: factory side comes first in the combined walker, then the disk
    // entry; a second forward step crosses the factory→user boundary.
    let tmp = tempfile::TempDir::new().unwrap();

    let mut blob = Vec::new();
    blob.extend_from_slice(&1u32.to_le_bytes());
    blob.extend_from_slice(&0.42_f32.to_le_bytes());
    fs::write(tmp.path().join("UserFoo.preset"), &blob).unwrap();

    let factory = vec![
        (PresetMeta { name: "FactoryOnly".into(), ..Default::default() }, blob.clone()),
    ];
    let store = Box::new(TestPresetStore::disk(tmp.path().to_path_buf(), factory));
    let (mut ctrl, _model, view_rx) = build_with(1, store);

    // Forward from cold → factory "FactoryOnly" (comes first alphabetically in
    // the combined list since factory entries precede user entries).
    ctrl.ui_sender().send(UiEvent::StepPreset { delta: 1 }).unwrap();
    ctrl.tick();
    assert_eq!(loaded_names(&view_rx), vec!["FactoryOnly".to_string()]);

    // Next step crosses the factory→user boundary → first user entry "UserFoo".
    ctrl.ui_sender().send(UiEvent::StepPreset { delta: 1 }).unwrap();
    ctrl.tick();
    assert_eq!(loaded_names(&view_rx), vec!["UserFoo".to_string()]);
}

#[test]
fn editor_ready_replays_params_and_corpus() {
    // 0050 race fix: EditorReady kicks a full param broadcast, the KeyMode,
    // *and* a benign PresetCorpusChanged. The corpus signal is what
    // re-triggers the webview backend's corpus push when the very first
    // one raced ahead of the page's bootstrap script.
    let (mut ctrl, _model, view_rx) = build(2);
    ctrl.ui_sender().send(UiEvent::EditorReady).unwrap();
    ctrl.tick();
    let events = drain(&view_rx);
    assert!(
        events
            .iter()
            .filter(|ev| matches!(ev, ViewEvent::ParamChanged { .. }))
            .count()
            >= 2,
        "expected a ParamChanged per param: {events:?}",
    );
    assert!(
        events
            .iter()
            .any(|ev| is_keymode_changed(ev)),
        "expected KeyModeChanged: {events:?}",
    );
    // 0053: the HTML keys panel has no idle-poll loop, so EditorReady
    // also re-broadcasts the split point.
    assert!(
        events
            .iter()
            .any(|ev| is_splitpoint_changed(ev)),
        "expected SplitPointChanged: {events:?}",
    );
    assert!(
        events
            .iter()
            .any(|ev| matches!(ev, ViewEvent::PresetCorpusChanged { follow: None })),
        "expected PresetCorpusChanged (corpus re-push trigger): {events:?}",
    );
}

#[test]
fn set_split_point_writes_model_and_echoes() {
    // 0053: SetSplitPoint writes the (non-automatable) shared state and
    // echoes SplitPointChanged so the HTML keys panel's slider reseeds
    // — the vizia editor still poll-syncs from the model and ignores
    // the echo.
    let (mut ctrl, model, view_rx) = build(2);
    ctrl.ui_sender()
        .send(Vxn1UiCustom::SetSplitPoint { note: 48 }.into_event())
        .unwrap();
    ctrl.tick();
    assert_eq!(model.split_point(), 48);
    let events = drain(&view_rx);
    assert!(
        events
            .iter()
            .any(|ev| is_splitpoint_changed_with(ev, 48)),
        "missing SplitPointChanged(48): {events:?}",
    );
}

#[test]
fn out_of_band_model_change_does_not_emit_keymode_split() {
    // 0082: key-mode / split-point events fire from the load path's
    // `on_model_loaded` hook (and direct UI edits), never from a per-tick
    // poll of the model. Mutating the shared state behind the controller's
    // back and ticking must therefore emit nothing — if the old
    // poll-and-diff shim were still present this tick would (wrongly)
    // notice the diff and re-announce it.
    let (mut ctrl, model, view_rx) = build(2);
    // Drain any construction-time events.
    let _ = drain(&view_rx);

    model.set_key_mode(KeyMode::Dual);
    model.set_split_point(48);
    ctrl.tick();

    let events = drain(&view_rx);
    assert!(
        !events.iter().any(|ev| is_keymode_changed(ev)),
        "poll-and-diff regression: KeyModeChanged emitted from a bare tick: {events:?}",
    );
    assert!(
        !events.iter().any(|ev| is_splitpoint_changed(ev)),
        "poll-and-diff regression: SplitPointChanged emitted from a bare tick: {events:?}",
    );
}

#[test]
fn step_preset_empty_corpus_is_noop() {
    // Cold start with no factory and no user presets: StepPreset must not
    // emit a PresetLoaded or touch the model.
    let (mut ctrl, _model, view_rx) = build(1);
    ctrl.ui_sender().send(UiEvent::StepPreset { delta: 1 }).unwrap();
    ctrl.tick();
    let events = drain(&view_rx);
    assert!(
        !events.iter().any(|ev| matches!(ev, ViewEvent::PresetLoaded { .. })),
        "expected no PresetLoaded with empty corpus: {events:?}"
    );
}
