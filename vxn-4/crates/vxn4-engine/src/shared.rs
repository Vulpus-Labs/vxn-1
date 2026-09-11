//! The authoritative patch, on the main thread (ticket 0382).
//!
//! **The rule this module exists to establish: the main thread owns the model;
//! the audio thread reads it and never owns it. Truth flows main → audio and
//! never back.** Everything below is in service of that one sentence, and the
//! rest of E052 — the preset codec, the state blob, the faceplate — is built on
//! it holding.
//!
//! Until this landed, [`Engine`] *was* the patch: it held a
//! [`Patch`](crate::patch::Patch) by value and `set_patch` swapped the whole
//! thing in from the baked bank. That is fine while the only writer is a
//! patch-index parameter and fatal once an editor is writing individual fields
//! at pointer rate — a knob drag would be a whole-patch rebuild per pointer
//! move, and there would be nowhere for a main-thread reader (`state.save`, the
//! faceplate's echo) to read a value from except the audio thread's own copy.
//!
//! ## A port, not a design
//!
//! vxn-1b solved this in ticket 0338 and the answer is in the tree, so this is
//! a port of `vxn1b_engine::shared` with vxn-4's table substituted for
//! vxn-1b's. The split:
//!
//! - **Values** — one `AtomicU32` (f32 bits) per descriptor id from
//!   [`crate::params`], all [`N_PARAMS`] of them. That is nearly the whole
//!   patch: every operator field and envelope segment, all 64 authored PM
//!   depths, the eight sum-bus sends, the 48 matrix slot depths and the patch
//!   trim, plus the eleven host params so the faceplate can bind them by name
//!   like everything else. Writes cross without locks and are latest-wins, so a
//!   knob drag coalesces into one re-sync per block for free.
//! - **Topology** — the matrix slots' endpoints, curves and on/off switches:
//!   the one part of a patch that is not a scalar. It lives behind a `Mutex`
//!   **the audio thread never takes**, and reaches the engine exclusively over
//!   the lock-free [`crate::topology`] ring.
//! - **`reload`** — raised by any write to a *patch* value and by every bulk
//!   op. The audio thread swaps it to `false` at the top of `process` and
//!   re-reads the values. Topology edits deliberately do not raise it; they
//!   ride the ring.
//!
//! ## The mutex is main-thread-only *by construction*
//!
//! This is the load-bearing invariant, and it is structural rather than a
//! convention someone has to remember. [`SharedParams::lock`] is private.
//! Nothing reachable from `process` calls it: [`SharedParams::sync`] and
//! [`SharedParams::drain_topology`] touch only atomics and the ring, and
//! [`Engine::matrix_mut`] — the only way to write the audio thread's table — is
//! `pub(crate)` and called from one place, the drain. The failure this buys
//! away is a priority inversion, whose symptom is a dropout under load and
//! whose test result on an idle machine is a pass, so it has to be
//! unrepresentable rather than merely untriggered.
//! `the_audio_thread_drains_while_the_editor_holds_the_lock` asserts it
//! dynamically as well, because a structural argument that no test can fail is
//! an argument nobody re-checks.
//!
//! ## What the two channels guarantee together
//!
//! Draining a `Snapshot` **implies** a param re-sync ([`SharedParams::sync`]),
//! and the producer queues the snapshot *before* raising `reload`. Between
//! them, those two facts make the producer's two stores order-independent: a
//! consumer that sees the snapshot has the values behind it, and a consumer
//! that sees the flag has the snapshot in front of it and drains once more
//! rather than rendering a block of new values over old topology. Preset load
//! is therefore coherent whichever order the stores land in, which is the
//! property `params_and_topology_converge_in_either_order` pins.
//!
//! What is *not* claimed, because a lock-free latest-wins value channel cannot
//! claim it without spinning: a bulk install that races a re-sync already in
//! flight can be observed with some values old for one block. Every value is a
//! single atomic, so nothing tears and nothing lands out of range; the block
//! after is exact. Making that window vanish would need a seqlock, and a
//! seqlock means the audio thread retries — which is spinning, which is the
//! thing this module exists to avoid.
//!
//! ## Host params are in the table but not in the sync
//!
//! [`SharedParams::write_tables`] copies the **patch region** only. Patch
//! index, quality, master gain and the eight macro positions are performance
//! and project state rather than patch state (see [`crate::params`]), and they
//! already reach the engine by the CLAP parameter path through `set_patch`,
//! `set_quality`, `set_master_gain` and `set_macro`. They live here so a
//! faceplate and a state blob have one place to read every named value from;
//! joining the two paths up is 0386's job, not this ticket's.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::engine::Engine;
use crate::matrix::Matrix;
use crate::params::{self, EgField, N_PARAMS, OpField, Param, ParamId, is_patch_field, patch_ids};
use crate::patch::{PatchTables, patch};
use crate::topology::{SlotEdit, TOPO_RING_SLOTS, TopoMsg, TopologyRing};

/// What one drain of the topology ring did.
///
/// Two bits rather than one because the two arms owe different work. Any record
/// leaves the engine's derived tables stale and needs a rebuild; only a
/// `Snapshot` additionally implies the values changed, since nothing but a bulk
/// op produces one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Drain {
    /// At least one record was applied.
    pub applied: bool,
    /// A [`TopoMsg::Snapshot`] was among them.
    pub snapshot: bool,
}

