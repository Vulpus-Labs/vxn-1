//! Controller — the sole non-audio mutator of the model.
//!
//! Owns the inbound UI + host queues and the outbound view queue.
//! [`Controller::tick`] is the only place model mutation happens off
//! the audio thread. Per-synth events arrive on `UiEvent::Custom` /
//! `HostEvent::Custom` and are handled by closures the synth supplies
//! to `tick`.

use std::any::Any;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};

use crate::events::{HostEvent, PresetSource, UiEvent, ViewEvent};
use crate::model::{ParamId, ParamModel};
use crate::preset::{PresetCorpus, PresetMeta, PresetStore};

/// Shared snapshot of the preset corpus the controller publishes for
/// the view. Read on idle when the controller emits
/// [`ViewEvent::PresetCorpusChanged`]; re-seeded by the controller
/// after every save / rename / delete / move / new-folder op.
pub type CorpusHandle = Arc<Mutex<PresetCorpus>>;

/// Bounded-channel depth. Sized for a preset-load burst (one
/// `ParamChanged` per CLAP id, ~hundreds) with headroom.
pub const CHANNEL_CAPACITY: usize = 1024;

/// Cheap-clone post handle for the UI side.
pub struct ControllerHandle {
    ui: SyncSender<UiEvent>,
}

impl Clone for ControllerHandle {
    fn clone(&self) -> Self {
        Self { ui: self.ui.clone() }
    }
}

impl ControllerHandle {
    /// Post a UI intent. Returns `Err` if the channel is full or the
    /// controller has been dropped.
    #[inline]
    pub fn post(&self, event: UiEvent) -> Result<(), TrySendError<UiEvent>> {
        self.ui.try_send(event)
    }
}

pub struct Controller<M: ParamModel> {
    model: Arc<M>,
    presets: Box<dyn PresetStore>,
    corpus: CorpusHandle,
    /// Last successfully loaded preset's source. Anchors the prev/next
    /// walker: `UiEvent::StepPreset` resolves this against the combined
    /// ordered list and advances by `delta`.
    current_source: Option<PresetSource>,
    /// Optional synthetic "no preset loaded yet" meta the controller
    /// emits as a [`ViewEvent::PresetLoaded`] on every [`UiEvent::EditorReady`]
    /// while `current_source` is `None`. Lets a synth seed its preset bar
    /// with a labelled default (e.g. vxn-2's "Init") before the preset
    /// epic ships.
    init_preset_meta: Option<PresetMeta>,
    ui_tx: SyncSender<UiEvent>,
    ui_rx: Receiver<UiEvent>,
    host_tx: SyncSender<HostEvent>,
    host_rx: Receiver<HostEvent>,
    view_tx: SyncSender<ViewEvent>,
    /// Set when `handle_ui` processes a [`UiEvent::EditorReady`]; consumed by
    /// callers via [`Self::take_editor_ready_flag`]. Wrapper controllers
    /// (vxn-1 / vxn-2) use this to detect a re-attached editor and force a
    /// republish of their non-param view state (key mode, split point,
    /// edit layer) — the editor's web page can reload without tearing down
    /// the plugin, so the per-instance state needs reseeding even though
    /// nothing in the model changed.
    editor_ready_pending: bool,
    /// Whether `handle_ui`'s `SetParam`/`SetParamNorm` and `handle_host`'s
    /// `StateLoaded` automatically echo `ParamChanged`/full-table view events.
    /// `true` (default) for vxn-1, whose CLAP shell has no dirty-bitset pump
    /// and depends on this echo (plus its own value-diff poll, which dedupes
    /// the overlap on the wire). `false` for vxn-2: its shell runs the
    /// dirty-bitset pump (ADR 0003 / E005) as the single Model→View emitter, so
    /// the echo would double every UI write and re-broadcast all params on a
    /// state load (ticket 0067). Gates only the model-backed echoes —
    /// `SetOpTab`, `RequestMatrixSnapshot`, preset/corpus, and gesture-gated
    /// `ParamAutomation` are untouched.
    echo_param_writes: bool,
}

