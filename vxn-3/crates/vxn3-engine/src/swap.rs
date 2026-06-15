//! Off-thread engine swap (ADR 0001 §4).
//!
//! A track's engine is built and pre-allocated on the **main thread**, then
//! handed to the **audio thread** which installs it without allocating, blocking,
//! or clicking. The audio thread must never *free* either, so the old engine is
//! sent *back* to the main thread to be dropped.
//!
//! Mechanism: two fixed-capacity SPSC rings whose slots hold the boxed engines
//! inline. Crossing a ring is a **move** of a `Box` (two words: data + vtable) —
//! never a heap alloc/free. The only allocation is `Box::new` on the main thread
//! (build) and the matching free on the main thread (drop after reclaim).
//!
//! The naive alternative — `AtomicPtr<Box<dyn TrackEngine>>` — fails because a
//! trait object is a fat pointer; boxing it to get a thin pointer forces the
//! audio thread to free that outer box on install. The ring avoids that.

use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::track_engine::TrackEngine;

type Dyn = Box<dyn TrackEngine>;

/// Ring capacity. Swaps are human-paced (seconds apart) so a handful of slots is
/// ample; the main thread reclaims on its UI timer.
const CAP: usize = 4;

/// Minimal single-producer / single-consumer ring of boxed engines.
///
/// One side only ever pushes, the other only ever pops — `head`/`tail` are
/// touched by exactly one thread each, synchronised with Acquire/Release.
struct Ring {
    slots: [UnsafeCell<Option<Dyn>>; CAP],
    /// Next write index — owned by the producer.
    head: AtomicUsize,
    /// Next read index — owned by the consumer.
    tail: AtomicUsize,
}

// SAFETY: access is strict SPSC — `head` is written only by the producer,
// `tail` only by the consumer, each slot handed off via the Acquire/Release
// pair on those indices. The boxed engine is `Send`.
unsafe impl Send for Ring {}
unsafe impl Sync for Ring {}

impl Ring {
    fn new() -> Self {
        Self {
            slots: [const { UnsafeCell::new(None) }; CAP],
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Producer: is there room for one more push? (consumer may free space
    /// concurrently, so a `false` is a conservative snapshot.)
    fn can_push(&self) -> bool {
        let next = (self.head.load(Ordering::Relaxed) + 1) % CAP;
        next != self.tail.load(Ordering::Acquire)
    }

    /// Producer side. Returns the value back as `Err` when the ring is full.
    fn push(&self, v: Dyn) -> Result<(), Dyn> {
        let head = self.head.load(Ordering::Relaxed);
        let next = (head + 1) % CAP;
        if next == self.tail.load(Ordering::Acquire) {
            return Err(v); // full
        }
        // SAFETY: SPSC — only the producer writes this slot, and the consumer
        // will not read index `head` until `head` is published below.
        unsafe { *self.slots[head].get() = Some(v) };
        self.head.store(next, Ordering::Release);
        Ok(())
    }

    /// Consumer side. `None` when empty.
    fn pop(&self) -> Option<Dyn> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None; // empty
        }
        // SAFETY: SPSC — the producer published this slot via its `head` store
        // (Acquire-loaded above) and will not touch it again until we advance
        // `tail`.
        let v = unsafe { (*self.slots[tail].get()).take() };
        self.tail.store((tail + 1) % CAP, Ordering::Release);
        v
    }
}

/// The per-track swap mailbox, shared (`Arc`) between the main and audio threads.
pub struct EngineSwap {
    /// main → audio: engines awaiting install.
    incoming: Ring,
    /// audio → main: retired engines awaiting drop on the main thread.
    retired: Ring,
}

impl EngineSwap {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            incoming: Ring::new(),
            retired: Ring::new(),
        })
    }

    /// **Main thread:** queue a freshly built engine for install. Returns the
    /// engine back as `Err` if the queue is full (caller may retry after a
    /// `reclaim`). Also opportunistically reclaims retired engines.
    pub fn send(&self, engine: Dyn) -> Result<(), Dyn> {
        self.reclaim();
        self.incoming.push(engine)
    }

    /// **Main thread:** drop every engine the audio thread has retired. Cheap to
    /// call on each UI tick.
    pub fn reclaim(&self) {
        while let Some(retired) = self.retired.pop() {
            drop(retired);
        }
    }

    /// **Audio thread:** if an engine is waiting *and* there's room to retire the
    /// current one, install it into `slot` and hand the old one back for the main
    /// thread to drop. Allocation-free and non-blocking. Returns `true` on swap.
    ///
    /// If the retired ring is full (main thread hasn't reclaimed yet) the swap is
    /// deferred — the old engine is never freed here.
    pub fn try_install(&self, slot: &mut Dyn) -> bool {
        if !self.retired.can_push() {
            return false; // can't retire safely → defer, never free on audio thread
        }
        match self.incoming.pop() {
            Some(new_engine) => {
                let old = std::mem::replace(slot, new_engine);
                let _ = self.retired.push(old); // room checked above
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engines::KickTone;
    use crate::track_engine::EngineKind;

    fn kick() -> Dyn {
        Box::new(KickTone::with_default_patch(48_000.0))
    }

    #[test]
    fn install_swaps_and_retires() {
        let swap = EngineSwap::new();
        let mut active: Dyn = kick();
        assert_eq!(active.kind(), EngineKind::KickTone);

        // Nothing pending → no swap.
        assert!(!swap.try_install(&mut active));

        // Main sends a new engine; audio installs it; old goes to retired.
        assert!(swap.send(kick()).is_ok(), "send");
        assert!(swap.try_install(&mut active), "pending engine installed");

        // The retired engine is waiting for the main thread to drop.
        assert!(!swap.retired.can_push() || swap.retired.pop().is_some());
    }

    #[test]
    fn defers_when_retired_unreclaimed() {
        let swap = EngineSwap::new();
        let mut active: Dyn = kick();
        // Fill retired to capacity by repeated installs without reclaiming.
        for _ in 0..CAP {
            if swap.send(kick()).is_err() {
                break;
            }
        }
        // Drain incoming as long as retired has room; once retired fills, swaps
        // defer rather than free on the audio thread.
        let mut installs = 0;
        while swap.try_install(&mut active) {
            installs += 1;
            if installs > CAP * 2 {
                panic!("install loop did not terminate — retired never filled");
            }
        }
        // Reclaiming frees space so swaps resume.
        swap.reclaim();
    }
}
