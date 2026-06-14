//! 0036 controller-in-wasm probe.
//!
//! Goal: prove the existing vxn-1 MVC controller (`vxn_app::Controller`,
//! which wraps `vxn_core_app::Controller`) compiles to
//! `wasm32-unknown-unknown` unchanged, and measure the resulting module
//! size — so ADR 0009 can decide controller placement (Rust-wasm reuse
//! vs JS reimplementation) on measured fact.
//!
//! Strategy: construct a *real* `Controller` with throwaway `ParamModel`
//! + `PresetStore` impls, post a few `UiEvent`s through the bounded
//! channel, `tick()` to drain them, and drain the resulting `ViewEvent`s.
//! This forces the whole controller code path (mpsc channels, the
//! `Arc<Mutex>` corpus, the param-broadcast loop, the `Box<dyn Any>`
//! custom-event downcast) into the binary, not just its type signatures.
//!
//! No wasm-bindgen — a raw C-ABI cdylib, exactly like the 0034 engine
//! spike, so the module instantiates inside an AudioWorklet/JS scope with
//! a hand-written glue layer.

use std::any::Any;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicBool, Ordering};

use vxn_app::{
    Controller, KeyMode, ParamDesc, ParamId, ParamModel, PresetLoad, PresetMeta, PresetStore,
    UiEvent, UserFolderEntry, ViewEvent, Vxn1Params, Vxn1UiCustom, Layer,
    TOTAL_PARAMS, desc_for_clap_id, patch_clap_id, PatchParam,
};

/// Minimal `ParamModel` + `Vxn1Params` backed by atomics — the same
/// shape vxn-clap's `SharedParams` uses (one `AtomicU32` per CLAP id,
/// f32 bit-cast). This is the audio-thread-safe param store the web port
/// would also build.
struct ProbeModel {
    vals: Vec<AtomicU32>,
    gestures: Vec<AtomicBool>,
    key_mode: AtomicU32,
    split: AtomicU32,
}

impl ProbeModel {
    fn new() -> Self {
        let mut vals = Vec::with_capacity(TOTAL_PARAMS);
        let mut gestures = Vec::with_capacity(TOTAL_PARAMS);
        for i in 0..TOTAL_PARAMS {
            let d = desc_for_clap_id(i).map_or(0.0, |d| d.default);
            vals.push(AtomicU32::new(d.to_bits()));
            gestures.push(AtomicBool::new(false));
        }
        Self {
            vals,
            gestures,
            key_mode: AtomicU32::new(0),
            split: AtomicU32::new(60),
        }
    }
}

impl ParamModel for ProbeModel {
    fn total(&self) -> usize {
        TOTAL_PARAMS
    }
    fn get(&self, id: ParamId) -> f32 {
        f32::from_bits(self.vals[id.raw()].load(Ordering::Relaxed))
    }
    fn set(&self, id: ParamId, plain: f32) {
        self.vals[id.raw()].store(plain.to_bits(), Ordering::Relaxed);
    }
    fn get_normalized(&self, id: ParamId) -> f32 {
        self.descriptor(id)
            .map_or(0.0, |d| d.to_fader(self.get(id)))
    }
    fn set_normalized(&self, id: ParamId, norm: f32) {
        let plain = self.descriptor(id).map_or(norm, |d| d.from_fader(norm));
        self.set(id, plain);
    }
    fn gesture(&self, id: ParamId) -> bool {
        self.gestures[id.raw()].load(Ordering::Relaxed)
    }
    fn set_gesture(&self, id: ParamId, on: bool) {
        self.gestures[id.raw()].store(on, Ordering::Relaxed);
    }
    fn descriptor(&self, id: ParamId) -> Option<&'static ParamDesc> {
        desc_for_clap_id(id.raw())
    }
    fn snapshot_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(TOTAL_PARAMS * 4);
        for v in &self.vals {
            out.extend_from_slice(&v.load(Ordering::Relaxed).to_le_bytes());
        }
        out
    }
    fn restore_from_bytes(&self, blob: &[u8]) -> Result<(), String> {
        if blob.len() != TOTAL_PARAMS * 4 {
            return Err("bad blob len".into());
        }
        for (i, chunk) in blob.chunks_exact(4).enumerate() {
            let bits = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            self.vals[i].store(bits, Ordering::Relaxed);
        }
        Ok(())
    }
}

impl Vxn1Params for ProbeModel {
    fn key_mode(&self) -> KeyMode {
        match self.key_mode.load(Ordering::Relaxed) {
            1 => KeyMode::Dual,
            2 => KeyMode::Split,
            _ => KeyMode::Whole,
        }
    }
    fn set_key_mode(&self, mode: KeyMode) {
        self.key_mode.store(mode as u32, Ordering::Relaxed);
    }
    fn set_key_mode_seeded(&self, mode: KeyMode) {
        self.set_key_mode(mode);
    }
    fn split_point(&self) -> u8 {
        self.split.load(Ordering::Relaxed) as u8
    }
    fn set_split_point(&self, note: u8) {
        self.split.store(note as u32, Ordering::Relaxed);
    }
}

