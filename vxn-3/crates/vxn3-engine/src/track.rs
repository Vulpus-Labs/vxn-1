//! A track: one active engine + its pattern + mix settings + swap mailbox
//! (ADR 0001 §4). Eight of these make the instrument.

use std::sync::Arc;

use crate::engines::KickTone;
use crate::lane::{LaneState, TrigEvent};
use crate::sequencer::{LockParam, N_LOCK_PARAMS, Pattern};
use crate::swap::EngineSwap;
use crate::track_engine::TrackEngine;

pub struct Track {
    /// The single active engine. Swapped off-thread via [`Track::swap`].
    pub engine: Box<dyn TrackEngine>,
    /// Main↔audio swap mailbox; clone the `Arc` to drive swaps from the main
    /// thread.
    pub swap: Arc<EngineSwap>,
    /// Lane geometry + hit list (each hit carrying its own p-locks).
    pub pattern: Pattern,
    /// Base values of the lockable params (UI-set), indexed by
    /// [`LockParam::index`]: `[gain, pan, macro0, macro1, macro2, send]` (the
    /// decay/tone/pitch lanes are the three engine macro slots). p-locks override
    /// these per step; `effective = override ?? base` (ADR 0001 §3a).
    base: [f32; N_LOCK_PARAMS],
    /// Last applied effective value per param, so knob re-cooks only fire on a
    /// real change. Seeded to NaN so the first block applies the base.
    applied: [f32; N_LOCK_PARAMS],
    /// Mute gate: when set, the track renders but contributes nothing to the mix
    /// (host-automatable mix param, 0171). Independent of `level` so unmuting
    /// restores the prior gain.
    muted: bool,
    /// Choke group (0 = none). Tracks sharing a non-zero group cut each other: a trig on any
    /// member fast-releases the others' sounding voices (808 open/closed hat behaviour, as a
    /// track-routing relationship rather than an engine-config one).
    choke_group: u8,
    /// Pre-allocated mono render scratch (sized at construction).
    mono: Vec<f32>,
}

impl Track {
    /// A track defaulting to a `Kick/Tone` engine and an empty pattern, using
    /// the given (shared) swap mailbox so the main thread can hand it engines.
    pub fn new(sample_rate: f32, max_block: usize, swap: Arc<EngineSwap>) -> Self {
        Self {
            engine: Box::new(KickTone::with_default_patch(sample_rate)),
            swap,
            pattern: Pattern::default(),
            // gain 1, pan 0, knobs at midpoint, send 0 (matches faceplate defaults).
            base: [1.0, 0.0, 0.5, 0.5, 0.5, 0.0],
            applied: [f32::NAN; N_LOCK_PARAMS],
            muted: false,
            choke_group: 0,
            mono: vec![0.0; max_block],
        }
    }

    /// Assign the track's choke group (0 = none).
    pub fn set_choke_group(&mut self, group: u8) {
        self.choke_group = group;
    }

    /// The track's choke group (0 = none).
    pub fn choke_group(&self) -> u8 {
        self.choke_group
    }

    /// Set a lockable param's base value (from a UI command).
    pub fn set_base(&mut self, param: LockParam, value: f32) {
        self.base[param.index()] = value;
    }

    /// Mute / unmute the track (gates its mix contribution; see [`Track::pan_gains`]).
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    /// This block's effective (post-p-lock) value for a lockable param — the
    /// resolved value the mix/engine actually used. Read by the host-param echo
    /// (0173). Seeded to the base until the first `apply_effective`.
    pub fn effective(&self, param: LockParam) -> f32 {
        let a = self.applied[param.index()];
        if a.is_nan() { self.base[param.index()] } else { a }
    }

    /// Whether the track is currently muted.
    pub fn is_muted(&self) -> bool {
        self.muted
    }

    /// Forget the last-applied params so the next [`Track::apply_effective`]
    /// re-pushes every value to the engine — used after an engine swap, whose
    /// fresh engine starts at its default patch (0174).
    pub fn invalidate_applied(&mut self) {
        self.applied = [f32::NAN; N_LOCK_PARAMS];
    }

    /// Resolve this block's effective params (`override ?? base`) and apply any
    /// that changed: gain/pan feed [`Track::pan_gains`]; knob changes re-cook the
    /// engine. Called once per block before render. Allocation-free.
    ///
    /// This is the **host** layer of the macro slots — automation and p-locks, per
    /// block. A firing hit's own colour outranks both and is applied per trig in
    /// [`Track::render_with_hits`]; it never comes back through here (ADR 0007 §7,
    /// precedence documented on [`crate::flavour::resolve`]).
    pub fn apply_effective(&mut self, lane: &LaneState) {
        for p in 0..N_LOCK_PARAMS {
            let eff = lane.override_value(p).unwrap_or(self.base[p]);
            if eff != self.applied[p] {
                self.applied[p] = eff;
                match p {
                    // gain / pan / send: read from `applied` in the mix
                    0 | 1 | 5 => {}
                    // decay / tone / pitch lanes → macro slots 0/1/2 (ADR 0003 §2)
                    2 => self.engine.set_macro(0, eff),
                    3 => self.engine.set_macro(1, eff),
                    4 => self.engine.set_macro(2, eff),
                    _ => {}
                }
            }
        }
    }

