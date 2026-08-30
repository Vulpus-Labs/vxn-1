//! Cross-thread parameter store for the CLAP shell (ticket 0204).
//!
//! CLAP calls the `params` extension (`get_value`, `get_info`, `flush`) on the
//! **main thread** while the [`crate::Engine`] runs on the **audio thread**, so
//! the two need a lock-free view of the current param values plus a channel for
//! the non-automatable matrix topology. This store is that bridge:
//!
//! - **Params** — one `AtomicU32` (f32 bits) per CLAP id. Writes cross without
//!   locks: the audio thread stores host automation as it happens, the main
//!   thread reads it back for `get_value`.
//! - **Matrix topology** — a `Mutex` **the audio thread never takes** (0338).
//!   This used to be justified as changing "only on state/preset load", which
//!   stopped being true the moment [`Self::edit_matrix_slot`] started raising
//!   the reload flag: every combo pick in the matrix overlay then routed the
//!   render through the lock, one editor preemption away from an audible
//!   dropout. The guarded tables are now **main-thread only** — the store's own
//!   authoritative copy, read by `state.save` and the editor's echo — and the
//!   audio thread learns about topology exclusively over the lock-free
//!   [`crate::topology`] ring.
//! - **Topology ring** — an SPSC queue of [`TopoMsg`] records. A single-field
//!   edit crosses as one [`TopoMsg::Edit`] and costs the audio thread one field
//!   write; a bulk change (preset load, `state.load`, copy/reset layer) crosses
//!   as one [`TopoMsg::Snapshot`], and that snapshot is also the ring's
//!   overflow backstop. See the module doc there for the overflow policy.
//! - **`reload`** — set by [`Self::restore_from_bytes`] and the bulk patch ops;
//!   the audio thread swaps it to `false` at the top of `process` and re-syncs
//!   the **params** from the store. This is how a state load that lands while
//!   the plugin is active reaches the running engine. Draining a `Snapshot`
//!   implies the same re-sync, so the two can't come apart whichever order the
//!   producer's two stores land in.
//! - **Keyboard state** — one `AtomicU32` (0338): three flags and a MIDI note
//!   pack losslessly into a word, so the audio thread's `take_key_state` is a
//!   pair of atomic loads rather than a second lock on the render path.
//!
//! Depth stays param-authoritative (ADR 0001 §5 / 0205): the stored topology's
//! depths already mirror the depth params (the codec seeds them), so a reload
//! that pushes params-then-topology can't disagree.
//!
//! **The web build uses this store as a UI model only.** `vxn1b-web-controller`
//! shares `SharedParams` with the native shell, but the worklet's engine is fed
//! by `vxn1b-wasm`'s own event codec, so nothing over there ever drains the
//! topology ring: after enough edits it sits permanently full with a resync
//! owed. That is inert (the guarded tables — the only thing the web build reads
//! — stay correct, and a full ring costs one refused push per edit), but it does
//! mean [`SharedParams::topology_backlog`] and
//! [`SharedParams::topology_resync_pending`] are meaningless off the CLAP path.
//!
//! **Gesture channel (E038).** Each param carries an `AtomicBool` gesture flag
//! the editor raises/lowers around a knob drag, so the controller can bracket a
//! UI drag into one host automation edit and suppress host echo mid-gesture.
//! Off the E038 GUI path this stays all-`false` and costs nothing.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::engine::{Engine, KeyOp, KeyState, MatrixEdit};
use crate::matrix::MatrixTable;
use crate::params::{
    ClapRef, GLOBAL_PARAMS, Layer, MATRIX_SLOTS, PATCH_PARAMS, ParamId, Params, TOTAL_PARAMS,
    clap_ref, desc_for_clap_id, global_clap_id, patch_clap_id,
};
use crate::state::{LayerState, PluginState};
use crate::topology::{TOPO_RING_SLOTS, TopoMsg, TopologyRing};

/// Detune stamped on the copy by [`SharedParams::copy_layer`] (0265), in cents.
/// Small enough to read as one wide sound rather than two instruments, large
/// enough that the pair cannot null-double.
pub const COPY_DETUNE_CENTS: f32 = 6.0;

/// Patch params [`SharedParams::copy_layer`] leaves alone: the mixer strip.
/// These place the two copies against each other, so duplicating them would
/// defeat the point of the copy.
const COPY_LAYER_EXCLUDED: [ParamId; 4] =
    [ParamId::LayerLevel, ParamId::LayerMute, ParamId::LayerPan, ParamId::LayerDetune];

/// Lock-free param mirror + topology channel shared by the CLAP main and audio
/// threads. Seeded to the factory-default patch
/// ([`PluginState::factory_default`]).
pub struct SharedParams {
    values: Vec<AtomicU32>,
    /// Per-param live-drag flags the editor raises around a UI gesture (E038).
    gestures: Vec<AtomicBool>,
    /// One matrix topology **per layer** (0216): index 0 = Layer 1, 1 = Layer 2.
    ///
    /// **Main thread only** (0338). This is the store's authoritative copy —
    /// what `state.save` serialises and what the editor's echo diffs against.
    /// The audio thread's copy lives in the engine and is fed by `topo`.
    matrix: Mutex<[MatrixTable; 2]>,
    /// Topology deltas + snapshots on their way to the audio thread. Lock-free
    /// both ends; see [`crate::topology`].
    topo: TopologyRing,
    /// Raised when the **params** need a full re-sync (state load, preset load,
    /// copy/reset layer); the audio thread clears it and re-reads the store.
    /// Topology edits deliberately do *not* raise it — they ride `topo`.
    reload: AtomicBool,
    /// Non-automatable keyboard state (0219), packed into one word by
    /// [`KeyState::to_bits`]. Written by the controller tick from a
    /// `set_key_mode` / `set_split_point` custom op; the audio thread swaps
    /// `key_dirty` and re-applies it to the engine's [`KeyState`].
    key: AtomicU32,
    key_dirty: AtomicBool,
}

impl Default for SharedParams {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedParams {
    /// A store seeded from the factory-default patch (both layers).
    pub fn new() -> Self {
        let factory = PluginState::factory_default();
        let values = (0..TOTAL_PARAMS)
            .map(|id| {
                let r = clap_ref(id).expect("id < TOTAL_PARAMS");
                let layer = match r {
                    ClapRef::Patch(l, _) => l as usize,
                    ClapRef::Global(_) => 0,
                };
                AtomicU32::new(factory.layers[layer].params.get(r.inner()).to_bits())
            })
            .collect();
        let gestures = (0..TOTAL_PARAMS).map(|_| AtomicBool::new(false)).collect();
        Self {
            values,
            gestures,
            matrix: Mutex::new([factory.layers[0].matrix, factory.layers[1].matrix]),
            topo: TopologyRing::new(),
            reload: AtomicBool::new(false),
            key: AtomicU32::new(KeyState::default().to_bits()),
            key_dirty: AtomicBool::new(false),
        }
    }

