//! `DirtyBits` — a lock-free Model → View change channel.
//!
//! One bit per id, flipped by whichever thread writes the value and drained by
//! a **single** reader (the main-thread editor tick). Extracted from vxn-2's
//! `SharedParams`, where it has shipped since vxn-2
//! [ADR 0003](../../../vxn-2/adrs/0003-dirty-bitset-diff-pump.md); three synths
//! now want it, so it lives here rather than being copied a third time.
//!
//! It replaces the poll-and-diff idiom (walk every param each tick, compare
//! against a mirror, emit the differences). The bitset says *which* ids moved,
//! so the tick's cost tracks the number of changes rather than the size of the
//! table.
//!
//! # Ordering contract
//!
//! This is the part most likely to be re-derived wrongly, so it is stated once,
//! here, rather than at each call site:
//!
//! - **Writer:** store the value `Relaxed`, **then** [`mark`](DirtyBits::mark)
//!   the bit (`Release`).
//! - **Reader:** [`drain`](DirtyBits::drain) the bits (`Acquire`), **then** load
//!   the values `Relaxed`.
//!
//! The Release/Acquire pair guarantees a reader that observes a set bit also
//! observes the value the writer stored before setting it. Stronger orderings
//! buy nothing for scalar param updates.
//!
//! **Single reader.** `drain` is `swap(0)`, so two concurrent drainers would
//! split the change set between them and each would emit half. One reader.
//!
//! # Race windows (both benign — no event is lost)
//!
//! - **Write between the reader's swap and its value load.** The swap already
//!   returned 0 for that bit, so the reader does not emit this round; the
//!   writer's bit is now set and the *next* drain emits with the latest value.
//! - **Write between the writer's value store and its bit set.** The reader can
//!   observe an empty bitset in that gap and skips the round. The next drain
//!   sees the bit and reads the value — which is already stored, so it is the
//!   right one.
//!
//! # Coalescing
//!
//! Several writes to one id between two drains produce **one** notification
//! carrying the latest value. That is what both consumers want: the host does
//! not need every intermediate of a fader drag, and the view does not paint
//! them.

use core::sync::atomic::{AtomicU64, Ordering};

/// Words needed to hold `n` bits. Use to compute a `DirtyBits`' `N_WORDS`:
///
/// ```
/// use vxn_core_utils::dirty::{DirtyBits, words_for};
/// const N: usize = 209;
/// type Bits = DirtyBits<{ words_for(N) }, N>;
/// let b: Bits = DirtyBits::new_all_set();
/// assert_eq!(b.count(), N);
/// ```
#[inline]
pub const fn words_for(n_bits: usize) -> usize {
    n_bits.div_ceil(64)
}

/// A bitset of `N_BITS` change flags over `N_WORDS` atomic words.
///
/// `N_WORDS` must be `words_for(N_BITS)`; the constructors debug-assert it.
/// Both are parameters rather than one derived from the other because
/// `[AtomicU64; words_for(N)]` needs `generic_const_exprs` to express.
///
/// Bits beyond `N_BITS` in the tail word are held clear by construction, so
/// [`mark_all`](Self::mark_all) followed by a drain never yields a phantom id
/// past the end of the table.
#[derive(Debug)]
pub struct DirtyBits<const N_WORDS: usize, const N_BITS: usize> {
    words: [AtomicU64; N_WORDS],
}

impl<const N_WORDS: usize, const N_BITS: usize> DirtyBits<N_WORDS, N_BITS> {
    /// Number of ids this set covers.
    pub const COUNT: usize = N_BITS;

    /// Mask of the valid bits in word `w`. Out-of-range bits in the last word
    /// are zero.
    #[inline]
    const fn full_word(w: usize) -> u64 {
        let start = w * 64;
        if start >= N_BITS {
            0
        } else {
            let n = N_BITS - start;
            if n >= 64 { u64::MAX } else { (1u64 << n) - 1 }
        }
    }

    /// Empty — nothing pending. The next drain yields nothing.
    #[inline]
    pub fn new_empty() -> Self {
        debug_assert!(N_WORDS == words_for(N_BITS));
        Self { words: core::array::from_fn(|_| AtomicU64::new(0)) }
    }

    /// Fully seeded — every valid id pending, tail-word padding clear.
    ///
    /// This is the usual choice for a param store: the first tick after the
    /// editor opens then broadcasts the whole table, so the view paints a
    /// complete picture without a separate "send me everything" path.
    #[inline]
    pub fn new_all_set() -> Self {
        debug_assert!(N_WORDS == words_for(N_BITS));
        Self { words: core::array::from_fn(|w| AtomicU64::new(Self::full_word(w))) }
    }

    /// Mark `id` changed. `Release`, so it publishes the value the caller
    /// stored before calling this.
    ///
    /// Out-of-range ids are ignored rather than panicking: this runs on the
    /// audio thread, where a panic is worse than a dropped notification.
    #[inline]
    pub fn mark(&self, id: usize) {
        if id >= N_BITS {
            return;
        }
        self.words[id / 64].fetch_or(1u64 << (id % 64), Ordering::Release);
    }

    /// Mark every valid id changed — the bulk-store path (state load, reset to
    /// defaults, first tick after init).
    #[inline]
    pub fn mark_all(&self) {
        for w in 0..N_WORDS {
            self.words[w].fetch_or(Self::full_word(w), Ordering::Release);
        }
    }