impl Drain {
    fn merge(self, other: Self) -> Self {
        Self {
            applied: self.applied || other.applied,
            snapshot: self.snapshot || other.snapshot,
        }
    }
}

/// The authoritative patch: lock-free values, a main-thread topology table, and
/// the channels that carry both to the audio thread.
///
/// Seeded to factory entry 0, which is what [`Engine::new`] installs, so a
/// fresh store and a fresh engine agree before anything has been synced.
pub struct SharedParams {
    /// One cell per descriptor id, f32 bits. Relaxed on both ends: each value
    /// is independent and latest-wins, and the ordering that matters between
    /// *channels* is carried by `reload` and the ring's cursors.
    values: Vec<AtomicU32>,
    /// The store's authoritative matrix topology.
    ///
    /// **Main thread only.** This is what `state.save`, the preset codec and
    /// the editor's echo read; the audio thread's copy lives in the engine and
    /// is fed by `topo`. See the module docs for why that is structural.
    matrix: Mutex<Matrix>,
    /// Topology deltas and snapshots on their way to the audio thread.
    /// Lock-free at both ends; see [`crate::topology`].
    topo: TopologyRing,
    /// Raised when the **values** need re-reading. Cleared by the audio thread.
    reload: AtomicBool,
}

impl Default for SharedParams {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedParams {
    /// A store holding factory entry 0.
    pub fn new() -> Self {
        let store = Self {
            values: (0..N_PARAMS)
                .map(|_| AtomicU32::new(0.0f32.to_bits()))
                .collect(),
            matrix: Mutex::new(Matrix::default()),
            topo: TopologyRing::new(),
            reload: AtomicBool::new(false),
        };
        // Host params take their descriptor defaults; the patch region takes the
        // factory entry. Not `install` for either half: a patch does not imply a
        // master gain or a macro position, and a fresh store must owe **no**
        // signal — `Engine::new` installs the same entry, so there is nothing to
        // tell it, and a store that raised `reload` at construction would make
        // the first `sync` of every session a pointless full rebuild while
        // hiding a genuinely missing signal behind one that is always there.
        for id in params::all_ids() {
            if let Some(d) = params::desc(id) {
                store.store_raw(id, d.default);
            }
        }
        let factory: PatchTables = patch(0).into();
        for id in patch_ids() {
            store.store_raw(id, read_field(&factory, id));
        }
        *store.lock() = factory.matrix;
        store
    }

    /// The authoritative topology table. **Main thread only** — nothing
    /// reachable from `process` may call this, which is why it is private.
    #[inline]
    fn lock(&self) -> std::sync::MutexGuard<'_, Matrix> {
        // Recover a poisoned lock rather than propagating a panic: the guarded
        // table is a plain value that is still readable after any mid-write
        // panic (plugin code unwinds), and a synth that refuses to make a sound
        // because a UI thread panicked once is worse than one carrying a
        // half-applied route.
        self.matrix.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ── Values ──────────────────────────────────────────────────────────────

    /// Read a value. `0.0` past the table.
    #[inline]
    pub fn get(&self, id: ParamId) -> f32 {
        self.values
            .get(id.raw())
            .map_or(0.0, |a| f32::from_bits(a.load(Ordering::Relaxed)))
    }

    /// Write a value, clamped to the descriptor range. No-op past the table.
    ///
    /// Raises [`Self::take_reload`] for a **patch** field, which is how an edit
    /// reaches the audio thread at all — there is no other channel for a
    /// scalar. Coalescing is the point: a hundred writes during one buffer
    /// period cost the audio thread one re-sync, and the value it reads is the
    /// last one written rather than a hundred applied in order.
    ///
    /// A host field does not raise it. Those reach the engine by the CLAP
    /// parameter path; see the module docs.
    #[inline]
    pub fn set(&self, id: ParamId, value: f32) {
        let Some(desc) = params::desc(id) else {
            return;
        };
        self.values[id.raw()].store(desc.clamp(value).to_bits(), Ordering::Relaxed);
        if is_patch_field(id) {
            self.reload.store(true, Ordering::Release);
        }
    }

    /// Store without clamping or signalling. Bulk paths only — they raise
    /// `reload` once at the end, after the topology snapshot is queued, which
    /// is what keeps the two channels in order.
    #[inline]
    fn store_raw(&self, id: ParamId, value: f32) {
        if let Some(cell) = self.values.get(id.raw()) {
            cell.store(value.to_bits(), Ordering::Relaxed);
        }
    }

    /// Read a value as a **fader position** in `[0, 1]`.
    ///
    /// `to_fader`, not `to_normalized`: the descriptor's taper is part of the
    /// calibration rather than a display flourish. `op-N-damp-hz` spans 20 Hz to
    /// 1 MHz, so a linear position would spend the top three quarters of the
    /// travel above audibility and crush the whole usable range into the first
    /// centimetre. Only the editor path goes through here — presets and the
    /// state blob are plain values — so the taper never reaches the wire.
    #[inline]
    pub fn get_normalized(&self, id: ParamId) -> f32 {
        params::desc(id).map_or(0.0, |d| d.to_fader(self.get(id)))
    }

    /// Write a value from a fader position. Inverse of
    /// [`Self::get_normalized`], taper included, so a drag and the echo that
    /// answers it agree.
    #[inline]
    pub fn set_normalized(&self, id: ParamId, norm: f32) {
        if let Some(d) = params::desc(id) {
            self.set(id, d.from_fader(norm));
        }
    }