impl<M: ParamModel> Controller<M> {
    /// Build a controller bound to `model` and a preset store. Returns
    /// the controller, the receiver end of the view-event channel, and
    /// the shared corpus snapshot.
    pub fn new(
        model: Arc<M>,
        presets: Box<dyn PresetStore>,
    ) -> (Self, Receiver<ViewEvent>, CorpusHandle) {
        let (ui_tx, ui_rx) = sync_channel(CHANNEL_CAPACITY);
        let (host_tx, host_rx) = sync_channel(CHANNEL_CAPACITY);
        let (view_tx, view_rx) = sync_channel(CHANNEL_CAPACITY);
        let factory: Vec<PresetMeta> = (0..presets.factory_len())
            .filter_map(|i| presets.factory_meta(i))
            .collect();
        let user = presets.list_user_tree();
        let corpus = Arc::new(Mutex::new(PresetCorpus { factory, user }));
        let ctrl = Self {
            model,
            presets,
            corpus: corpus.clone(),
            current_source: None,
            init_preset_meta: None,
            ui_tx,
            ui_rx,
            host_tx,
            host_rx,
            view_tx,
            editor_ready_pending: false,
            echo_param_writes: true,
        };
        (ctrl, view_rx, corpus)
    }

    /// Consume the pending-EditorReady flag. `true` means an
    /// [`UiEvent::EditorReady`] was processed since the last call;
    /// wrappers use this to reseed non-param view state on every editor
    /// attach (including page reloads inside a long-lived plugin
    /// instance).
    pub fn take_editor_ready_flag(&mut self) -> bool {
        std::mem::take(&mut self.editor_ready_pending)
    }

    pub fn handle(&self) -> ControllerHandle {
        ControllerHandle { ui: self.ui_tx.clone() }
    }

    pub fn ui_sender(&self) -> SyncSender<UiEvent> {
        self.ui_tx.clone()
    }

    pub fn host_sender(&self) -> SyncSender<HostEvent> {
        self.host_tx.clone()
    }

    pub fn model(&self) -> &Arc<M> {
        &self.model
    }

    pub fn preset_store(&self) -> &dyn PresetStore {
        &*self.presets
    }

    pub fn corpus_handle(&self) -> CorpusHandle {
        self.corpus.clone()
    }

    /// Register a synthetic "no preset loaded yet" meta. While no preset
    /// has been loaded, every [`UiEvent::EditorReady`] emits a
    /// [`ViewEvent::PresetLoaded`] with this meta before the param
    /// re-broadcast, so the editor's preset bar paints the configured
    /// label (e.g. "Init") immediately on first open. Pass `None` to
    /// clear.
    pub fn set_init_preset_meta(&mut self, meta: Option<PresetMeta>) {
        self.init_preset_meta = meta;
    }

    /// Disable the automatic `ParamChanged` echo on UI param writes and the
    /// full-table re-broadcast on state load. A synth whose CLAP shell owns a
    /// dirty-bitset pump (vxn-2 — the pump is the single Model→View emitter,
    /// ADR 0003 / E005) calls this with `false` at construction so the echo
    /// doesn't double every write. Defaults to `true` (vxn-1, no pump). See
    /// [`Self::echo_param_writes`].
    pub fn set_echo_param_writes(&mut self, echo: bool) {
        self.echo_param_writes = echo;
    }

    /// Re-read the user-side corpus from disk and refresh the shared
    /// snapshot. Factory entries are static; left alone.
    pub fn refresh_user_corpus(&self) {
        let user = self.presets.list_user_tree();
        if let Ok(mut c) = self.corpus.lock() {
            c.user = user;
        }
    }