/// No-op preset store. The web port's real store lives in JS / E019; the
/// probe only needs the controller to *link* against the trait surface.
struct NullStore;

impl PresetStore for NullStore {
    fn factory_len(&self) -> usize {
        0
    }
    fn factory_load(&self, _index: usize) -> Result<PresetLoad, String> {
        Err("no presets".into())
    }
    fn factory_meta(&self, _index: usize) -> Option<PresetMeta> {
        None
    }
    fn user_load(&self, _path: &Path) -> Result<PresetLoad, String> {
        Err("no presets".into())
    }
    fn user_save(
        &self,
        _name: &str,
        _folder: Option<&str>,
        _meta: &PresetMeta,
        _blob: &[u8],
    ) -> Result<PathBuf, String> {
        Err("readonly".into())
    }
    fn user_delete(&self, _path: &Path) -> Result<(), String> {
        Err("readonly".into())
    }
    fn user_rename(&self, _path: &Path, _new_name: &str) -> Result<PathBuf, String> {
        Err("readonly".into())
    }
    fn user_move(&self, _path: &Path, _dest_folder: Option<&str>) -> Result<PathBuf, String> {
        Err("readonly".into())
    }
    fn user_create_folder(&self, _suggested: &str) -> Result<(PathBuf, String), String> {
        Err("readonly".into())
    }
    fn user_rename_folder(&self, _old: &str, _new: &str) -> Result<(PathBuf, String), String> {
        Err("readonly".into())
    }
    fn user_delete_folder(&self, _name: &str) -> Result<(), String> {
        Err("readonly".into())
    }
    fn list_user_tree(&self) -> Vec<UserFolderEntry> {
        Vec::new()
    }
}

/// Single global controller instance (single-threaded main-thread model:
/// the `Arc<Mutex<Controller>>` of the native shell collapses to a plain
/// owned value here — no second thread touches it).
struct ProbeState {
    ctrl: Controller<ProbeModel>,
    view_rx: std::sync::mpsc::Receiver<ViewEvent>,
    ui_tx: std::sync::mpsc::SyncSender<UiEvent>,
}

static mut STATE: Option<ProbeState> = None;

/// Construct the controller. Mirrors the native `vxn-clap` setup path.
#[unsafe(no_mangle)]
pub extern "C" fn probe_init() {
    let model = Arc::new(ProbeModel::new());
    let (ctrl, view_rx, _corpus) = Controller::new(model, Box::new(NullStore));
    let ui_tx = ctrl.ui_sender();
    unsafe {
        STATE = Some(ProbeState { ctrl, view_rx, ui_tx });
    }
}

/// Post a normalized param set + gesture bracket + a custom key-mode
/// event, tick the controller, drain ViewEvents, and return how many
/// ViewEvents the drain produced. Exercises the marshalling-relevant
/// paths (param write echo, custom downcast, broadcast).
#[unsafe(no_mangle)]
pub extern "C" fn probe_roundtrip(clap_id: u32, norm: f32) -> u32 {
    let st = unsafe { (&raw mut STATE).as_mut().unwrap().as_mut().unwrap() };
    let id = ParamId::new(clap_id as usize);
    let _ = st.ui_tx.try_send(UiEvent::BeginGesture { id });
    let _ = st.ui_tx.try_send(UiEvent::SetParamNorm { id, norm });
    let _ = st.ui_tx.try_send(UiEvent::EndGesture { id });
    let _ = st
        .ui_tx
        .try_send(Vxn1UiCustom::SetKeyMode { mode: KeyMode::Dual }.into_event());
    let _ = st.ui_tx.try_send(UiEvent::EditorReady);
    st.ctrl.tick();
    let mut n = 0u32;
    while let Ok(ev) = st.view_rx.try_recv() {
        // Touch each variant so the codegen for ViewEvent stays live.
        match ev {
            ViewEvent::ParamChanged { .. } => n += 1,
            _ => n += 1,
        }
    }
    n
}

/// Bulk-preset-load shape: write all 165 params via the model directly
/// (what `restore_from_bytes` + broadcast does on a preset load), then
/// read one back. Returns the read-back value's bits.
#[unsafe(no_mangle)]
pub extern "C" fn probe_bulk_default(layer: u32, patch_idx: u32) -> u32 {
    let st = unsafe { (&raw mut STATE).as_ref().unwrap().as_ref().unwrap() };
    let layer = if layer == 0 { Layer::Upper } else { Layer::Lower };
    let pp = PatchParam::from_index(patch_idx as usize).unwrap_or(PatchParam::Cutoff);
    let id = patch_clap_id(layer, pp);
    st.ctrl.model().get(ParamId::new(id)).to_bits()
}

// Keep the type imports live even if a path above is optimised out.
#[allow(dead_code)]
fn _keep_alive(_: Box<dyn Any + Send>) {}
