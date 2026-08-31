//! Is this routing worth making? The **coherence predicate** — one rule shared
//! by both synths, plus whatever special cases each one has of its own.
//!
//! A routing is coherent iff the source's granularity tier is coarser-or-equal
//! to the destination's ([`Tier::covers`](crate::roster::Tier::covers)). A
//! coarser source broadcasts unambiguously to a finer dest — one patch-global
//! LFO reaches every lane and every lane gets the same value. A finer source
//! into a coarser dest has no such answer: eight lanes offer eight values and
//! the dest can hold one, so the engine takes lane 0 and the other seven are
//! silently discarded. That is not an error the audio path can report — the
//! route *works*, it just does not do what the patch says — so the verdict is a
//! UI surface, computed once when the faceplate's pick-lists are built and read
//! back per row.
//!
//! ## Why the special cases are a per-synth hook
//!
//! The tier rule is arithmetic on two declared columns and is the same
//! everywhere. The special cases are not: vxn-2's two both name *its* variants
//! (`lfo1 → lfo1-rate`, `voice-idx → cutoff`), and one of them is not even a
//! shared judgement — vxn-1b's `lfo1 → lfo1-rate` is a documented, working
//! route that reads the previous control block's total, deliberately lagged by
//! one block. Hard-coding "an LFO may not drive its own rate" into the shared
//! engine would flag a feature vxn-1b shipped on purpose.
//!
//! So [`CoherenceRoster`](crate::coherence::CoherenceRoster) owns the *shape*
//! of the predicate — the verdict vocabulary, the empty-slot short circuit, the
//! precedence between the special cases and the tier rule — and each synth
//! supplies only the cases the shape has a hole for. vxn-1b supplies none and
//! takes the default, which is how it gets the whole surface for the cost of
//! two tier lookups.
//!
//! ## Why this is not keyed on [`MatrixRoster`](crate::roster::MatrixRoster)
//!
//! [`MatrixRoster`](crate::roster::MatrixRoster) is the audio-thread seam and
//! speaks **storage indices**, `0..N`, from which the empty-slot sentinel is
//! excluded by construction (ADR 0003 §2). Coherence is the opposite surface: it
//! is a descriptor built for a UI, addressed by the synth's own **wire
//! discriminants** with the sentinel at 0 — a faceplate's first pick-list entry
//! is "—", and asking for its verdict has to be answerable. A trait keyed on the
//! synth's own id types is the honest spelling of that, and it keeps the two
//! seams from being forced to agree about a sentinel only one of them has.

use crate::roster::Tier;

/// Why a routing is degenerate or incoherent, or [`Coherence::Ok`] if it sounds.
///
/// One vocabulary for both synths, and the single source of truth behind the
/// tooltip a faceplate shows: the descriptor exports these verdicts by
/// [`name`](Coherence::name) so a UI reads the engine's answer rather than
/// re-deriving the rule and drifting from it.
///
/// The discriminants are stable because vxn-2's descriptor has exported them
/// since E008 0090; the *names* are the wire contract, not the numbers.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum Coherence {
    /// Coherent — the source tier is coarser-or-equal to the dest tier, or the
    /// slot is empty.
    #[default]
    Ok = 0,
    /// A finer source into a coarser dest: the per-lane (or per-stack) value
    /// collapses to a single lane — lossy, and ambiguous about which lane won.
    TierCollapse = 1,
    /// A modulator driving its own rate: self-referential. A per-synth special
    /// case — whether it is an error at all depends on how that synth orders
    /// its block (vxn-2 says no; vxn-1b runs exactly this route on purpose,
    /// lagged one control block).
    SelfRate = 2,
    /// A route that is arithmetically a constant zero — the source's value at
    /// the lane the dest collapses to is always 0, so the route has no effect
    /// at any depth. A per-synth special case: only the synth knows which of
    /// its sources are zero at lane 0.
    Degenerate = 3,
}