    /// Drain inbound queues and apply their effects.
    ///
    /// UI drains first so an in-flight gesture is bracketed correctly
    /// when host automation arrives in the same tick — host events
    /// landing during a gesture are folded into the model (the audio
    /// path needs them) but their view echo is suppressed until the
    /// gesture ends.
    ///
    /// `on_custom_ui` / `on_custom_host` handle the `Custom` variants
    /// (per-synth events). They receive a mutable reference to the
    /// controller so they can call [`Self::broadcast_all_params`],
    /// [`Self::push_view_event`], etc.
    pub fn tick(
        &mut self,
        on_custom_ui: &mut dyn FnMut(&mut Controller<M>, Box<dyn Any + Send>),
        on_custom_host: &mut dyn FnMut(&mut Controller<M>, Box<dyn Any + Send>),
    ) {
        while let Ok(ev) = self.ui_rx.try_recv() {
            self.handle_ui(ev, on_custom_ui);
        }
        while let Ok(ev) = self.host_rx.try_recv() {
            self.handle_host(ev, on_custom_host);
        }
    }

    /// Convenience wrapper for synths with no custom events.
    pub fn tick_no_custom(&mut self) {
        self.tick(&mut |_, _| {}, &mut |_, _| {});
    }

    fn handle_ui(
        &mut self,
        ev: UiEvent,
        on_custom: &mut dyn FnMut(&mut Controller<M>, Box<dyn Any + Send>),
    ) {
        match ev {
            UiEvent::SetParam { id, plain } => {
                self.model.set(id, plain);
                // vxn-2's dirty-bitset pump re-emits this from the bit the
                // write just flipped; only echo when there's no pump (0067).
                if self.echo_param_writes {
                    self.emit_param_changed(id);
                }
            }
            UiEvent::SetParamNorm { id, norm } => {
                self.model.set_normalized(id, norm);
                if self.echo_param_writes {
                    self.emit_param_changed(id);
                }
            }
            UiEvent::BeginGesture { id } => {
                self.model.set_gesture(id, true);
            }
            UiEvent::EndGesture { id } => {
                self.model.set_gesture(id, false);
            }
            UiEvent::LoadPreset { source } => {
                self.load_preset(source, on_custom);
            }
            UiEvent::StepPreset { delta } => {
                self.step_preset(delta, on_custom);
            }
            UiEvent::SavePreset { name, folder } => {
                self.save_preset(name, folder);
            }
            UiEvent::RenamePreset { path, new_name } => {
                match self.presets.user_rename(&path, &new_name) {
                    Ok(new_path) => {
                        self.refresh_user_corpus();
                        self.push_view_event(ViewEvent::PresetCorpusChanged {
                            follow: Some(new_path),
                        });
                        self.send_status(format!("Renamed to {}", new_name.trim()));
                    }
                    Err(e) => self.send_status(format!("rename failed: {e}")),
                }
            }
            UiEvent::DeletePreset { path } => match self.presets.user_delete(&path) {
                Ok(()) => {
                    self.refresh_user_corpus();
                    self.push_view_event(ViewEvent::PresetCorpusChanged { follow: None });
                }
                Err(e) => self.send_status(format!("delete failed: {e}")),
            },
            UiEvent::MovePreset { path, dest_folder } => {
                match self.presets.user_move(&path, dest_folder.as_deref()) {
                    Ok(new_path) => {
                        self.refresh_user_corpus();
                        self.push_view_event(ViewEvent::PresetCorpusChanged {
                            follow: Some(new_path),
                        });
                    }
                    Err(e) => self.send_status(format!("move failed: {e}")),
                }
            }
            UiEvent::RenameFolder { old_name, new_name } => {
                match self.presets.user_rename_folder(&old_name, &new_name) {
                    Ok((_path, final_name)) => {
                        self.refresh_user_corpus();
                        self.push_view_event(ViewEvent::PresetCorpusChanged { follow: None });
                        self.send_status(format!("Renamed folder to {final_name}"));
                    }
                    Err(e) => self.send_status(format!("rename folder failed: {e}")),
                }
            }
            UiEvent::DeleteFolder { name } => match self.presets.user_delete_folder(&name) {
                Ok(()) => {
                    self.refresh_user_corpus();
                    self.push_view_event(ViewEvent::PresetCorpusChanged { follow: None });
                    self.send_status(format!("Deleted folder {name}"));
                }
                Err(e) => self.send_status(format!("delete folder failed: {e}")),
            },
            UiEvent::NewFolder { suggested } => match self.presets.user_create_folder(&suggested) {
                Ok(_) => {
                    self.refresh_user_corpus();
                    self.push_view_event(ViewEvent::PresetCorpusChanged { follow: None });
                }
                Err(e) => self.send_status(format!("create folder failed: {e}")),
            },
            UiEvent::RequestTextInput { id, title, initial } => {
                self.push_view_event(ViewEvent::OpenTextInput { id, title, initial });
            }
            UiEvent::TextInputResult { id, value } => {
                self.push_view_event(ViewEvent::TextInputResult { id, value });
            }
            UiEvent::EditorReady => {
                // Flag the attach for wrapper controllers (vxn-1's
                // KeyMode/Split republish, etc). Consumed via
                // `take_editor_ready_flag()` after `tick()` returns.
                self.editor_ready_pending = true;
                // Seed the preset bar with the configured init meta
                // (e.g. vxn-2 "Init") before the param broadcast so the
                // name display paints in the same tick the controls
                // hydrate. Skipped once a preset has actually been
                // loaded — the live `PresetLoaded` from `load_preset`
                // is the authoritative label after that.
                if self.current_source.is_none() {
                    if let Some(meta) = self.init_preset_meta.clone() {
                        self.push_view_event(ViewEvent::PresetLoaded {
                            meta,
                            source: None,
                            warnings: Vec::new(),
                        });
                    }
                }
                self.broadcast_all_params();
                // 0050 race fix: the webview backend's first corpus push
                // can land before the page's bootstrap script has
                // installed its corpus handler. The webview's
                // flush_view_events keys its retry off
                // PresetCorpusChanged, so emit a benign one here to
                // force a corpus re-push after the page reports ready.
                self.push_view_event(ViewEvent::PresetCorpusChanged { follow: None });
            }
            UiEvent::Custom(payload) => on_custom(self, payload),
        }
    }