    /// Whether the audio thread should re-read the values; clears the flag.
    /// Call once per block, after the topology drain — see [`Self::sync`].
    #[inline]
    pub fn take_reload(&self) -> bool {
        self.reload.swap(false, Ordering::Acquire)
    }

    // ── Topology ────────────────────────────────────────────────────────────

    /// A copy of the authoritative topology. **Main thread** — `state.save`,
    /// the preset codec, the editor's echo.
    pub fn matrix_snapshot(&self) -> Matrix {
        *self.lock()
    }

    /// Apply one topology edit and post it to the audio thread. Out-of-range
    /// slot indices are ignored.
    ///
    /// **This is the path that would otherwise make the render take a lock.**
    /// It raises no reload flag and triggers no whole-patch re-read: the record
    /// crosses on the ring and the audio thread applies exactly the one field
    /// it names, on the next block. Depth is not a topology field — it is a
    /// descriptor param and rides [`Self::set`].
    pub fn edit_slot(&self, edit: SlotEdit) {
        crate::topology::apply_edit(&mut self.lock(), edit);
        self.publish(TopoMsg::Edit(edit));
    }

    /// Post one record, or fall back to the snapshot path.
    ///
    /// A resync already owed supersedes an individual edit — the snapshot it
    /// will carry is taken from the table the edit has just been applied to —
    /// so in that state the record is deliberately withheld rather than queued.
    fn publish(&self, msg: TopoMsg) {
        if self.topo.resync_pending() || !self.topo.try_push(msg) {
            self.topo.request_resync();
        }
        self.service_topology_resync();
    }

    /// Declare that the whole topology changed and the audio thread must adopt
    /// the store's table wholesale. The bulk path and the ring's overflow
    /// backstop are the same code, which is the point of the snapshot arm.
    pub fn request_topology_resync(&self) {
        self.topo.request_resync();
        self.service_topology_resync();
    }

    /// Publish the owed snapshot if there is one and the ring has room.
    ///
    /// **Main thread**; idempotent, and a no-op in the overwhelmingly common
    /// case where nothing is owed. A shell should also call it from its editor
    /// tick, so a snapshot deferred by a full ring cannot sit unsent while the
    /// user waits for a preset to take effect.
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

    // ── Bulk ────────────────────────────────────────────────────────────────

    /// Install a whole patch: every value, the topology, and both signals.
    ///
    /// Order is load-bearing. Values first, then the topology snapshot, then
    /// `reload` — so an audio thread that observes the flag already has the
    /// snapshot in front of it, and one that observes the snapshot already has
    /// the values behind it. Reverse any pair and a preset load can be seen
    /// half applied.
    pub fn install(&self, tables: &PatchTables) {
        for id in patch_ids() {
            self.store_raw(id, read_field(tables, id));
        }
        *self.lock() = tables.matrix;
        self.request_topology_resync();
        self.reload.store(true, Ordering::Release);
    }

    /// [`Self::install`] of a factory entry.
    ///
    /// Note what this does **not** do: panic the voices. That is the whole
    /// point of the inversion — a patch arriving as values and one snapshot is
    /// a large edit, not a topology the sounding voices are about to lose.
    /// `Engine::set_patch` still silences them, because it is the CLAP
    /// parameter path and a host stepping that parameter means something
    /// different by it.
    pub fn load_factory(&self, index: usize) {
        self.install(&patch(index).into());
    }

    // ── The audio thread's side ─────────────────────────────────────────────

    /// Copy the **patch region** of the store into `tables`, leaving the matrix
    /// slots' topology alone. **Audio thread**, lock-free and allocation-free.
    ///
    /// Depths are written; endpoints, curves and switches are not, because they
    /// arrive on the ring and whatever is already in `tables` is newer than
    /// anything this could reconstruct.
    pub fn write_tables(&self, tables: &mut PatchTables) {
        for id in patch_ids() {
            write_field(tables, id, self.get(id));
        }
    }

    /// Drain the topology channel onto `engine`. **Audio thread**, once at the
    /// top of `process`, before the reload check.
    ///
    /// Wait-free: a bounded number of pops, each a plain copy out of a
    /// pre-allocated cell, and a field write on the engine's own table. No
    /// lock, no allocation, and no whole-patch rebuild for a single-field edit.
    ///
    /// Leaves the engine's derived tables stale — the caller rebuilds once, at
    /// the end, rather than 32 times. [`Self::sync`] is that caller.
    pub fn drain_topology(&self, engine: &mut Engine) -> Drain {
        let mut d = Drain::default();
        while let Some(msg) = self.topo.pop() {
            d.applied = true;
            match msg {
                TopoMsg::Edit(edit) => crate::topology::apply_edit(engine.matrix_mut(), edit),
                TopoMsg::Snapshot(table) => {
                    d.snapshot = true;
                    crate::topology::apply_snapshot(engine.matrix_mut(), &table);
                }
            }
        }
        d
    }