    /// The authoritative topology tables. **Main thread only** — nothing
    /// reachable from `process` may call this (0338).
    #[inline]
    fn lock(&self) -> std::sync::MutexGuard<'_, [MatrixTable; 2]> {
        // Recover a poisoned lock rather than propagating a panic: the guarded
        // topology is a plain value that's still readable after any mid-write
        // panic (plugin code unwinds).
        self.matrix.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Build one layer's [`Params`] from the current CLAP value array: its own
    /// patch params plus the shared globals.
    fn layer_params(&self, layer: Layer) -> Params {
        let mut p = Params::default();
        for &inner in PATCH_PARAMS.iter() {
            let id = patch_clap_id(layer, inner).expect("patch param");
            p.set(inner, self.get(id));
        }
        for &g in GLOBAL_PARAMS.iter() {
            p.set(g, self.get(global_clap_id(g).expect("global param")));
        }
        p
    }

    /// Read a CLAP-id param value (`0.0` past the table).
    #[inline]
    pub fn get(&self, id: usize) -> f32 {
        self.values
            .get(id)
            .map_or(0.0, |a| f32::from_bits(a.load(Ordering::Relaxed)))
    }

    /// Write a CLAP-id param value, clamped to the descriptor range.
    #[inline]
    pub fn set(&self, id: usize, value: f32) {
        if let Some(desc) = desc_for_clap_id(id) {
            self.values[id].store(desc.clamp(value).to_bits(), Ordering::Relaxed);
        }
    }

    /// Read a CLAP-id param as a **fader position** in `[0, 1]` (`0.0` past the
    /// table). The editor's controller echoes both plain and this position on a
    /// `ParamChanged`, and the faders paint the thumb from it.
    ///
    /// This is `to_fader`, not `to_normalized`: the descriptor's taper is part
    /// of the calibration, not a display flourish. Cutoff is
    /// `Exp { mid: 800 }` over 16.35 Hz … 16 kHz, so a *linear* position would
    /// put 800 Hz at 5% of the travel and spend the top half of the fader
    /// between 8 k and 16 k — the whole usable low end crushed into the bottom
    /// centimetre. `to_fader` pins the midpoint to `mid` and gives each octave
    /// roughly equal travel. VXN1's `SharedParams` has always done this
    /// (`vxn-1/crates/vxn-engine/src/shared.rs`); VXN1b's fork read the linear
    /// pair, which is the entire behavioural difference in fader feel between
    /// the two synths (0243).
    ///
    /// Only the editor path goes through here — CLAP exchanges *plain* values
    /// against the descriptor range, and preset/state I/O is plain — so the
    /// taper never reaches host automation or the wire format.
    #[inline]
    pub fn get_normalized(&self, id: usize) -> f32 {
        match desc_for_clap_id(id) {
            Some(desc) => desc.to_fader(self.get(id)),
            None => 0.0,
        }
    }

    /// Write a CLAP-id param from a fader position in `[0, 1]` (clamped to
    /// range by `set`). Inverse of [`Self::get_normalized`], taper included, so
    /// a drag and the echo that answers it agree. No-op past the table.
    #[inline]
    pub fn set_normalized(&self, id: usize, norm: f32) {
        if let Some(desc) = desc_for_clap_id(id) {
            self.set(id, desc.from_fader(norm));
        }
    }

    /// Whether the editor is actively dragging `id` (E038). Host automation
    /// echo is suppressed while this is set so the knob doesn't fight the drag.
    #[inline]
    pub fn gesture(&self, id: usize) -> bool {
        self.gestures
            .get(id)
            .is_some_and(|g| g.load(Ordering::Relaxed))
    }

    /// Raise/lower the live-drag flag for `id` (E038). No-op past the table.
    #[inline]
    pub fn set_gesture(&self, id: usize, on: bool) {
        if let Some(g) = self.gestures.get(id) {
            g.store(on, Ordering::Relaxed);
        }
    }

    /// A copy of both layers' matrix topology (for `state.save`).
    pub fn matrix_snapshot(&self) -> [MatrixTable; 2] {
        *self.lock()
    }

    /// Whether the audio thread should re-sync the engine from this store; clears
    /// the flag. Call once at the top of `process`.
    #[inline]
    pub fn take_reload(&self) -> bool {
        self.reload.swap(false, Ordering::Acquire)
    }

    /// Apply a UI key-op to the shared [`KeyState`] and flag the audio thread to
    /// re-sync. Called on the controller (main) thread.
    pub fn apply_key_op(&self, op: KeyOp) {
        // Read-modify-write without a CAS loop: the key channel has exactly one
        // writer (the CLAP main thread — controller tick, state load, preset
        // load), and the audio thread only ever reads.
        let mut key = self.key_state();
        key.apply(op);
        self.key.store(key.to_bits(), Ordering::Relaxed);
        self.key_dirty.store(true, Ordering::Release);
    }

    /// Duplicate `from`'s patch onto `to` — params and matrix topology.
    ///
    /// **Excludes the mixer strip.** `LayerLevel`, `LayerMute`, `LayerPan` and
    /// `LayerDetune` are balance and placement *between* the two copies, not
    /// part of the sound, so they stay as the player set them.
    ///
    /// **Stamps a detune offset**, and this is required rather than a nicety:
    /// `lane_phase` is a fixed function of lane index with no seed and both
    /// allocators pick the same lane for the same note, so an exact copy with
    /// `MasterDrift` at 0 renders *bit-identical* layers — +6 dB and no width at
    /// all. `to`'s [`ParamId::LayerDetune`] is set to [`COPY_DETUNE_CENTS`],
    /// leaving `from`'s alone, so the pair sits a few cents apart out of the box.
    /// `layer_detune` is the right knob rather than the per-osc `Fine` params:
    /// it moves the layer's whole pitch base, and it keeps the copy's one
    /// sound-affecting edit visible in a single control the player can undo by
    /// eye.
    ///
    /// **Levels are deliberately not trimmed.** Both `LayerLevel`s default to
    /// 1.0, so a copy is roughly +6 dB. The balance is the player's, the detune
    /// takes some of the coherence out of the sum, and a silent gain change on a
    /// button press is worse than a loud one.
    ///
    /// Echo to the host and the faceplate is free: the audio thread's
    /// `take_reload` re-syncs the engine, the per-block publish pushes the
    /// changed ids to the host, and the timer tick's param diff + matrix echo
    /// repaint the editor. Gesture flags are **not** raised — they exist to
    /// suppress host echo during a live drag and would only fight the repaint.
    ///
    /// Copying while in Single mode would do nothing audible, so it also
    /// switches to Dual. An existing Split is left alone — the player chose that
    /// routing.
    pub fn copy_layer(&self, from: Layer, to: Layer) {
        if from == to {
            return;
        }
        for &inner in PATCH_PARAMS.iter() {
            if COPY_LAYER_EXCLUDED.contains(&inner) {
                continue;
            }
            let (Some(src), Some(dst)) = (patch_clap_id(from, inner), patch_clap_id(to, inner))
            else {
                continue;
            };
            self.set(dst, self.get(src));
        }
        {
            let mut m = self.lock();
            m[to as usize] = m[from as usize];
        }
        if let Some(id) = patch_clap_id(to, ParamId::LayerDetune) {
            self.set(id, COPY_DETUNE_CENTS);
        }
        if !self.key_state().layer2_on {
            self.apply_key_op(KeyOp::SetKeyMode(1));
        }
        // Topology first, params second: `reload` is only ever observed after
        // the snapshot it belongs with is already queued.
        self.request_topology_resync();
        self.reload.store(true, Ordering::Release);
    }