    fn handle_host(
        &mut self,
        ev: HostEvent,
        on_custom: &mut dyn FnMut(&mut Controller<M>, Box<dyn Any + Send>),
    ) {
        match ev {
            HostEvent::ParamAutomation { id, plain } => {
                self.model.set(id, plain);
                // Echo to view unless the user is actively dragging this
                // param: the editor's signal is the source of truth
                // during a gesture; host automation would yank the knob.
                if !self.model.gesture(id) {
                    self.emit_param_changed(id);
                }
            }
            HostEvent::StateLoaded { blob } => {
                if let Err(e) = self.model.restore_from_bytes(&blob) {
                    self.send_status(format!("state load failed: {e}"));
                    return;
                }
                self.push_view_event(ViewEvent::PresetLoaded {
                    meta: PresetMeta::default(),
                    source: None,
                    warnings: Vec::new(),
                });
                // `restore_from_bytes` flips every dirty bit; vxn-2's pump
                // re-broadcasts the whole table next tick, so the explicit
                // broadcast here would double ~360 events on a load (0067).
                if self.echo_param_writes {
                    self.broadcast_all_params();
                }
            }
            HostEvent::Tempo { bpm: _ } => {
                // Routed to the engine on a separate channel per-synth.
            }
            HostEvent::Custom(payload) => on_custom(self, payload),
        }
    }

    fn load_preset(
        &mut self,
        source: PresetSource,
        _on_custom: &mut dyn FnMut(&mut Controller<M>, Box<dyn Any + Send>),
    ) {
        let loaded = match &source {
            PresetSource::Factory { index } => self.presets.factory_load(*index),
            PresetSource::User { path } => self.presets.user_load(path),
        };
        match loaded {
            Ok(load) => {
                if let Err(e) = self.model.restore_from_bytes(&load.blob) {
                    self.send_status(format!("preset apply failed: {e}"));
                    return;
                }
                self.current_source = Some(source.clone());
                self.push_view_event(ViewEvent::PresetLoaded {
                    meta: load.meta,
                    source: Some(source),
                    warnings: load.warnings,
                });
                self.broadcast_all_params();
            }
            Err(e) => self.send_status(format!("preset load failed: {e}")),
        }
    }