    /// Bring `engine` up to date with the store. **Audio thread**, once at the
    /// top of `process`. Returns whether anything changed.
    ///
    /// Takes no lock, spins on nothing and allocates nothing, on any path.
    ///
    /// The double drain is the ordering fix. The producer queues its snapshot
    /// before raising `reload`, so a flag seen *after* a drain that found
    /// nothing means the snapshot landed in the window between the two — and
    /// one more pop finds it. Without it, that block would render the new
    /// values against the old topology, which for a preset load is an audible
    /// wrong sound for one buffer rather than a clean switch. It costs one
    /// atomic load in the common case, because `reload` is false.
    pub fn sync(&self, engine: &mut Engine) -> bool {
        let mut d = self.drain_topology(engine);
        let reload = self.take_reload();
        if reload && !d.snapshot {
            d = d.merge(self.drain_topology(engine));
        }
        if reload || d.snapshot {
            // A snapshot is only ever produced by a bulk op, which also rewrote
            // the values — so it implies the re-read whether or not the flag
            // was observed with it. That is what makes the producer's two
            // stores order-independent.
            engine.adopt_params(self);
            true
        } else if d.applied {
            // A field edit and nothing else: the values are already correct, so
            // rebuilding the derived tables is the whole job.
            engine.resync();
            true
        } else {
            false
        }
    }
}

// ── The patch region, field by field ────────────────────────────────────────
//
// The two directions are written out rather than derived from a shared
// accessor, because `op-N-wave` is a `Waveform` on one side and an f32 on the
// other and there is no reference that is both. They are mirror images, so the
// hazard is one of them drifting — `every_patch_field_round_trips` walks all
// 205 ids through both and is the guard against it.

/// Read the value a descriptor id names out of a patch.
fn read_field(t: &PatchTables, id: ParamId) -> f32 {
    match params::decode(id).expect("a patch id decodes") {
        Param::Op { op, field } => {
            let o = &t.ops[op];
            match field {
                OpField::Wave => o.wave.index() as f32,
                OpField::Ratio => o.ratio,
                OpField::Level => o.level,
                OpField::Pan => o.pan,
                OpField::DampHz => o.damp_hz,
                OpField::Phase => o.phase,
                OpField::PhaseSpread => o.phase_spread,
            }
        }
        Param::OpEg { op, field } => eg_slot(field, &t.eg[op].t, &t.eg[op].l),
        Param::Pm { dest, src } => t.routing.pm[dest][src],
        Param::Out { op } => t.routing.out[op],
        Param::MatrixDepth { slot } => t.matrix.slots[slot].depth,
        Param::PatchGain => t.gain,
        Param::Host(h) => unreachable!("{h:?} is not a patch field"),
    }
}

/// Write the value a descriptor id names into a patch.
fn write_field(t: &mut PatchTables, id: ParamId, v: f32) {
    match params::decode(id).expect("a patch id decodes") {
        Param::Op { op, field } => {
            let o = &mut t.ops[op];
            match field {
                OpField::Wave => o.wave = params::waveform_from(v),
                OpField::Ratio => o.ratio = v,
                OpField::Level => o.level = v,
                OpField::Pan => o.pan = v,
                OpField::DampHz => o.damp_hz = v,
                OpField::Phase => o.phase = v,
                OpField::PhaseSpread => o.phase_spread = v,
            }
        }
        Param::OpEg { op, field } => {
            let eg = &mut t.eg[op];
            let i = field as usize;
            if i < eg.t.len() {
                eg.t[i] = v;
            } else {
                eg.l[i - eg.t.len()] = v;
            }
        }
        Param::Pm { dest, src } => t.routing.pm[dest][src] = v,
        Param::Out { op } => t.routing.out[op] = v,
        // Depth only. The rest of the slot is topology and arrives on the ring.
        Param::MatrixDepth { slot } => t.matrix.slots[slot].depth = v,
        Param::PatchGain => t.gain = v,
        Param::Host(h) => unreachable!("{h:?} is not a patch field"),
    }
}

/// The envelope table is four times then four levels, which is the layout
/// [`EgField`]'s discriminants are numbered in.
fn eg_slot(field: EgField, t: &[f32; 4], l: &[f32; 4]) -> f32 {
    let i = field as usize;
    if i < t.len() { t[i] } else { l[i - t.len()] }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::matrix::{DestId, SourceId};
    use crate::params::{encode, macro_id};
    use crate::patch::N_PATCHES;
    use crate::topology::SlotField;
    use vxn_core_matrix::curve::Polarity;

    const SR: f32 = 48_000.0;

    fn source_edit(slot: u8, src: SourceId) -> SlotEdit {
        SlotEdit {
            slot,
            field: SlotField::Source,
            value: src as u8,
        }
    }

    fn dest_edit(slot: u8, dest: DestId) -> SlotEdit {
        SlotEdit {
            slot,
            field: SlotField::Dest,
            value: dest as u8,
        }
    }

    /// An engine synced to the store: the shell's `activate` followed by its
    /// first drain.
    fn activated(sp: &SharedParams) -> Engine {
        let mut e = Engine::new(SR);
        sp.request_topology_resync();
        sp.sync(&mut e);
        e
    }

    fn render(e: &mut Engine, blocks: usize) -> Vec<f32> {
        let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
        let mut out = Vec::with_capacity(blocks * 256);
        for _ in 0..blocks {
            e.process(&mut l, &mut r);
            out.extend_from_slice(&l);
            out.extend_from_slice(&r);
        }
        out
    }

    // ── The table ───────────────────────────────────────────────────────────

    #[test]
    fn a_fresh_store_holds_the_first_factory_patch() {
        let sp = SharedParams::new();
        let factory: PatchTables = patch(0).into();
        for id in patch_ids() {
            assert_eq!(
                sp.get(id),
                read_field(&factory, id),
                "{}",
                params::desc(id).unwrap().name
            );
        }
        assert_eq!(sp.matrix_snapshot(), factory.matrix);
    }

    /// Host params take their descriptor defaults, not whatever a patch
    /// implies: a macro's position belongs to the song, and a store seeded from
    /// a patch must not have opened one.
    #[test]
    fn a_fresh_store_leaves_the_host_params_at_their_defaults() {
        let sp = SharedParams::new();
        for m in 0..crate::matrix::N_MACROS {
            assert_eq!(sp.get(macro_id(m)), 0.0);
        }
        assert_eq!(sp.get(params::param_for_clap(2).unwrap()), 1.0);
    }

    /// A fresh store owes nothing. Anything else would make the first `sync` of
    /// every session a full rebuild, and would hide a genuinely missing signal
    /// behind one that is always there.
    #[test]
    fn a_fresh_store_owes_no_resync() {
        let sp = SharedParams::new();
        assert!(!sp.take_reload());
        assert_eq!(sp.topology_backlog(), 0);
        assert!(!sp.topology_resync_pending());
        let mut e = Engine::new(SR);
        assert!(!sp.sync(&mut e), "nothing to say to a matching engine");
    }

    /// The two field accessors are mirror images written out twice, so the
    /// thing to pin is that they agree — over every id, not over an example.
    #[test]
    fn every_patch_field_round_trips() {
        let mut t: PatchTables = patch(4).into();
        // A value per id that is inside its range and distinct from the
        // factory's, so a field written to the wrong place is visible.
        for (n, id) in patch_ids().enumerate() {
            let d = params::desc(id).unwrap();
            let v = d.from_fader((n % 7) as f32 / 7.0 + 0.05);
            write_field(&mut t, id, v);
            let back = read_field(&t, id);
            // Enums quantise; everything else must survive exactly.
            let want = if matches!(d.kind, vxn_core_app::ParamKind::Enum { .. }) {
                v.round()
            } else {
                v
            };
            assert_eq!(back, want, "{} did not round-trip", d.name);
        }
    }

    /// A field written through the store must land in that field of the tables
    /// and nowhere else — the positional hazard a 205-entry table carries.
    #[test]
    fn a_value_written_to_the_store_lands_in_its_own_field() {
        for id in patch_ids() {
            let sp = SharedParams::new();
            let d = params::desc(id).unwrap();
            let mut before: PatchTables = patch(0).into();
            sp.write_tables(&mut before);

            // Somewhere in range that is not where it already is.
            let target = if sp.get(id) == d.min { d.max } else { d.min };
            sp.set(id, target);
            let mut after = before;
            sp.write_tables(&mut after);

            for other in patch_ids() {
                let want = if other == id {
                    read_field(&after, id)
                } else {
                    read_field(&before, other)
                };
                assert_eq!(
                    read_field(&after, other),
                    want,
                    "writing {} moved {}",
                    d.name,
                    params::desc(other).unwrap().name
                );
            }
        }
    }

    #[test]
    fn set_clamps_to_the_descriptor_range() {
        let sp = SharedParams::new();
        let ratio = encode(Param::Op {
            op: 0,
            field: OpField::Ratio,
        });
        sp.set(ratio, 1e9);
        assert_eq!(sp.get(ratio), params::desc(ratio).unwrap().max);
        sp.set(ratio, -5.0);
        assert_eq!(sp.get(ratio), params::desc(ratio).unwrap().min);
        // Past the table is a no-op rather than a panic.
        sp.set(ParamId::new(N_PARAMS + 99), 1.0);
        assert_eq!(sp.get(ParamId::new(N_PARAMS + 99)), 0.0);
    }

    /// Only a patch field signals the audio thread. A macro reaches the engine
    /// by the CLAP path, and raising `reload` for it would make every knob
    /// sweep a whole-patch re-read for nothing.
    #[test]
    fn only_a_patch_field_raises_the_reload_flag() {
        let sp = SharedParams::new();
        sp.set(macro_id(3), 0.5);
        assert!(!sp.take_reload());
        sp.set(encode(Param::PatchGain), 0.5);
        assert!(sp.take_reload());
        assert!(!sp.take_reload(), "the flag clears after one read");
    }

    // ── The transport is exact ──────────────────────────────────────────────

    /// The whole ticket in one assertion: a patch that has crossed both
    /// channels renders **bit-identically** to the same patch installed the old
    /// way. Ownership moved; nothing about what is computed did.
    ///
    /// Not a tolerance. A field applied in the wrong order, a derived table
    /// rebuilt one control tick late or a clamp applied twice all produce audio
    /// that is nearly right, and "nearly" is exactly what this must not accept.
    #[test]
    fn a_patch_that_crossed_the_store_renders_bit_identically() {
        for p in 0..N_PATCHES {
            let mut direct = Engine::new(SR);
            direct.set_patch(p);
            direct.note_on(60, 100);
            direct.note_on(67, 100);
            let want = render(&mut direct, 12);

            let sp = SharedParams::new();
            sp.load_factory(p);
            let mut via = Engine::new(SR);
            assert!(sp.sync(&mut via), "the install must reach the engine");
            via.note_on(60, 100);
            via.note_on(67, 100);
            let got = render(&mut via, 12);

            assert_eq!(got, want, "patch {p} did not survive the store");
        }
    }

    /// And with the macros open, so the matrix evaluation, the damping, detune,
    /// pan and spread branches are all in the comparison rather than only the
    /// authored state.
    #[test]
    fn a_modulated_patch_that_crossed_the_store_renders_bit_identically() {
        for p in 0..N_PATCHES {
            let pose = |e: &mut Engine| {
                for m in 0..crate::matrix::N_MACROS {
                    e.set_macro(m, ((m * 5 + p) % 9) as f32 / 8.0);
                }
            };
            let mut direct = Engine::new(SR);
            direct.set_patch(p);
            pose(&mut direct);
            direct.note_on(55, 110);
            let want = render(&mut direct, 12);

            let sp = SharedParams::new();
            sp.load_factory(p);
            let mut via = Engine::new(SR);
            sp.sync(&mut via);
            pose(&mut via);
            via.note_on(55, 110);
            let got = render(&mut via, 12);

            assert_eq!(got, want, "patch {p} moved under modulation");
        }
    }

    // ── Edits do not panic voices ───────────────────────────────────────────

    /// The property the `Edit` arm exists for, and the difference between this
    /// and `set_patch`: a field edit under a held chord is *audible* without
    /// being *silencing*.
    #[test]
    fn a_field_edit_changes_the_sound_without_stealing_the_voices() {
        let sp = SharedParams::new();
        sp.load_factory(1); // epiano
        let mut e = activated(&sp);
        for n in [48u8, 55, 60] {
            e.note_on(n, 100);
        }
        render(&mut e, 4);
        assert_eq!(e.active_voices(), 3);

        let before = render(&mut e, 4);

        // Open a route the patch authors much shallower.
        sp.set(encode(Param::Pm { dest: 0, src: 1 }), 1.4);
        assert!(sp.sync(&mut e), "the edit must reach the engine");
        assert_eq!(e.active_voices(), 3, "an edit stole the sounding voices");
        let after = render(&mut e, 4);
        assert_ne!(after, before, "the edit was inaudible");
    }

    /// `op-N-ratio` is the one field a sounding voice cannot pick up on its
    /// own: the phase increment was cooked at note-on. It has to be pushed.
    #[test]
    fn a_ratio_edit_repitches_notes_already_sounding() {
        let ratio0 = encode(Param::Op {
            op: 0,
            field: OpField::Ratio,
        });
        let run = |edited: bool| {
            let sp = SharedParams::new();
            let mut e = activated(&sp); // sine: op0 straight to the bus
            e.note_on(69, 110);
            render(&mut e, 4);
            if edited {
                sp.set(ratio0, 2.0);
            }
            sp.sync(&mut e);
            render(&mut e, 8)
        };
        assert_ne!(
            run(true),
            run(false),
            "a held note ignored an edit to its own operator's ratio"
        );
    }

    /// Phase and phase spread are read by `reset_lane`, at onset. A note
    /// already sounding has no start phase left to change, so leaving it alone
    /// is behaviour rather than a gap — and the *next* note must take it.
    ///
    /// Two engines arranged identically, one of which gets the edit. The
    /// control makes a **same-value** write so it takes the whole re-sync path
    /// too; otherwise this would be comparing an engine that rebuilt against
    /// one that did not, and any difference between them would be attributed to
    /// the value rather than to the rebuild.
    #[test]
    fn a_phase_spread_edit_reaches_the_next_note_and_not_the_held_one() {
        // `supersaw`, whose stack is authored phase-coherent, so decorrelating
        // one saw of the seven is plainly audible.
        let spread = encode(Param::Op {
            op: 1,
            field: OpField::PhaseSpread,
        });
        let arranged = || {
            let sp = SharedParams::new();
            sp.load_factory(6);
            let mut e = Engine::new(SR);
            sp.sync(&mut e);
            (sp, e)
        };
        let (control, mut a) = arranged();
        let (edited, mut b) = arranged();
        a.note_on(60, 100);
        b.note_on(60, 100);
        render(&mut a, 4);
        render(&mut b, 4);

        control.set(spread, control.get(spread));
        edited.set(spread, 1.0);
        control.sync(&mut a);
        edited.sync(&mut b);
        assert_eq!(
            render(&mut a, 6),
            render(&mut b, 6),
            "the edit reached a note that was already sounding"
        );

        a.note_on(72, 100);
        b.note_on(72, 100);
        assert_ne!(
            render(&mut a, 6),
            render(&mut b, 6),
            "the edit never reached a fresh note either"
        );
    }

    // ── The channels ────────────────────────────────────────────────────────

    /// The drain writes the one field the record names and touches nothing
    /// else — not the other slots, not the depths, and above all not the
    /// values.
    #[test]
    fn a_single_field_edit_applies_that_field_and_nothing_else() {
        let sp = SharedParams::new();
        let mut e = activated(&sp);
        let before = *e.matrix();

        sp.edit_slot(source_edit(7, SourceId::Macro5));
        assert!(!sp.take_reload(), "a topology edit must not flag a re-read");
        assert_eq!(sp.topology_backlog(), 1, "one record per edit");
        let d = sp.drain_topology(&mut e);
        assert!(d.applied && !d.snapshot, "an edit is not a snapshot");

        let after = *e.matrix();
        assert_eq!(after.slots[7].source, SourceId::Macro5);
        for slot in 0..crate::matrix::N_MATRIX_SLOTS {
            if slot != 7 {
                assert_eq!(after.slots[slot], before.slots[slot], "slot {slot}");
            }
        }
        assert_eq!(after.slots[7].dest, before.slots[7].dest, "dest untouched");
        assert_eq!(
            after.slots[7].depth, before.slots[7].depth,
            "depth untouched"
        );
    }

    /// Ring overflow is a defined path, not an argument that it cannot happen:
    /// the dropped record is subsumed by a full snapshot and the audio thread
    /// converges on exactly the store's topology.
    #[test]
    fn a_full_ring_falls_back_to_the_snapshot_path() {
        let sp = SharedParams::new();
        let mut e = activated(&sp);

        // Nothing drains, so the ring fills exactly.
        for i in 0..TOPO_RING_SLOTS {
            sp.edit_slot(source_edit((i % 48) as u8, SourceId::Macro5));
        }
        assert_eq!(sp.topology_backlog(), TOPO_RING_SLOTS, "the ring is full");
        assert!(!sp.topology_resync_pending(), "full is not yet overflowed");

        // One more has nowhere to go. It still lands on the store's table.
        sp.edit_slot(source_edit(5, SourceId::Macro8));
        assert!(sp.topology_resync_pending(), "an overflow owes a snapshot");
        assert_eq!(sp.topology_backlog(), TOPO_RING_SLOTS, "nothing was queued");
        assert_eq!(sp.matrix_snapshot().slots[5].source, SourceId::Macro8);

        // The audio thread drains what fits; the dropped record is still gone.
        assert!(!sp.drain_topology(&mut e).snapshot);
        assert_eq!(e.matrix().slots[5].source, SourceId::Macro5, "tail dropped");

        // Now there is room, so the producer's next service pays the debt.
        sp.service_topology_resync();
        assert!(!sp.topology_resync_pending(), "the snapshot is queued");
        assert_eq!(sp.topology_backlog(), 1, "one snapshot, not 48 slot edits");
        assert!(sp.drain_topology(&mut e).snapshot);

        // Converged, dropped edit included.
        let (store, engine) = (sp.matrix_snapshot(), *e.matrix());
        for slot in 0..crate::matrix::N_MATRIX_SLOTS {
            assert_eq!(
                store.slots[slot].source, engine.slots[slot].source,
                "{slot}"
            );
            assert_eq!(store.slots[slot].dest, engine.slots[slot].dest, "{slot}");
            assert_eq!(
                store.slots[slot].enabled, engine.slots[slot].enabled,
                "{slot}"
            );
        }
    }

    /// While a resync is owed, individual edits are withheld rather than queued
    /// — the snapshot is taken *after* they hit the table, so it carries them.
    #[test]
    fn edits_made_while_a_resync_is_owed_ride_the_snapshot() {
        let sp = SharedParams::new();
        let mut e = activated(&sp);

        for _ in 0..(TOPO_RING_SLOTS + 1) {
            sp.edit_slot(source_edit(0, SourceId::Macro5));
        }
        assert!(sp.topology_resync_pending());
        sp.drain_topology(&mut e);
        assert!(sp.topology_resync_pending(), "the debt outlives the drain");

        sp.edit_slot(source_edit(11, SourceId::Macro8));
        assert!(!sp.topology_resync_pending());
        assert_eq!(sp.topology_backlog(), 1);
        assert!(sp.drain_topology(&mut e).snapshot);
        assert_eq!(
            e.matrix().slots[11].source,
            SourceId::Macro8,
            "the edit made mid-debt never reached the engine"
        );
    }

    /// A bulk install crosses as **one** snapshot, never as 48 slot edits, and
    /// raises the value flag beside it.
    #[test]
    fn an_install_crosses_as_one_snapshot_not_as_edits() {
        let sp = SharedParams::new();
        let mut e = activated(&sp);
        sp.load_factory(4); // web
        assert_eq!(sp.topology_backlog(), 1, "one record for the whole patch");
        assert!(sp.drain_topology(&mut e).snapshot);
        assert!(sp.take_reload(), "an install still re-reads the values");
    }

    /// The coherence property the ticket asks for, stated as an experiment: the
    /// two channels must arrive at the same place whichever order the
    /// producer's stores land in, because the consumer cannot control which it
    /// observes first.
    #[test]
    fn params_and_topology_converge_in_either_order() {
        // The state to reach: a patch's values, and one route the factory bank
        // has nowhere — macro 8 onto op 7's damping, at full depth.
        let arrange = |sp: &SharedParams| {
            sp.edit_slot(source_edit(20, SourceId::Macro8));
            sp.edit_slot(dest_edit(20, DestId::Damp7));
            sp.edit_slot(SlotEdit {
                slot: 20,
                field: SlotField::Enabled,
                value: 1,
            });
            sp.set(encode(Param::MatrixDepth { slot: 20 }), -1.0);
        };

        // (a) values, then topology.
        let a = SharedParams::new();
        let mut ea = activated(&a);
        a.load_factory(2);
        arrange(&a);
        a.sync(&mut ea);

        // (b) topology, then values — the same edits, ordered the other way.
        let b = SharedParams::new();
        let mut eb = activated(&b);
        arrange(&b);
        b.load_factory(2);
        // The install's snapshot supersedes the arranged topology, so re-apply
        // it: what is under test is the *hand-off*, not that a later bulk write
        // loses to an earlier edit.
        arrange(&b);
        b.sync(&mut eb);

        assert_eq!(
            a.matrix_snapshot(),
            b.matrix_snapshot(),
            "the stores differ"
        );
        assert_eq!(*ea.matrix(), *eb.matrix(), "the engines differ");
        for id in patch_ids() {
            assert_eq!(a.get(id), b.get(id), "{}", params::desc(id).unwrap().name);
        }
        ea.note_on(60, 100);
        eb.note_on(60, 100);
        assert_eq!(
            render(&mut ea, 8),
            render(&mut eb, 8),
            "they do not sound alike"
        );
    }

    /// The reverse-order half of the hand-off, isolated. The producer queues
    /// the snapshot and *then* raises `reload`; a consumer that drains an empty
    /// ring and only afterwards reads the flag would otherwise install the new
    /// values over the old topology for a block.
    #[test]
    fn a_reload_seen_after_the_drain_still_finds_its_snapshot() {
        let sp = SharedParams::new();
        let mut e = activated(&sp);

        // Block N: drain an empty ring...
        assert!(!sp.drain_topology(&mut e).applied);
        // ...and the load lands in the window before the flag is read.
        sp.load_factory(4);
        // One `sync` must find both halves, not just the values.
        assert!(sp.sync(&mut e));
        assert_eq!(*e.matrix(), sp.matrix_snapshot(), "topology arrived late");
    }

    /// A snapshot leaves depth alone — it is param-authoritative — and the
    /// re-read it implies seeds it, so the pair converge in one block rather
    /// than the depth briefly being whatever the producer's mirror held.
    #[test]
    fn a_snapshot_leaves_depth_to_the_values() {
        let sp = SharedParams::new();
        let mut e = activated(&sp);
        let depth = encode(Param::MatrixDepth { slot: 4 });

        sp.set(depth, -0.75);
        sp.request_topology_resync();
        assert!(sp.sync(&mut e));
        assert_eq!(e.matrix().slots[4].depth, -0.75);
    }

    /// The structural claim, asserted dynamically. If the drain ever reached
    /// for the store's table this would block for as long as an editor held it
    /// — which on the audio thread is a dropout, and on an idle test machine is
    /// a pass. So: hold the lock, and require the drain to finish anyway.
    #[test]
    fn the_audio_thread_drains_while_the_editor_holds_the_lock() {
        use std::sync::Arc;
        use std::sync::mpsc;
        use std::time::Duration;

        let sp = Arc::new(SharedParams::new());
        let mut e = activated(&sp);
        sp.edit_slot(source_edit(3, SourceId::Macro5));

        let guard = sp.lock();
        let (tx, rx) = mpsc::channel();
        let audio = {
            let sp = Arc::clone(&sp);
            std::thread::spawn(move || {
                let d = sp.drain_topology(&mut e);
                let _ = tx.send(d);
                (e, d)
            })
        };
        let d = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the drain took the topology lock — that is a priority inversion");
        assert!(d.applied);
        drop(guard);
        let (e, _) = audio.join().expect("audio thread");
        assert_eq!(e.matrix().slots[3].source, SourceId::Macro5);
    }

    /// Adopting a factory entry through the store must **not** silence what is
    /// sounding — the difference between this and `Engine::set_patch`, and the
    /// reason a preset browser can be auditioned under a held chord.
    #[test]
    fn loading_a_patch_through_the_store_does_not_panic_the_voices() {
        let sp = SharedParams::new();
        let mut e = activated(&sp);
        for n in [50u8, 57, 62] {
            e.note_on(n, 100);
        }
        render(&mut e, 4);
        assert_eq!(e.active_voices(), 3);

        sp.load_factory(3);
        assert!(sp.sync(&mut e));
        assert_eq!(e.active_voices(), 3, "a preset load stole the voices");
        let out = render(&mut e, 8);
        assert!(out.iter().all(|s| s.is_finite()));

        // And `set_patch` still does, because a host stepping the CLAP patch
        // parameter means something different by it.
        e.set_patch(3);
        assert_eq!(e.active_voices(), 0);
    }

    /// Topology edits that leave a slot pointing at a family the engine has a
    /// fast path for must switch that path on. This is the whole reason a
    /// topology drain rebuilds rather than only writing a field.
    #[test]
    fn a_topology_edit_arms_the_destination_family_it_names() {
        let sp = SharedParams::new();
        let mut e = activated(&sp); // sine, no pan routes
        e.note_on(60, 100);
        render(&mut e, 4);
        let mono = render(&mut e, 4);

        sp.edit_slot(source_edit(30, SourceId::Macro1));
        sp.edit_slot(dest_edit(30, DestId::Pan0));
        sp.edit_slot(SlotEdit {
            slot: 30,
            field: SlotField::Enabled,
            value: 1,
        });
        sp.edit_slot(SlotEdit {
            slot: 30,
            field: SlotField::Polarity,
            value: Polarity::None as u8,
        });
        sp.set(encode(Param::MatrixDepth { slot: 30 }), 1.0);
        e.set_macro(0, 1.0);
        assert!(sp.sync(&mut e));
        assert_ne!(render(&mut e, 4), mono, "the pan route never armed");
    }
}