    /// Reset one layer to the factory patch (0307): every patch param back to
    /// its descriptor default and the layer's matrix topology back to
    /// [`crate::matrix::default_patch`].
    ///
    /// Installs [`LayerState::factory_default`] rather than rebuilding the
    /// notion of "default" here, so reset, `Engine::new` and the state blob all
    /// agree on what a fresh layer is — including the depth-authority contract
    /// (slot-depth params seeded from the matrix, 0205), which a naive
    /// per-param loop over descriptor defaults would silently break.
    ///
    /// **Unlike [`Self::copy_layer`], this does NOT spare the mixer strip.**
    /// Copy excludes level / mute / pan / detune because writing a sound onto
    /// the other layer should not move where that layer sits in the mix. Reset
    /// is the opposite intent — the player is asking for a blank layer, and a
    /// "blank" one that stays muted at -6 dB hard left is a puzzle, not a
    /// courtesy. So the strip resets too, and the two ops deliberately do not
    /// share [`COPY_LAYER_EXCLUDED`].
    ///
    /// Key state is untouched: layer enable and the split are properties of how
    /// the layers share the keyboard, not of either layer's patch. Copy turns
    /// Layer 2 on because a copy to a silent layer is pointless; reset has no
    /// such reason.
    ///
    /// Raises no gesture flags — same as `copy_layer`. The shell brackets the
    /// whole thing as one host edit.
    pub fn reset_layer(&self, layer: Layer) {
        let factory = LayerState::factory_default();
        for &inner in PATCH_PARAMS.iter() {
            let Some(dst) = patch_clap_id(layer, inner) else {
                continue;
            };
            self.set(dst, factory.params.get(inner));
        }
        {
            let mut m = self.lock();
            m[layer as usize] = factory.matrix;
        }
        self.request_topology_resync();
        self.reload.store(true, Ordering::Release);
    }

    /// Apply a UI matrix-topology edit to the store's tables and post it to the
    /// audio thread (0338). Out-of-range slot indices are ignored.
    ///
    /// **This is the path that used to make the render take a lock.** It raises
    /// no reload flag and triggers no whole-patch re-sync: the record crosses on
    /// the topology ring, and the audio thread applies exactly the one field it
    /// names, on the next block. Depth is not a topology field — it is a CLAP
    /// param and rides the value store.
    pub fn edit_matrix_slot(&self, edit: MatrixEdit) {
        {
            let mut m = self.lock();
            crate::topology::apply_edit(&mut m[edit.layer as usize], edit);
        }
        self.publish_topology(TopoMsg::Edit(edit));
    }

    /// Post one record to the audio thread, or fall back to the snapshot path.
    ///
    /// A resync already owed supersedes an individual edit — the snapshot it
    /// will carry is taken from the table the edit has just been applied to —
    /// so in that state the record is deliberately withheld rather than queued.
    fn publish_topology(&self, msg: TopoMsg) {
        if self.topo.resync_pending() || !self.topo.try_push(msg) {
            self.topo.request_resync();
        }
        self.service_topology_resync();
    }

    /// Declare that the whole topology changed: the audio thread must adopt the
    /// store's tables wholesale. The bulk path (preset load, `state.load`,
    /// copy/reset layer) and the ring's overflow backstop are the same code.
    pub fn request_topology_resync(&self) {
        self.topo.request_resync();
        self.service_topology_resync();
    }

    /// Publish the owed snapshot if there is one and the ring has room.
    /// **Main thread**; idempotent, and a no-op in the overwhelmingly common
    /// case where nothing is owed. The CLAP shell also calls this from its
    /// editor tick so a snapshot deferred by a full ring cannot sit unsent.
    pub fn service_topology_resync(&self) {
        // Cheap gates before the table copy: nothing owed, or nowhere to put it.
        if !self.topo.resync_pending() || self.topo.len() >= TOPO_RING_SLOTS {
            return;
        }
        let snapshot = *self.lock();
        if self.topo.try_push(TopoMsg::Snapshot(snapshot)) {
            self.topo.clear_resync();
        }
    }

    /// Drain the topology channel onto `engine`. **Audio thread**, once at the
    /// top of `process`, before the param re-sync.
    ///
    /// Wait-free: a bounded number of pops, each a plain copy out of a
    /// pre-allocated cell, and a field write on the engine's own table. No
    /// lock, no allocation, no whole-patch rebuild for a single-field edit.
    ///
    /// Returns whether a snapshot was applied. A snapshot is only ever produced
    /// by a bulk change, which also rewrote the param store, so the caller must
    /// treat `true` as a reload: that makes the producer's two stores
    /// order-independent, and it re-seeds the slot depths the snapshot
    /// deliberately left alone.
    pub fn drain_topology(&self, engine: &mut Engine) -> bool {
        let mut saw_snapshot = false;
        while let Some(msg) = self.topo.pop() {
            match msg {
                TopoMsg::Edit(edit) => {
                    crate::topology::apply_edit(engine.matrix_mut(edit.layer), edit);
                }
                TopoMsg::Snapshot(tables) => {
                    saw_snapshot = true;
                    for layer in [Layer::L1, Layer::L2] {
                        crate::topology::apply_snapshot(
                            engine.matrix_mut(layer),
                            &tables[layer as usize],
                        );
                    }
                }
            }
        }
        saw_snapshot
    }

    /// Records queued on the topology channel (tests / diagnostics).
    #[inline]
    pub fn topology_backlog(&self) -> usize {
        self.topo.len()
    }

    /// Whether a full-table resync is owed because the ring overflowed (tests).
    #[inline]
    pub fn topology_resync_pending(&self) -> bool {
        self.topo.resync_pending()
    }

    /// The current keyboard state, if a key-op landed since the last call
    /// (clears the dirty flag). The audio thread applies it via
    /// [`crate::Engine::set_key_state`]. `None` means no change — the common case.
    #[inline]
    pub fn take_key_state(&self) -> Option<KeyState> {
        if self.key_dirty.swap(false, Ordering::Acquire) {
            Some(self.key_state())
        } else {
            None
        }
    }

    /// The current keyboard state **without** consuming the dirty flag (0221).
    /// The main thread's editor echo reads it every tick and diffs; only the
    /// audio thread's [`Self::take_key_state`] may clear the flag, or a tick that
    /// happened to land first would swallow the re-sync.
    #[inline]
    pub fn key_state(&self) -> KeyState {
        KeyState::from_bits(self.key.load(Ordering::Relaxed))
    }

    /// Snapshot the whole store to a `clap.state` blob (params + topology).
    pub fn snapshot_bytes(&self) -> Vec<u8> {
        let state = self.to_state();
        let mut blob = Vec::with_capacity(TOTAL_PARAMS * 4 + 64);
        // Writing to a `Vec` can't fail.
        let _ = state.write(&mut blob);
        blob
    }

    /// Build a two-layer [`PluginState`] from the current store contents. Each
    /// layer's params come from its CLAP block (+ shared globals); its matrix
    /// topology comes from the per-layer topology channel, with slot depths
    /// re-seeded from the params so depth stays param-authoritative. The
    /// keyboard record rides along from the key channel, so a `state.save`
    /// carries the routing mode as well as the two patches.
    pub fn to_state(&self) -> PluginState {
        self.state_with(self.matrix_snapshot())
    }