    /// Install a pending off-thread engine swap, if any. Audio-thread,
    /// allocation-free. Returns `true` when a swap happened.
    #[inline]
    pub fn poll_swap(&mut self) -> bool {
        self.swap.try_install(&mut self.engine)
    }

    /// Equal-power pan gains `(left, right)`, from the effective gain/pan.
    #[inline]
    pub fn pan_gains(&self) -> (f32, f32) {
        if self.muted {
            return (0.0, 0.0);
        }
        let gain = self.applied[0];
        let angle = (self.applied[1].clamp(-1.0, 1.0) * 0.5 + 0.5) * std::f32::consts::FRAC_PI_2;
        (gain * angle.cos(), gain * angle.sin())
    }

    /// Render this track for the block into its mono scratch, applying the
    /// pre-scheduled `hits` sample-accurately by slicing the render at each hit's
    /// frame offset. `hits` are frame-ordered and clamped to `[0, frames]` by the
    /// scheduler ([`crate::lane`]). Allocation-free.
    /// `chokes` are sample offsets (sorted) at which a sibling choke-group track fires — the
    /// engine is fast-released at each. Hits and chokes are merged by frame so both stay
    /// sample-accurate; at a shared frame the choke is applied before the trig.
    pub fn render_with_hits(&mut self, hits: &[TrigEvent], chokes: &[usize], frames: usize) {
        let frames = frames.min(self.mono.len());
        let engine: &mut dyn TrackEngine = &mut *self.engine;
        let mono = &mut self.mono[..frames];

        let mut pos = 0usize;
        let (mut hi, mut ci) = (0usize, 0usize);
        loop {
            let hf = hits.get(hi).map_or(usize::MAX, |h| h.frame.min(frames));
            let cf = chokes.get(ci).map_or(usize::MAX, |&c| c.min(frames));
            let f = hf.min(cf);
            if f == usize::MAX {
                break;
            }
            if f > pos {
                engine.render(&mut mono[pos..f]);
                pos = f;
            }
            // Choke first at a shared frame, so a closed hit cuts the ring it lands on.
            if cf == f {
                engine.choke();
                ci += 1;
            } else {
                // The trig carries its own modulation (ADR 0007 §7): the hit's colour
                // as a macro vector, and its lateness in the swung slot. Handed to the
                // engine *with* the trig rather than pushed through `set_macro`
                // beforehand — a per-hit override must not become host param state, and
                // `apply_effective`'s per-block values stay exactly what the host set.
                let h = hits[hi];
                engine.on_trig_with(h.note, h.velocity, h.modulation);
                hi += 1;
            }
        }
        engine.render(&mut mono[pos..frames]);
    }