    /// Walk the combined factory + user preset list by `delta` and load
    /// the resulting entry. Order: factory entries first
    /// (alpha-by-name), then user entries (alpha-by-name across
    /// folders); wraps at either end. With no prior preset,
    /// `delta >= 0` seeds at the first entry, `delta < 0` at the last.
    fn step_preset(
        &mut self,
        delta: i32,
        on_custom: &mut dyn FnMut(&mut Controller<M>, Box<dyn Any + Send>),
    ) {
        let list = self.combined_preset_list();
        if list.is_empty() {
            self.send_status("No presets available".into());
            return;
        }
        let cur_idx = self
            .current_source
            .as_ref()
            .and_then(|c| list.iter().position(|s| s == c));
        let len = list.len() as i32;
        let next = match cur_idx {
            Some(i) => (i as i32 + delta).rem_euclid(len) as usize,
            None if delta >= 0 => 0,
            None => (len - 1) as usize,
        };
        let source = list[next].clone();
        self.load_preset(source, on_custom);
    }

    fn combined_preset_list(&self) -> Vec<PresetSource> {
        let corpus = match self.corpus.lock() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let mut factory: Vec<(usize, &PresetMeta)> = corpus.factory.iter().enumerate().collect();
        factory.sort_by_cached_key(|a| a.1.name.to_lowercase());
        let mut user: Vec<&crate::preset::UserPresetEntry> = corpus
            .user
            .iter()
            .flat_map(|f| f.presets.iter())
            .collect();
        user.sort_by_cached_key(|a| a.meta.name.to_lowercase());
        let mut out: Vec<PresetSource> = Vec::with_capacity(factory.len() + user.len());
        for (i, _) in factory {
            out.push(PresetSource::Factory { index: i });
        }
        for p in user {
            out.push(PresetSource::User { path: p.path.clone() });
        }
        out
    }

    fn save_preset(&mut self, name: String, folder: Option<String>) {
        let blob = self.model.snapshot_bytes();
        let meta = PresetMeta {
            name: name.clone(),
            ..Default::default()
        };
        match self
            .presets
            .user_save(&name, folder.as_deref(), &meta, &blob)
        {
            Ok(path) => {
                self.refresh_user_corpus();
                self.push_view_event(ViewEvent::PresetCorpusChanged {
                    follow: Some(path.clone()),
                });
                self.send_status(format!("Saved {name}"));
            }
            Err(e) => self.send_status(format!("save failed: {e}")),
        }
    }

    /// Send a `ParamChanged` for `id` with the model's current value.
    /// Public so a custom-event handler can re-emit after a synth-side
    /// write.
    pub fn emit_param_changed(&self, id: ParamId) {
        let plain = self.model.get(id);
        let norm = self.model.get_normalized(id);
        let display = self
            .model
            .descriptor(id)
            .map(|d| d.display(plain))
            .unwrap_or_default();
        self.push_view_event(ViewEvent::ParamChanged {
            id,
            plain,
            norm,
            display,
        });
    }

    /// Emit a `ParamChanged` for every id in the model.
    pub fn broadcast_all_params(&self) {
        let n = self.model.total();
        for i in 0..n {
            self.emit_param_changed(ParamId::new(i));
        }
    }

    /// Push an event onto the view-bound queue. Drops on full —
    /// preferable to blocking the controller; the view-event queue is
    /// sized for a preset-load burst.
    pub fn push_view_event(&self, ev: ViewEvent) {
        let _ = self.view_tx.try_send(ev);
    }

    fn send_status(&self, line: String) {
        self.push_view_event(ViewEvent::Status { line });
    }
}