    /// [`Self::to_state`] with the topology supplied by the caller rather than
    /// read from the guarded tables — everything else (params, key record,
    /// depth re-seeding) is identical.
    ///
    /// This is the **audio thread's** re-sync path (0338) and the reason it can
    /// stay lock-free: it is passed the engine's own tables, which the topology
    /// ring has already brought up to date, so the render never reaches for the
    /// store's copy.
    pub fn state_with(&self, matrices: [MatrixTable; 2]) -> PluginState {
        let layers = [Layer::L1, Layer::L2].map(|layer| {
            let params = self.layer_params(layer);
            let mut matrix = matrices[layer as usize];
            for s in 0..MATRIX_SLOTS {
                matrix.slots[s].depth = params.slot_depth(s);
            }
            LayerState { params, matrix }
        });
        PluginState { layers, key: self.key_state() }
    }

    /// Apply a two-layer `clap.state` blob. On success overwrites every param +
    /// both topologies and raises [`Self::take_reload`]. Returns `Err` (leaving
    /// the store untouched) for an empty/undecodable blob — the 0196 contract: an
    /// invalid restore must report failure, not silently accept.
    pub fn restore_from_bytes(&self, blob: &[u8]) -> Result<(), String> {
        let state = PluginState::read(&mut &blob[..]).map_err(|e| e.to_string())?;
        for layer in [Layer::L1, Layer::L2] {
            let p = &state.layers[layer as usize].params;
            for &inner in PATCH_PARAMS.iter() {
                let id = patch_clap_id(layer, inner).expect("patch param");
                self.values[id].store(p.get(inner).to_bits(), Ordering::Relaxed);
            }
        }
        // Globals are saved identically in both layers; take Layer 1's copy.
        let g = &state.layers[0].params;
        for &gp in GLOBAL_PARAMS.iter() {
            let id = global_clap_id(gp).expect("global param");
            self.values[id].store(g.get(gp).to_bits(), Ordering::Relaxed);
        }
        *self.lock() = [state.layers[0].matrix, state.layers[1].matrix];
        // Keyboard state travels its own channel to the engine, so the
        // reload flag alone would not carry it: raise `key_dirty` too, and the
        // audio thread's `take_key_state` applies the loaded routing mode.
        self.key.store(state.key.to_bits(), Ordering::Relaxed);
        self.key_dirty.store(true, Ordering::Release);
        // Topology is a third channel again (0338): a load is a bulk change, so
        // it crosses as one snapshot rather than 32 edits. Queued before
        // `reload` is raised, so an audio thread that sees the flag already has
        // the snapshot in front of it.
        self.request_topology_resync();
        self.reload.store(true, Ordering::Release);
        Ok(())
    }

    /// A freshly-decoded engine-ready [`PluginState`] out of the store, topology
    /// included. **Main thread** (`activate`, the web build, tests) — the audio
    /// thread's reload uses [`Self::state_with`] instead, which takes no lock.
    pub fn engine_state(&self) -> PluginState {
        self.to_state()
    }
}

// ── ParamModel trait (vxn-core-app) ───────────────────────────────────────────
//
// The E038 editor's [`vxn_core_app::Controller`] drives the parameter store
// through [`vxn_core_app::ParamModel`]; this is the adaptor that lets it. Pure
// delegation to the inherent methods above. `SharedParams` stays trait-free on
// the audio path; the trait surface is only the controller's generic
// seam. Lives in-crate so the orphan rules don't bite (both `SharedParams` and
// the trait would be foreign to the clap crate).

impl vxn_core_app::ParamModel for SharedParams {
    fn total(&self) -> usize {
        TOTAL_PARAMS
    }

    fn get(&self, id: vxn_core_app::ParamId) -> f32 {
        SharedParams::get(self, id.raw())
    }

    fn set(&self, id: vxn_core_app::ParamId, plain: f32) {
        SharedParams::set(self, id.raw(), plain);
    }

    fn get_normalized(&self, id: vxn_core_app::ParamId) -> f32 {
        SharedParams::get_normalized(self, id.raw())
    }

    fn set_normalized(&self, id: vxn_core_app::ParamId, norm: f32) {
        SharedParams::set_normalized(self, id.raw(), norm);
    }

    fn gesture(&self, id: vxn_core_app::ParamId) -> bool {
        SharedParams::gesture(self, id.raw())
    }

    fn set_gesture(&self, id: vxn_core_app::ParamId, on: bool) {
        SharedParams::set_gesture(self, id.raw(), on);
    }

    fn descriptor(&self, id: vxn_core_app::ParamId) -> Option<&'static vxn_core_app::ParamDesc> {
        desc_for_clap_id(id.raw())
    }

    fn snapshot_bytes(&self) -> Vec<u8> {
        SharedParams::snapshot_bytes(self)
    }