    /// Mix the rendered mono scratch into the dry stereo bus (gain/pan) and the
    /// stereo delay-send bus (centred, scaled by the effective send amount — the
    /// p-lockable dub throw).
    #[inline]
    pub fn mix_into(
        &self,
        out_l: &mut [f32],
        out_r: &mut [f32],
        send_l: &mut [f32],
        send_r: &mut [f32],
        frames: usize,
    ) {
        let frames = frames
            .min(self.mono.len())
            .min(out_l.len())
            .min(out_r.len())
            .min(send_l.len())
            .min(send_r.len());
        let (gl, gr) = self.pan_gains();
        let send = self.applied[5]; // effective send amount (override ?? base)
        for f in 0..frames {
            let s = self.mono[f];
            out_l[f] += s * gl;
            out_r[f] += s * gr;
            let snd = s * send;
            send_l[f] += snd;
            send_r[f] += snd;
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.engine.set_sample_rate(sample_rate);
    }

    pub fn reset(&mut self) {
        self.engine.reset();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::sequencer::{Lock, Termination};
    use crate::swap::EngineSwap;
    use crate::track_engine::{EngineKind, MACRO_SLOTS, TrigMod};

    const SR: f32 = 48_000.0;
    const BPS: f64 = 120.0 / 60.0 / 48_000.0;
    const BLOCK: usize = 12_000; // two 16ths at 120/48k

    /// What a trig actually resolved against, and everything the host wrote.
    #[derive(Default)]
    struct Log {
        /// The macro vector each trig would hand `flavour::resolve`.
        resolved: Vec<[f32; MACRO_SLOTS]>,
        /// Every `set_macro` — the host-state writes. A per-hit override must appear
        /// in `resolved` and **never** here.
        host_writes: Vec<(usize, f32)>,
    }

    /// A stand-in engine that resolves its sources exactly as a real family does, so
    /// the precedence rule is tested where it is implemented rather than re-stated.
    struct Spy {
        macros: [f32; MACRO_SLOTS],
        log: Arc<Mutex<Log>>,
    }

    impl TrackEngine for Spy {
        fn render(&mut self, out: &mut [f32]) {
            out.fill(0.0);
        }
        fn on_trig(&mut self, _note: f32, _velocity: f32) {
            self.log.lock().unwrap().resolved.push(self.macros);
        }
        fn on_trig_with(&mut self, _note: f32, _velocity: f32, m: TrigMod) {
            let s = m.sources(&self.macros);
            self.log.lock().unwrap().resolved.push([s[0], s[1], s[2]]);
        }
        fn reset(&mut self) {}
        fn set_sample_rate(&mut self, _sr: f32) {}
        fn kind(&self) -> EngineKind {
            EngineKind::KickTone
        }
        fn set_macro(&mut self, slot: usize, value: f32) {
            if slot < MACRO_SLOTS {
                self.macros[slot] = value;
            }
            self.log.lock().unwrap().host_writes.push((slot, value));
        }
    }

    fn spied_track() -> (Track, Arc<Mutex<Log>>) {
        let log = Arc::new(Mutex::new(Log::default()));
        let mut track = Track::new(SR, BLOCK, EngineSwap::new());
        track.engine = Box::new(Spy { macros: [0.0; MACRO_SLOTS], log: log.clone() });
        (track, log)
    }

    /// AC: precedence between a per-hit colour and a p-lock on the same macro slot,
    /// **both ways round**, in one pass of one lane.
    ///
    /// The lock is a `Latch` on Decay — macro slot 0 — set by the hit on slot 0 and
    /// still held when the hit on slot 1 fires. The coloured hit ignores it; the
    /// uncoloured one obeys it. That is exactly the rule documented on
    /// [`crate::flavour::resolve`]: a colour is attached to the hit being fired, and a
    /// latched lock is not.
    #[test]
    fn a_per_hit_colour_outranks_a_p_lock_and_an_uncoloured_hit_does_not() {
        let (mut track, log) = spied_track();
        track.pattern.set(0, 36.0, 1.0);
        track.pattern.set(1, 36.0, 1.0);
        track.pattern.set_colour(0, [0.1, 0.2, 0.3]);
        track
            .pattern
            .set_lock(0, LockParam::Decay, Lock { value: 0.9, termination: Termination::Latch });

        let mut lane = LaneState::new(0);
        let mut hits = Vec::with_capacity(16);
        let pattern = track.pattern;
        lane.schedule(&pattern, 0.0, BPS, BLOCK, true, &mut hits);
        assert_eq!(hits.len(), 2, "both hits fire in this block");
        assert_eq!(lane.override_value(LockParam::Decay.index()), Some(0.9), "the latch is live");

        track.apply_effective(&lane);
        track.render_with_hits(&hits, &[], BLOCK);

        let log = log.lock().unwrap();
        assert_eq!(
            log.resolved[0],
            [0.1, 0.2, 0.3],
            "the coloured hit's own vector wins over the latched lock"
        );
        assert_eq!(
            log.resolved[1],
            [0.9, 0.5, 0.5],
            "the uncoloured hit takes the lock on slot 0 and the base on 1 and 2"
        );
    }

    /// AC: a per-hit override does not write back to host macro param state. After the
    /// trig the host's echo (`effective`, what `get_value` reports) is still the
    /// automated value, and the engine was never handed the colour through `set_macro`.
    #[test]
    fn a_per_hit_colour_never_becomes_host_macro_state() {
        let (mut track, log) = spied_track();
        track.set_base(LockParam::Decay, 0.42);
        track.pattern.set(0, 36.0, 1.0);
        track.pattern.set_colour(0, [1.0, 1.0, 1.0]);

        let mut lane = LaneState::new(0);
        let mut hits = Vec::with_capacity(16);
        let pattern = track.pattern;
        lane.schedule(&pattern, 0.0, BPS, BLOCK, true, &mut hits);
        track.apply_effective(&lane);
        track.render_with_hits(&hits, &[], BLOCK);

        assert_eq!(log.lock().unwrap().resolved[0], [1.0, 1.0, 1.0], "the colour reached resolve");
        assert_eq!(
            track.effective(LockParam::Decay),
            0.42,
            "the host echo must still report the automated value"
        );
        // The one write is `apply_effective`'s, carrying the automated value — never
        // the hit's colour.
        let writes = log.lock().unwrap().host_writes.clone();
        assert_eq!(writes.iter().filter(|(s, _)| *s == 0).collect::<Vec<_>>(), vec![&(0, 0.42)]);

        // A second block with no colour in it resolves against that same host value,
        // which is the observable form of "the override was not sticky".
        track.pattern.clear_colour(0);
        let pattern = track.pattern;
        lane.reset();
        lane.schedule(&pattern, 0.0, BPS, BLOCK, true, &mut hits);
        track.apply_effective(&lane);
        track.render_with_hits(&hits, &[], BLOCK);
        assert_eq!(log.lock().unwrap().resolved[1], [0.42, 0.5, 0.5]);
    }
}