impl Coherence {
    /// Machine name for the descriptor export and the faceplate's tooltip
    /// table. **This string is the cross-language contract** — a faceplate keys
    /// its reason text off it, so renaming one here without the other silently
    /// kills the warning rather than breaking a build.
    #[inline]
    pub const fn name(self) -> &'static str {
        match self {
            Coherence::Ok => "ok",
            Coherence::TierCollapse => "tier-collapse",
            Coherence::SelfRate => "self-rate",
            Coherence::Degenerate => "degenerate",
        }
    }

    /// Whether this verdict is one a UI should flag. Sugar for `!= Ok`, spelled
    /// out because "is this row red?" reads better than a negated comparison at
    /// the call sites that ask it.
    #[inline]
    pub const fn is_flagged(self) -> bool {
        !matches!(self, Coherence::Ok)
    }
}

/// A synth's routing vocabulary, as the coherence predicate needs to see it.
///
/// Implemented on a zero-sized marker per synth (per *layer*, if a synth ever
/// gives its layers different rosters), naming the synth's own source and
/// destination id types. Two required methods carry the declared tiers; the
/// third is the hook for special cases and defaults to none.
///
/// Both tier methods return `Option<Tier>`, with `None` meaning "this is the
/// empty-slot sentinel". That is deliberately not the same as reporting some
/// inert tier: the sentinel has no granularity, and making the caller spell
/// that out is what lets [`coherence`] own the short circuit instead of leaving
/// each synth to remember it.
pub trait CoherenceRoster {
    /// The synth's source id type, sentinel included.
    type Source: Copy;
    /// The synth's destination id type, sentinel included.
    type Dest: Copy;

    /// Granularity tier of `src`, or `None` for the empty-slot sentinel.
    fn source_tier(src: Self::Source) -> Option<Tier>;

    /// Granularity tier of `dst`, or `None` for the empty-slot sentinel.
    fn dest_tier(dst: Self::Dest) -> Option<Tier>;

    /// This synth's own verdict for a pair, checked **before** the tier rule so
    /// that a specific reason wins over the generic one — `voice-idx → cutoff`
    /// is both a tier collapse and a constant zero, and "no effect" is the more
    /// useful thing to tell a player.
    ///
    /// Never called for an empty slot. Returning `Some(Coherence::Ok)` is a
    /// legitimate way to *exempt* a pair the tier rule would otherwise flag.
    ///
    /// The default is `None` — no special cases — which is the whole of a flat
    /// synth's implementation.
    #[inline]
    fn special_case(_src: Self::Source, _dst: Self::Dest) -> Option<Coherence> {
        None
    }
}

/// The coherence verdict for one `source → dest` pair.
///
/// Order of checks, which is load-bearing:
///
/// 1. **Empty slot** — either endpoint the sentinel → [`Coherence::Ok`]. A slot
///    the player has not wired up is not a mistake, and nothing downstream
///    should paint it red.
/// 2. **The synth's special cases** — before the tier rule, so a pair that is
///    both gets the more specific verdict.
/// 3. **The tier rule** — `!src.covers(dst)` → [`Coherence::TierCollapse`].
/// 4. Otherwise coherent.
#[inline]
pub fn coherence<R: CoherenceRoster>(src: R::Source, dst: R::Dest) -> Coherence {
    let (Some(src_tier), Some(dst_tier)) = (R::source_tier(src), R::dest_tier(dst)) else {
        return Coherence::Ok;
    };
    if let Some(verdict) = R::special_case(src, dst) {
        return verdict;
    }
    if !src_tier.covers(dst_tier) {
        return Coherence::TierCollapse;
    }
    Coherence::Ok
}