    /// Drain the set, calling `f` once per changed id in ascending order.
    ///
    /// The callback form keeps the set-bit walk in one place instead of
    /// re-appearing at every drain site, and lets the caller emit directly
    /// rather than collecting into a `Vec` first.
    #[inline]
    pub fn drain(&self, mut f: impl FnMut(usize)) {
        for w in 0..N_WORDS {
            let mut bits = self.words[w].swap(0, Ordering::Acquire);
            while bits != 0 {
                let b = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let id = w * 64 + b;
                // Padding is masked at construction and in `mark_all`, so this
                // cannot normally fire — it is a cheap belt to the braces.
                if id < N_BITS {
                    f(id);
                }
            }
        }
    }

    /// Drain into raw words, for a caller that needs the bit pattern itself.
    /// Prefer [`drain`](Self::drain).
    #[inline]
    pub fn take(&self) -> [u64; N_WORDS] {
        core::array::from_fn(|w| self.words[w].swap(0, Ordering::Acquire))
    }

    /// Is anything pending? Advisory only — a writer may set a bit the instant
    /// after this returns.
    #[inline]
    pub fn any(&self) -> bool {
        self.words.iter().any(|w| w.load(Ordering::Relaxed) != 0)
    }

    /// How many ids are pending. Advisory, as [`any`](Self::any).
    #[inline]
    pub fn count(&self) -> usize {
        self.words.iter().map(|w| w.load(Ordering::Relaxed).count_ones() as usize).sum()
    }
}

impl<const N_WORDS: usize, const N_BITS: usize> Default for DirtyBits<N_WORDS, N_BITS> {
    fn default() -> Self {
        Self::new_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 209 = vxn-2's TOTAL_PARAMS: 3 full words + 17 bits of a 4th, so the tail
    // word is genuinely partial and the padding has somewhere to go wrong.
    const N: usize = 209;
    type Bits = DirtyBits<{ words_for(N) }, N>;

    fn drained(b: &Bits) -> Vec<usize> {
        let mut v = Vec::new();
        b.drain(|id| v.push(id));
        v
    }

    #[test]
    fn words_for_rounds_up() {
        assert_eq!(words_for(0), 0);
        assert_eq!(words_for(1), 1);
        assert_eq!(words_for(64), 1);
        assert_eq!(words_for(65), 2);
        assert_eq!(words_for(209), 4);
    }

    #[test]
    fn seeded_full_drains_every_valid_id_exactly_once() {
        let b = Bits::new_all_set();
        assert_eq!(drained(&b), (0..N).collect::<Vec<_>>());
    }

    #[test]
    fn tail_word_padding_never_surfaces() {
        // Both routes to a full set must mask the tail word.
        let seeded = Bits::new_all_set();
        assert_eq!(drained(&seeded).last().copied(), Some(N - 1));

        let marked = Bits::new_empty();
        marked.mark_all();
        let ids = drained(&marked);
        assert_eq!(ids.last().copied(), Some(N - 1));
        assert!(ids.iter().all(|&id| id < N), "a phantom id past the table surfaced");
        assert_eq!(ids.len(), N);
    }

    #[test]
    fn a_second_drain_with_no_writes_yields_nothing() {
        let b = Bits::new_all_set();
        assert_eq!(drained(&b).len(), N);
        assert_eq!(drained(&b), Vec::<usize>::new());
    }

    #[test]
    fn mark_after_a_drain_surfaces_on_the_next_drain() {
        let b = Bits::new_empty();
        assert_eq!(drained(&b), Vec::<usize>::new());
        b.mark(0);
        b.mark(63);
        b.mark(64);
        b.mark(N - 1);
        assert_eq!(drained(&b), vec![0, 63, 64, N - 1]);
        assert_eq!(drained(&b), Vec::<usize>::new());
    }

    #[test]
    fn repeated_marks_coalesce_to_one_notification() {
        let b = Bits::new_empty();
        for _ in 0..10 {
            b.mark(7);
        }
        assert_eq!(drained(&b), vec![7]);
    }

    #[test]
    fn an_out_of_range_mark_is_ignored_not_panicking() {
        let b = Bits::new_empty();
        b.mark(N);
        b.mark(usize::MAX);
        assert_eq!(drained(&b), Vec::<usize>::new());
    }

    #[test]
    fn empty_starts_empty_and_full_starts_full() {
        assert_eq!(Bits::new_empty().count(), 0);
        assert!(!Bits::new_empty().any());
        assert_eq!(Bits::new_all_set().count(), N);
        assert!(Bits::new_all_set().any());
    }

    #[test]
    fn take_returns_the_raw_words_and_clears() {
        let b = Bits::new_empty();
        b.mark(1);
        b.mark(70);
        let w = b.take();
        assert_eq!(w[0], 1u64 << 1);
        assert_eq!(w[1], 1u64 << 6);
        assert_eq!(b.take(), [0u64; words_for(N)]);
    }

    #[test]
    fn an_exact_multiple_of_64_has_no_padding() {
        const M: usize = 128;
        type Exact = DirtyBits<{ words_for(M) }, M>;
        let b = Exact::new_all_set();
        assert_eq!(b.count(), M);
        let mut v = Vec::new();
        b.drain(|id| v.push(id));
        assert_eq!(v, (0..M).collect::<Vec<_>>());
    }
}