    fn restore_from_bytes(&self, blob: &[u8]) -> Result<(), String> {
        SharedParams::restore_from_bytes(self, blob)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::MatrixField;
    use crate::matrix::{DestId, SourceId};
    use crate::params::{ParamId, clap_id_of};

    #[test]
    fn seeds_to_factory_default() {
        let sp = SharedParams::new();
        let factory = PluginState::factory_default();
        for id in 0..TOTAL_PARAMS {
            let r = clap_ref(id).unwrap();
            let layer = match r {
                ClapRef::Patch(l, _) => l as usize,
                ClapRef::Global(_) => 0,
            };
            assert_eq!(
                sp.get(id),
                factory.layers[layer].params.get(r.inner()),
                "param {id}"
            );
        }
        // The seeded Amp depth is present in the store on both layers.
        assert_eq!(sp.get(clap_id_of(Layer::L1, ParamId::MatrixSlot0Depth)), 1.0);
        assert_eq!(sp.get(clap_id_of(Layer::L2, ParamId::MatrixSlot0Depth)), 1.0);
    }

    #[test]
    fn set_clamps_and_reads_back() {
        let sp = SharedParams::new();
        sp.set(ParamId::Resonance as usize, 9.0);
        assert_eq!(sp.get(ParamId::Resonance as usize), 1.0);
        sp.set(ParamId::Cutoff as usize, 500.0);
        assert_eq!(sp.get(ParamId::Cutoff as usize), 500.0);
    }

    #[test]
    fn snapshot_restore_round_trips_and_flags_reload() {
        let sp = SharedParams::new();
        // Distinct edits on each layer to prove per-layer round-trip.
        sp.set(clap_id_of(Layer::L1, ParamId::Cutoff), 1234.0);
        sp.set(clap_id_of(Layer::L2, ParamId::Cutoff), 220.0);
        {
            let mut m = sp.lock();
            m[1].slots[5] = crate::matrix::MatrixSlot {
                source: crate::matrix::SourceId::Lfo2,
                dest: crate::matrix::DestId::Cutoff,
                depth: 0.0,
                polarity: crate::matrix::Polarity::Direct,
                shape: crate::matrix::Shape::Lin,
                enabled: true,
                scale_src: crate::matrix::SourceId::None,
                scale_shape: crate::matrix::Shape::Lin,
            };
        }
        let blob = sp.snapshot_bytes();

        let sp2 = SharedParams::new();
        assert!(!sp2.take_reload());
        sp2.restore_from_bytes(&blob).unwrap();
        assert!(sp2.take_reload(), "restore must flag a reload");
        assert!(!sp2.take_reload(), "flag clears after one read");
        assert_eq!(sp2.get(clap_id_of(Layer::L1, ParamId::Cutoff)), 1234.0);
        assert_eq!(sp2.get(clap_id_of(Layer::L2, ParamId::Cutoff)), 220.0);
        // Layer 2's private matrix edit survived on layer 2, not layer 1.
        assert_eq!(sp2.matrix_snapshot()[1].slots[5].source, crate::matrix::SourceId::Lfo2);
        assert!(!sp2.matrix_snapshot()[0].slots[5].is_active());
    }

    #[test]
    fn key_op_channel_is_dirty_once_and_carries_state() {
        use crate::engine::{KeyMode, KeyOp};
        let sp = SharedParams::new();
        // Clean by default — no spurious re-sync.
        assert!(sp.take_key_state().is_none());

        sp.apply_key_op(KeyOp::SetKeyMode(1)); // enable → Dual
        let k = sp.take_key_state().expect("dirty after a key-op");
        assert_eq!(k.key_mode(), KeyMode::Dual);
        assert!(k.layer2_on);
        // Flag clears after one read.
        assert!(sp.take_key_state().is_none());

        sp.apply_key_op(KeyOp::SetSplitPoint(48));
        sp.apply_key_op(KeyOp::SetKeyMode(2)); // Split, keeps the point
        let k = sp.take_key_state().unwrap();
        assert_eq!(k.key_mode(), KeyMode::Split);
        assert_eq!(k.split_point, 48);
    }

    #[test]
    fn snapshot_restore_carries_key_state_and_flags_the_key_channel() {
        use crate::engine::{KeyMode, KeyOp};
        let sp = SharedParams::new();
        sp.apply_key_op(KeyOp::SetSplitPoint(43));
        sp.apply_key_op(KeyOp::SetKeyMode(2)); // Split
        sp.apply_key_op(KeyOp::SetLfo2Link(true));
        // Drain the dirty flag: the snapshot must read the state itself, not
        // depend on a pending re-sync.
        assert!(sp.take_key_state().is_some());
        let blob = sp.snapshot_bytes();

        let sp2 = SharedParams::new();
        assert!(sp2.take_key_state().is_none(), "fresh store is clean");
        sp2.restore_from_bytes(&blob).unwrap();

        // The restore must flag the key channel as well as the reload — they are
        // separate wires to the audio thread.
        let k = sp2.take_key_state().expect("restore must flag the key channel");
        assert_eq!(k.key_mode(), KeyMode::Split);
        assert_eq!(k.split_point, 43);
        assert!(k.lfo2_link);
        // And the non-consuming peek agrees with what the audio thread got.
        assert_eq!(sp2.key_state(), k);
    }

    #[test]
    fn matrix_edit_updates_the_right_layer_and_rides_the_ring() {
        let sp = SharedParams::new();
        // Edit Layer 2, slot 7: route LFO2 → Cutoff.
        sp.edit_matrix_slot(source_edit(Layer::L2, 7, SourceId::Lfo2));
        sp.edit_matrix_slot(dest_edit(Layer::L2, 7, DestId::Cutoff));
        // 0338: topology has its own channel now. Raising the param reload flag
        // here is what used to drag the audio thread through the matrix mutex
        // on every combo pick.
        assert!(!sp.take_reload(), "a topology edit must not flag a param reload");
        assert_eq!(sp.topology_backlog(), 2, "one record per edit, on the ring");
        assert!(!sp.topology_resync_pending());

        let m = sp.matrix_snapshot();
        assert_eq!(m[1].slots[7].source, SourceId::Lfo2);
        assert_eq!(m[1].slots[7].dest, DestId::Cutoff);
        // Layer 1's same slot is untouched.
        assert!(!m[0].slots[7].is_active());
    }

    // ── The topology channel (0338) ──────────────────────────────────────

    fn source_edit(layer: Layer, slot: u8, src: SourceId) -> MatrixEdit {
        MatrixEdit { layer, slot, field: MatrixField::Source, value: src as u8 }
    }

    fn dest_edit(layer: Layer, slot: u8, dest: DestId) -> MatrixEdit {
        MatrixEdit { layer, slot, field: MatrixField::Dest, value: dest as u8 }
    }

    /// An engine holding the store's patch with the channel already settled —
    /// the CLAP shell's `activate` followed by its first drain.
    fn activated(sp: &SharedParams) -> crate::engine::Engine {
        let mut e = crate::engine::Engine::new(48_000.0);
        sp.take_reload();
        e.load_state(sp.engine_state());
        sp.request_topology_resync();
        sp.drain_topology(&mut e);
        e
    }

    /// The other half of "no whole-patch re-sync": the drain writes the one
    /// field the record names and touches nothing else — not the other layer,
    /// not the other slots, and above all not the params.
    #[test]
    fn a_single_field_edit_applies_that_field_and_nothing_else() {
        let sp = SharedParams::new();
        let mut e = activated(&sp);

        // Drive one engine param away from the store. A whole-patch re-sync
        // would stomp it back; a field write must leave it alone.
        let cutoff_id = clap_id_of(Layer::L1, ParamId::Cutoff);
        e.set_param(cutoff_id, 1234.0);
        let before = e.matrices();

        sp.edit_matrix_slot(source_edit(Layer::L2, 7, SourceId::Lfo2));
        assert!(!sp.drain_topology(&mut e), "an edit is not a snapshot");

        let after = e.matrices();
        assert_eq!(after[1].slots[7].source, SourceId::Lfo2, "the edited field landed");
        assert_eq!(after[0], before[0], "the other layer must not move");
        for slot in 0..MATRIX_SLOTS {
            if slot != 7 {
                assert_eq!(after[1].slots[slot], before[1].slots[slot], "slot {slot}");
            }
        }
        assert_eq!(after[1].slots[7].dest, before[1].slots[7].dest, "dest untouched");
        assert_eq!(after[1].slots[7].depth, before[1].slots[7].depth, "depth untouched");
        assert_eq!(e.param(cutoff_id), 1234.0, "a field edit must not re-sync params");
    }

    /// Ring overflow is a defined path, not an argument that it cannot happen:
    /// the dropped record is subsumed by a full snapshot, and the audio thread
    /// converges on exactly the store's topology.
    #[test]
    fn a_full_ring_falls_back_to_the_snapshot_path() {
        let sp = SharedParams::new();
        let mut e = activated(&sp);

        // Nothing drains, so the ring fills exactly.
        for i in 0..TOPO_RING_SLOTS {
            let slot = (i % MATRIX_SLOTS) as u8;
            sp.edit_matrix_slot(source_edit(Layer::L1, slot, SourceId::Lfo2));
        }
        assert_eq!(sp.topology_backlog(), TOPO_RING_SLOTS, "the ring is full");
        assert!(!sp.topology_resync_pending(), "full is not yet overflowed");

        // One more has nowhere to go. It still lands on the store's table.
        sp.edit_matrix_slot(source_edit(Layer::L1, 5, SourceId::Aftertouch));
        assert!(sp.topology_resync_pending(), "an overflow owes a snapshot");
        assert_eq!(sp.topology_backlog(), TOPO_RING_SLOTS, "and nothing was queued");
        assert_eq!(sp.matrix_snapshot()[0].slots[5].source, SourceId::Aftertouch);

        // The audio thread drains what fits; the dropped record is still missing.
        assert!(!sp.drain_topology(&mut e), "no snapshot has been queued yet");
        assert_eq!(e.matrices()[0].slots[5].source, SourceId::Lfo2, "the tail was dropped");

        // Now there is room, so the producer's next service publishes the debt.
        sp.service_topology_resync();
        assert!(!sp.topology_resync_pending(), "the snapshot is queued");
        assert_eq!(sp.topology_backlog(), 1, "one snapshot, not 32 slot edits");
        assert!(sp.drain_topology(&mut e), "the drain must report the snapshot");

        // Converged: the engine's topology is the store's, dropped edit included.
        let (store, engine) = (sp.matrix_snapshot(), e.matrices());
        for layer in 0..2 {
            for slot in 0..MATRIX_SLOTS {
                let (a, b) = (store[layer].slots[slot], engine[layer].slots[slot]);
                assert_eq!(a.source, b.source, "layer {layer} slot {slot} source");
                assert_eq!(a.dest, b.dest, "layer {layer} slot {slot} dest");
                assert_eq!(a.enabled, b.enabled, "layer {layer} slot {slot} enabled");
            }
        }
    }

    /// While a resync is owed, individual edits are withheld rather than queued
    /// — the snapshot is taken *after* they hit the table, so it carries them.
    #[test]
    fn edits_made_while_a_resync_is_owed_ride_the_snapshot() {
        let sp = SharedParams::new();
        let mut e = activated(&sp);

        for _ in 0..(TOPO_RING_SLOTS + 1) {
            sp.edit_matrix_slot(source_edit(Layer::L1, 0, SourceId::Lfo2));
        }
        assert!(sp.topology_resync_pending());
        // Drain everything queued; the debt outlives it.
        sp.drain_topology(&mut e);
        assert!(sp.topology_resync_pending());

        // A fresh edit posted while still owing. Room exists now, so servicing
        // the debt publishes a snapshot that already includes it.
        sp.edit_matrix_slot(source_edit(Layer::L1, 11, SourceId::Aftertouch));
        assert!(!sp.topology_resync_pending());
        assert_eq!(sp.topology_backlog(), 1);
        assert!(sp.drain_topology(&mut e), "the record must be the snapshot");
        assert_eq!(
            e.matrices()[0].slots[11].source,
            SourceId::Aftertouch,
            "the edit made mid-debt must still reach the engine"
        );
    }

    /// Preset / state load is bulk: one snapshot record, never decomposed into
    /// per-field edits, and it still raises the param reload flag beside it.
    #[test]
    fn a_state_restore_crosses_as_one_snapshot_not_as_edits() {
        let sp = SharedParams::new();
        sp.edit_matrix_slot(source_edit(Layer::L1, 3, SourceId::Lfo2));
        sp.edit_matrix_slot(dest_edit(Layer::L1, 3, DestId::Cutoff));
        sp.set(clap_id_of(Layer::L1, ParamId::Cutoff), 987.0);
        let blob = sp.snapshot_bytes();

        let back = SharedParams::new();
        let mut e = activated(&back);
        back.restore_from_bytes(&blob).unwrap();
        assert_eq!(back.topology_backlog(), 1, "one snapshot for the whole load");
        assert!(back.take_reload(), "a load still re-syncs the params");

        assert!(back.drain_topology(&mut e), "the record must be a snapshot");
        assert_eq!(e.matrices()[0].slots[3].source, SourceId::Lfo2);
        assert_eq!(e.matrices()[0].slots[3].dest, DestId::Cutoff);
    }

    /// A snapshot leaves depth alone (it is param-authoritative, 0205) and the
    /// re-sync it implies re-seeds it — so the pair converge in one block.
    #[test]
    fn a_snapshot_leaves_depth_to_the_params() {
        let sp = SharedParams::new();
        let mut e = activated(&sp);

        sp.set(clap_id_of(Layer::L1, ParamId::MatrixSlot4Depth), -0.75);
        sp.request_topology_resync();
        assert!(sp.drain_topology(&mut e));
        // The snapshot alone must not have written a depth...
        assert_ne!(e.matrices()[0].slots[4].depth, -0.75);
        // ...but the re-sync it implies does, from the params, in the same block.
        let matrices = e.matrices();
        e.load_state(sp.state_with(matrices));
        assert_eq!(e.matrices()[0].slots[4].depth, -0.75);
    }

    /// `activate` adopts the store directly, so anything already queued predates
    /// that adoption. It is superseded rather than dropped — a snapshot pushed
    /// *behind* the stale records, so the producer never has to reach across the
    /// channel for the consumer's cursor.
    #[test]
    fn activate_supersedes_records_older_than_the_state_it_adopts() {
        let sp = SharedParams::new();
        // A stale edit posted before the table change that supersedes it.
        sp.edit_matrix_slot(source_edit(Layer::L1, 6, SourceId::Lfo2));
        {
            let mut m = sp.lock();
            m[0].slots[6].source = SourceId::Aftertouch;
        }

        let mut e = activated(&sp);
        assert_eq!(sp.topology_backlog(), 0, "activate's snapshot drained with it");
        assert!(!sp.drain_topology(&mut e));
        assert_eq!(
            e.matrices()[0].slots[6].source,
            SourceId::Aftertouch,
            "the stale edit was replayed but the snapshot behind it won"
        );
    }

    /// The reverse-order half of the bulk hand-off. The producer pushes the
    /// snapshot and *then* raises `reload`; a consumer that reads the flag
    /// between its own drain and the check would otherwise install the new
    /// params over the old topology for a block. Seeing `reload` set means the
    /// snapshot is already queued, so one more drain always finds it.
    #[test]
    fn a_reload_seen_after_the_drain_still_finds_its_snapshot() {
        let sp = SharedParams::new();
        let mut e = activated(&sp);

        // Block N: the shell drains an empty channel...
        assert!(!sp.drain_topology(&mut e));
        // ...and the load lands in the window before it reads the flag.
        let other = SharedParams::new();
        other.edit_matrix_slot(source_edit(Layer::L1, 12, SourceId::Aftertouch));
        sp.restore_from_bytes(&other.snapshot_bytes()).unwrap();

        assert!(sp.take_reload());
        assert!(
            sp.drain_topology(&mut e),
            "the second drain must pick up the snapshot the flag implies"
        );
        assert_eq!(e.matrices()[0].slots[12].source, SourceId::Aftertouch);
    }

    /// The latency criterion: an edit posted after block N is in block N+1's
    /// render, exactly as the mutex path managed. The control writes the same
    /// fields straight onto the engine at the same instant — a zero-latency
    /// transport by construction — and the two renders must be bit-identical.
    #[test]
    fn an_edit_posted_between_blocks_is_audible_in_the_next_one() {
        const FRAMES: usize = 128;

        // Slot 9: LFO 2 → Cutoff, switched on, at full depth. The depth is a
        // param, so it is in place from the start; only the topology is late.
        let edits = [
            source_edit(Layer::L1, 9, SourceId::Lfo2),
            dest_edit(Layer::L1, 9, DestId::Cutoff),
            MatrixEdit { layer: Layer::L1, slot: 9, field: MatrixField::Enabled, value: 1 },
        ];
        let seeded = || {
            let sp = SharedParams::new();
            sp.set(clap_id_of(Layer::L1, ParamId::MatrixSlot9Depth), 1.0);
            sp
        };
        let block = |e: &mut crate::engine::Engine| {
            let (mut l, mut r) = (vec![0.0f32; FRAMES], vec![0.0f32; FRAMES]);
            e.process_block(&mut l, &mut r);
            l
        };

        // (a) Posted between block 0 and block 1, drained at the top of block 1
        // — the CLAP shell's order.
        let sp = seeded();
        let mut e_ring = activated(&sp);
        e_ring.note_on(0, 60, 1.0);
        let _ = block(&mut e_ring);
        for e in edits {
            sp.edit_matrix_slot(e);
        }
        assert!(!sp.drain_topology(&mut e_ring), "edits, not a snapshot");
        let via_ring = block(&mut e_ring);

        // (b) The same three field writes, applied directly at the same instant.
        let control = seeded();
        let mut e_direct = activated(&control);
        e_direct.note_on(0, 60, 1.0);
        let _ = block(&mut e_direct);
        for e in edits {
            crate::topology::apply_edit(e_direct.matrix_mut(e.layer), e);
        }
        let via_direct = block(&mut e_direct);

        // (c) And the same block with the route never wired, so (a) == (b) is
        // not two identical renders of nothing.
        let plain = seeded();
        let mut e_plain = activated(&plain);
        e_plain.note_on(0, 60, 1.0);
        let _ = block(&mut e_plain);
        let unwired = block(&mut e_plain);

        assert_eq!(
            via_ring, via_direct,
            "the ring must not defer the edit by a block"
        );
        assert_ne!(via_ring, unwired, "the wired route must be audible in that block");
    }

    #[test]
    fn empty_blob_restore_is_an_error_and_leaves_store() {
        let sp = SharedParams::new();
        sp.set(ParamId::Cutoff as usize, 777.0);
        assert!(sp.restore_from_bytes(&[]).is_err());
        assert_eq!(sp.get(ParamId::Cutoff as usize), 777.0, "store untouched on error");
        assert!(!sp.take_reload(), "a failed restore does not flag reload");
    }

    // ── Copy Layer 1 → Layer 2 ───────────────────────────────────────

    /// Seed layer 1 with a recognisable patch and a route the factory lacks.
    fn seeded_for_copy() -> SharedParams {
        let sp = SharedParams::new();
        for (p, v) in [
            (ParamId::Cutoff, 3210.0),
            (ParamId::Resonance, 0.77),
            (ParamId::Osc1Level, 0.42),
            (ParamId::StackWidth, 3.0),
        ] {
            sp.set(patch_clap_id(Layer::L1, p).unwrap(), v);
        }
        sp.edit_matrix_slot(MatrixEdit {
            layer: Layer::L1,
            slot: 9,
            field: MatrixField::Source,
            value: SourceId::Aftertouch as u8,
        });
        sp.edit_matrix_slot(MatrixEdit {
            layer: Layer::L1,
            slot: 9,
            field: MatrixField::Dest,
            value: DestId::HpfCutoff as u8,
        });
        sp.take_reload();
        sp
    }

    #[test]
    fn copy_layer_duplicates_every_patch_param_but_the_mixer_strip() {
        let sp = seeded_for_copy();
        // Give layer 2 a mixer strip the copy must not touch.
        for (p, v) in [
            (ParamId::LayerLevel, 0.25),
            (ParamId::LayerMute, 1.0),
            (ParamId::LayerPan, -0.5),
        ] {
            sp.set(patch_clap_id(Layer::L2, p).unwrap(), v);
        }
        sp.copy_layer(Layer::L1, Layer::L2);

        for &inner in PATCH_PARAMS.iter() {
            let (a, b) = (
                sp.get(patch_clap_id(Layer::L1, inner).unwrap()),
                sp.get(patch_clap_id(Layer::L2, inner).unwrap()),
            );
            if COPY_LAYER_EXCLUDED.contains(&inner) {
                continue;
            }
            assert_eq!(a, b, "{inner:?} did not copy");
        }
        assert_eq!(sp.get(patch_clap_id(Layer::L2, ParamId::LayerLevel).unwrap()), 0.25);
        assert_eq!(sp.get(patch_clap_id(Layer::L2, ParamId::LayerMute).unwrap()), 1.0);
        assert_eq!(sp.get(patch_clap_id(Layer::L2, ParamId::LayerPan).unwrap()), -0.5);
        assert!(sp.take_reload(), "a copy must flag a reload");
    }

    #[test]
    fn copy_layer_duplicates_the_matrix_topology() {
        let sp = seeded_for_copy();
        sp.copy_layer(Layer::L1, Layer::L2);
        let m = sp.matrix_snapshot();
        for slot in 0..MATRIX_SLOTS {
            assert_eq!(m[0].slots[slot].source, m[1].slots[slot].source, "slot {slot} source");
            assert_eq!(m[0].slots[slot].dest, m[1].slots[slot].dest, "slot {slot} dest");
            assert_eq!(
                m[0].slots[slot].polarity, m[1].slots[slot].polarity,
                "slot {slot} polarity"
            );
            assert_eq!(m[0].slots[slot].shape, m[1].slots[slot].shape, "slot {slot} shape");
            assert_eq!(
                m[0].slots[slot].enabled, m[1].slots[slot].enabled,
                "slot {slot} enabled"
            );
            assert_eq!(
                m[0].slots[slot].scale_shape, m[1].slots[slot].scale_shape,
                "slot {slot} scale shape"
            );
            assert_eq!(
                m[0].slots[slot].scale_src, m[1].slots[slot].scale_src,
                "slot {slot} scale"
            );
        }
        assert_eq!(m[1].slots[9].dest, DestId::HpfCutoff, "the seeded route came across");
    }

    /// The null-doubling guard. `lane_phase` is a fixed function of lane index
    /// with no seed and both allocators pick the same lane for the same note, so
    /// an exact copy would render bit-identical layers — +6 dB and no width.
    #[test]
    fn copy_layer_offsets_the_copys_detune_only() {
        let sp = seeded_for_copy();
        sp.set(patch_clap_id(Layer::L1, ParamId::LayerDetune).unwrap(), 0.0);
        sp.copy_layer(Layer::L1, Layer::L2);
        assert_eq!(
            sp.get(patch_clap_id(Layer::L2, ParamId::LayerDetune).unwrap()),
            COPY_DETUNE_CENTS
        );
        assert_eq!(
            sp.get(patch_clap_id(Layer::L1, ParamId::LayerDetune).unwrap()),
            0.0,
            "the source layer's detune must be left alone"
        );
    }

    #[test]
    fn copy_layer_turns_on_layer_2_but_leaves_an_existing_split() {
        let sp = SharedParams::new();
        assert!(!sp.key_state().layer2_on, "single mode to start");
        sp.copy_layer(Layer::L1, Layer::L2);
        let k = sp.key_state();
        assert!(k.layer2_on, "copying from single must land in dual");
        assert!(!k.split_enabled);

        // Already split → the routing is the player's choice, leave it.
        let sp = SharedParams::new();
        sp.apply_key_op(KeyOp::SetKeyMode(2));
        sp.copy_layer(Layer::L1, Layer::L2);
        let k = sp.key_state();
        assert!(k.layer2_on && k.split_enabled, "an existing split must survive");
    }

    #[test]
    fn reset_layer_returns_every_patch_param_to_its_default() {
        let sp = seeded_for_copy();
        let factory = LayerState::factory_default();
        sp.reset_layer(Layer::L1);
        for &inner in PATCH_PARAMS.iter() {
            let id = patch_clap_id(Layer::L1, inner).unwrap();
            assert_eq!(sp.get(id), factory.params.get(inner), "{inner:?} did not reset");
        }
    }

    /// The deliberate divergence from `copy_layer`: copy spares the mixer
    /// strip, reset does not. A blank layer that is still muted and panned hard
    /// left reads as broken rather than blank.
    #[test]
    fn reset_layer_also_resets_the_mixer_strip() {
        let sp = SharedParams::new();
        for (p, v) in [
            (ParamId::LayerLevel, 0.25),
            (ParamId::LayerMute, 1.0),
            (ParamId::LayerPan, -0.5),
            (ParamId::LayerDetune, 7.0),
        ] {
            sp.set(patch_clap_id(Layer::L1, p).unwrap(), v);
        }
        sp.reset_layer(Layer::L1);
        for p in COPY_LAYER_EXCLUDED {
            let id = patch_clap_id(Layer::L1, p).unwrap();
            assert_eq!(sp.get(id), p.desc().default, "{p:?} is spared by copy, not by reset");
        }
    }

    #[test]
    fn reset_layer_restores_the_default_matrix_topology() {
        let sp = seeded_for_copy();
        sp.reset_layer(Layer::L1);
        assert_eq!(sp.matrix_snapshot()[Layer::L1 as usize], crate::matrix::default_patch());
    }

    #[test]
    fn reset_layer_leaves_the_other_layer_alone() {
        let sp = seeded_for_copy();
        sp.copy_layer(Layer::L1, Layer::L2);
        let before: Vec<f32> = PATCH_PARAMS
            .iter()
            .map(|&i| sp.get(patch_clap_id(Layer::L2, i).unwrap()))
            .collect();
        let matrix_before = sp.matrix_snapshot()[Layer::L2 as usize];
        sp.reset_layer(Layer::L1);
        let after: Vec<f32> = PATCH_PARAMS
            .iter()
            .map(|&i| sp.get(patch_clap_id(Layer::L2, i).unwrap()))
            .collect();
        assert_eq!(before, after, "resetting L1 moved L2");
        assert_eq!(matrix_before, sp.matrix_snapshot()[Layer::L2 as usize]);
    }

    /// Layer enable and the split describe how the layers share the keyboard,
    /// not either layer's patch. Copy turns Layer 2 on (a copy to a silent layer
    /// is pointless); reset has no such reason and must not touch it.
    #[test]
    fn reset_layer_leaves_key_state_alone() {
        let sp = SharedParams::new();
        sp.apply_key_op(KeyOp::SetKeyMode(2));
        sp.apply_key_op(KeyOp::SetSplitPoint(48));
        let before = sp.key_state();
        sp.reset_layer(Layer::L2);
        let after = sp.key_state();
        assert_eq!(before.layer2_on, after.layer2_on);
        assert_eq!(before.split_enabled, after.split_enabled);
        assert_eq!(before.split_point, after.split_point);
    }

    #[test]
    fn reset_layer_flags_a_reload() {
        let sp = seeded_for_copy();
        sp.reset_layer(Layer::L1);
        assert!(sp.take_reload(), "the audio thread must re-sync after a reset");
    }

    #[test]
    fn reset_layer_raises_no_gesture_flags() {
        let sp = seeded_for_copy();
        sp.reset_layer(Layer::L1);
        assert!(
            (0..TOTAL_PARAMS).all(|id| !sp.gesture(id)),
            "gestures suppress host echo mid-drag; a bulk write must not raise them"
        );
    }

    #[test]
    fn copy_layer_raises_no_gesture_flags() {
        let sp = seeded_for_copy();
        sp.copy_layer(Layer::L1, Layer::L2);
        assert!(
            (0..TOTAL_PARAMS).all(|id| !sp.gesture(id)),
            "gestures suppress host echo mid-drag; a bulk write must not raise them"
        );
    }

    #[test]
    fn copy_layer_onto_itself_is_a_no_op() {
        let sp = seeded_for_copy();
        let before = sp.get(patch_clap_id(Layer::L1, ParamId::LayerDetune).unwrap());
        sp.copy_layer(Layer::L1, Layer::L1);
        assert_eq!(sp.get(patch_clap_id(Layer::L1, ParamId::LayerDetune).unwrap()), before);
        assert!(!sp.take_reload(), "a self-copy must not flag a reload");
    }

    /// The null-doubling regression, end to end. The param assertion above says
    /// the detune landed; this says it *matters*. Drive a real engine from the
    /// copied state and compare each layer's contribution: with `MasterDrift` at
    /// 0 and no detune the two would render bit-identically (`lane_phase` is a
    /// fixed function of lane index with no seed, and both allocators pick the
    /// same lane for the same note), giving +6 dB and no width at all.
    #[test]
    fn a_copied_pair_does_not_null_double() {
        use crate::engine::Engine;

        // One layer's contribution, with the other muted.
        let render = |sp: &SharedParams, mute: Layer| {
            let sp2 = SharedParams::new();
            sp2.restore_from_bytes(&sp.snapshot_bytes()).expect("round-trip");
            sp2.set(patch_clap_id(mute, ParamId::LayerMute).unwrap(), 1.0);
            let mut e = Engine::new(48_000.0);
            e.load_state(sp2.engine_state());
            e.set_key_state(sp2.key_state());
            e.note_on(0, 60, 1.0);
            let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
            for _ in 0..4 {
                e.process_block(&mut l, &mut r);
            }
            l
        };

        let sp = SharedParams::new();
        sp.set(global_clap_id(ParamId::MasterDrift).unwrap(), 0.0);
        sp.copy_layer(Layer::L1, Layer::L2);

        let a = render(&sp, Layer::L2);
        let b = render(&sp, Layer::L1);
        assert!(a.iter().any(|&s| s != 0.0), "layer 1 must sound");
        assert!(b.iter().any(|&s| s != 0.0), "layer 2 must sound");
        let diff = a.iter().zip(&b).fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
        assert!(diff > 1e-6, "the copied pair renders identically — it will null-double");
    }

    /// A copy must survive a host save/reload with both layers intact — params
    /// *and* the duplicated topology.
    #[test]
    fn clap_state_round_trips_after_a_copy() {
        let sp = seeded_for_copy();
        sp.copy_layer(Layer::L1, Layer::L2);
        let blob = sp.snapshot_bytes();

        let back = SharedParams::new();
        back.restore_from_bytes(&blob).expect("round-trip");
        for &inner in PATCH_PARAMS.iter() {
            for layer in [Layer::L1, Layer::L2] {
                let id = patch_clap_id(layer, inner).unwrap();
                assert_eq!(back.get(id), sp.get(id), "{layer:?} {inner:?} drifted");
            }
        }
        let (a, b) = (sp.matrix_snapshot(), back.matrix_snapshot());
        for layer in 0..2 {
            for slot in 0..MATRIX_SLOTS {
                assert_eq!(a[layer].slots[slot].source, b[layer].slots[slot].source);
                assert_eq!(a[layer].slots[slot].dest, b[layer].slots[slot].dest);
            }
        }
        assert_eq!(
            back.get(patch_clap_id(Layer::L2, ParamId::LayerDetune).unwrap()),
            COPY_DETUNE_CENTS,
            "the copy's detune offset must survive the round-trip"
        );
    }
}