/// The dense verdict table a faceplate descriptor carries, as
/// `grid[i][j] = coherence(sources[i], dests[j]).name()`.
///
/// Indexing is positional in the slices passed, so handing it each enum's `ALL`
/// — which the generator guarantees is in discriminant order — produces the
/// `[srcWireId][dstWireId]` table both synths' pages index by, sentinel row and
/// column included. The strings rather than the verdicts because the only
/// consumer is a JSON descriptor, and [`Coherence::name`] is the encoding that
/// crosses to it.
///
/// This is a descriptor-time helper — it allocates, and belongs nowhere near a
/// render.
pub fn coherence_name_grid<R: CoherenceRoster>(
    sources: &[R::Source],
    dests: &[R::Dest],
) -> Vec<Vec<&'static str>> {
    sources
        .iter()
        .map(|&src| {
            dests
                .iter()
                .map(|&dst| coherence::<R>(src, dst).name())
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roster::MatrixRoster;
    use crate::test_roster::TestRoster;

    /// [`TestRoster`]'s four sources and four dests, plus a sentinel at 0, as an
    /// enum-shaped id the trait can be keyed on. The fixture's tiers span every
    /// pairing (`PatchGlobal`, `PerStack`, `PerLane`, `PerLane`), so the grid
    /// below covers the whole rule.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    struct Id(Option<u8>);

    const NONE: Id = Id(None);
    const IDS: [Id; 5] = [NONE, Id(Some(0)), Id(Some(1)), Id(Some(2)), Id(Some(3))];

    /// The flat case: tiers straight off the fixture, no special cases. This is
    /// the shape vxn-1b has.
    struct Flat;

    impl CoherenceRoster for Flat {
        type Source = Id;
        type Dest = Id;

        fn source_tier(src: Id) -> Option<Tier> {
            src.0.map(TestRoster::source_tier)
        }

        fn dest_tier(dst: Id) -> Option<Tier> {
            dst.0.map(TestRoster::dest_tier)
        }
    }

    /// The same rosters with two special cases layered on, shaped like vxn-2's:
    /// one that flags a pair the tier rule calls fine (source 0 into dest 0 —
    /// both `PatchGlobal`), and one that flags a pair the tier rule *also*
    /// flags (source 2 into dest 1 — `PerLane` into `PerStack`), which is where
    /// precedence is observable.
    struct Special;

    impl CoherenceRoster for Special {
        type Source = Id;
        type Dest = Id;

        fn source_tier(src: Id) -> Option<Tier> {
            Flat::source_tier(src)
        }

        fn dest_tier(dst: Id) -> Option<Tier> {
            Flat::dest_tier(dst)
        }

        fn special_case(src: Id, dst: Id) -> Option<Coherence> {
            match (src.0?, dst.0?) {
                (0, 0) => Some(Coherence::SelfRate),
                (2, 1) => Some(Coherence::Degenerate),
                _ => None,
            }
        }
    }

    #[test]
    fn the_tier_rule_is_covers_and_nothing_else() {
        for (si, src) in IDS.iter().enumerate().skip(1) {
            for (di, dst) in IDS.iter().enumerate().skip(1) {
                let want = if TestRoster::source_tier(si as u8 - 1)
                    .covers(TestRoster::dest_tier(di as u8 - 1))
                {
                    Coherence::Ok
                } else {
                    Coherence::TierCollapse
                };
                assert_eq!(coherence::<Flat>(*src, *dst), want, "{si}→{di}");
            }
        }
    }

    /// The pairings the fixture exists to cover, spelled out rather than
    /// re-derived: coarse reaches fine, equal reaches equal, fine collapses.
    #[test]
    fn coarse_reaches_fine_and_fine_collapses() {
        // PatchGlobal → PerLane, and PatchGlobal → PatchGlobal.
        assert_eq!(coherence::<Flat>(Id(Some(0)), Id(Some(2))), Coherence::Ok);
        assert_eq!(coherence::<Flat>(Id(Some(0)), Id(Some(0))), Coherence::Ok);
        // PerStack → PerLane is fine; PerStack → PatchGlobal is not.
        assert_eq!(coherence::<Flat>(Id(Some(1)), Id(Some(3))), Coherence::Ok);
        assert_eq!(
            coherence::<Flat>(Id(Some(1)), Id(Some(0))),
            Coherence::TierCollapse
        );
        // PerLane → anything coarser collapses.
        assert_eq!(
            coherence::<Flat>(Id(Some(2)), Id(Some(1))),
            Coherence::TierCollapse
        );
        assert_eq!(coherence::<Flat>(Id(Some(2)), Id(Some(3))), Coherence::Ok);
    }

    /// An empty endpoint short-circuits before any tier is read — including for
    /// the pairs a special case would otherwise claim.
    #[test]
    fn an_empty_endpoint_is_never_flagged() {
        for id in IDS {
            assert_eq!(coherence::<Special>(NONE, id), Coherence::Ok, "none→{id:?}");
            assert_eq!(coherence::<Special>(id, NONE), Coherence::Ok, "{id:?}→none");
        }
    }

    /// The precedence the ordering exists for: a pair that is *both* a special
    /// case and a tier collapse reports the special case, because that is the
    /// more specific thing to tell a player.
    #[test]
    fn a_special_case_outranks_the_tier_rule() {
        // Tier-legal on its own, flagged by the hook.
        assert_eq!(coherence::<Flat>(Id(Some(0)), Id(Some(0))), Coherence::Ok);
        assert_eq!(
            coherence::<Special>(Id(Some(0)), Id(Some(0))),
            Coherence::SelfRate
        );
        // Tier-collapsing on its own, and the hook's verdict wins.
        assert_eq!(
            coherence::<Flat>(Id(Some(2)), Id(Some(1))),
            Coherence::TierCollapse
        );
        assert_eq!(
            coherence::<Special>(Id(Some(2)), Id(Some(1))),
            Coherence::Degenerate
        );
    }

    /// Taking the default `special_case` must change nothing but the special
    /// cases — the flat roster's whole grid is the tier rule.
    #[test]
    fn the_default_hook_leaves_the_tier_rule_alone() {
        for src in IDS {
            for dst in IDS {
                let flat = coherence::<Flat>(src, dst);
                let special = coherence::<Special>(src, dst);
                let is_hooked = matches!((src.0, dst.0), (Some(0), Some(0)) | (Some(2), Some(1)));
                if !is_hooked {
                    assert_eq!(flat, special, "{src:?}→{dst:?}");
                }
            }
        }
    }

    #[test]
    fn names_are_the_wire_contract() {
        assert_eq!(Coherence::Ok.name(), "ok");
        assert_eq!(Coherence::TierCollapse.name(), "tier-collapse");
        assert_eq!(Coherence::SelfRate.name(), "self-rate");
        assert_eq!(Coherence::Degenerate.name(), "degenerate");
        for v in [
            Coherence::TierCollapse,
            Coherence::SelfRate,
            Coherence::Degenerate,
        ] {
            assert!(v.is_flagged(), "{v:?}");
        }
        assert!(!Coherence::Ok.is_flagged());
        assert_eq!(Coherence::default(), Coherence::Ok);
    }

    /// The descriptor's table is positional in the slices handed to it, so a
    /// faceplate indexing it by wire discriminant reads the right cell.
    #[test]
    fn the_name_grid_is_positional_in_the_slices() {
        let grid = coherence_name_grid::<Special>(&IDS, &IDS);
        assert_eq!(grid.len(), IDS.len());
        for row in &grid {
            assert_eq!(row.len(), IDS.len());
        }
        // Sentinel row and column are all "ok".
        assert!(grid[0].iter().all(|v| *v == "ok"));
        assert!(grid.iter().all(|row| row[0] == "ok"));
        // The two hooked pairs, at their positions (+1 for the sentinel).
        assert_eq!(grid[1][1], "self-rate");
        assert_eq!(grid[3][2], "degenerate");
        // And one the tier rule flags on its own: PerLane source 3 → dest 0.
        assert_eq!(grid[4][1], "tier-collapse");
        // Every cell agrees with the predicate.
        for (i, src) in IDS.iter().enumerate() {
            for (j, dst) in IDS.iter().enumerate() {
                assert_eq!(grid[i][j], coherence::<Special>(*src, *dst).name());
            }
        }
    }
}
