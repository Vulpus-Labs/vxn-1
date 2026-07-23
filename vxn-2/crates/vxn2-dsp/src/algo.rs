//! 32-algorithm FM router.
//!
//! Each VXN2 voice has 6 operators wired together by one of 32 canonical
//! algorithm graphs. This module encodes those graphs and provides a
//! branch-free per-sample router: given the prior-sample outputs of all six
//! ops, returns each op's structural modulation input and the carrier-bus
//! sum for the current sample.
//!
//! Two-stage dispatch: the caller resolves the algorithm number to a
//! specialised function once per block (via [`resolve_route`]), then calls
//! that function per sample. Each per-algo body is straight-line code — only
//! fused prev-output reads + adds — emitted as a distinct symbol so an asm
//! dump can verify each algorithm individually.
//!
//! ## Feedback path (one per algorithm)
//!
//! Each algorithm has exactly **one** feedback path, scaled by the
//! single global feedback control. The path is a
//! `fb_src → fb_dst` op pair: the (two-sample-averaged) prior output of
//! `fb_src` is summed into `fb_dst`'s modulation input.
//!
//! For 30 algorithms `fb_src == fb_dst` — a single-operator self-feedback
//! loop. Algorithms **4** and **6** are the exceptions: their feedback
//! wraps *two* operators, so `fb_src != fb_dst` (algo 4: OP4→OP6, algo 6:
//! OP5→OP6). The [`AlgoSpec::fb_src`] / [`AlgoSpec::fb_dst`] fields encode
//! this pair.
//!
//! The router itself does not apply feedback — it only sums the structural
//! modulator→carrier edges. Feedback injection (reading `fb_src` history,
//! scaling, adding into `fb_dst`'s mod input) happens in the voice / stack
//! tick loop, before [`crate::op::op_tick`], using the same one-sample-delay
//! convention as the router. `fb_scale` (the cooked global feedback amount)
//! is stored on the `fb_src` op.
//!
//! ## Sample-delay convention
//!
//! Routing is via *prev*-sample outputs. Each op reads its modulators'
//! one-sample-delayed values, removing within-sample data dependencies and
//! letting the six ops in a voice run "in parallel" each sample tick
//! (which is what enables SoA voice packing for SIMD). The carrier bus is
//! likewise summed from prev-sample carrier outputs — the audio output is
//! a single sample behind the latest `op_tick`s, which is inaudible.
//!
//! ## Reference
//!
//! Algorithms 1–32 follow the reference algorithm chart. When
//! editing this table, cross-reference the chart — the per-algo carrier
//! set and edge list MUST match it exactly. The [`tests::ping_test`]
//! module validates carrier sets via independent expected-carrier-count and
//! expected-pure-carriers tables; an editing mistake there will fail the
//! tests rather than ship silently.

/// Number of operators per voice. Fixed at 6.
pub const N_OPS: usize = 6;

/// Number of algorithms. Fixed at 32.
pub const N_ALGOS: usize = 32;

/// Maximum modulator→carrier edges any single algorithm carries. The
/// densest algorithm (algo 8 / 9) uses 4 edges; we pad to 5 for slack.
const MAX_EDGES: usize = 5;

/// One algorithm's structural graph.
///
/// `edges` is a list of `(mod, car)` op pairs (1-indexed) representing
/// modulator→carrier wires. Only the first `n_edges` entries are valid.
///
/// `carriers` is a 6-bit mask of carrier ops: bit `i` (0..=5) set means
/// op (i + 1) is a carrier (its output contributes to the audio bus).
///
/// `fb_src` / `fb_dst` are the 1-indexed endpoints of the algorithm's single
/// feedback path: `fb_src`'s prior output feeds back into `fb_dst`'s
/// modulation input, scaled by the global feedback control. For the 30
/// self-feedback algorithms `fb_src == fb_dst`; algorithms 4 and 6 have a
/// two-operator loop (`fb_src != fb_dst`). The cooked `fb_scale` lives on the
/// `fb_src` op; injection happens in the voice/stack tick loop (see module
/// docs), not in the router.
#[derive(Clone, Copy, Debug)]
pub struct AlgoSpec {
    pub edges: [(u8, u8); MAX_EDGES],
    pub n_edges: u8,
    pub carriers: u8,
    pub fb_src: u8,
    pub fb_dst: u8,
}

const fn carrier_mask(carriers: &[u8]) -> u8 {
    let mut mask = 0u8;
    let mut i = 0;
    while i < carriers.len() {
        mask |= 1u8 << (carriers[i] - 1);
        i += 1;
    }
    mask
}

const fn edge_buf(src: &[(u8, u8)]) -> [(u8, u8); MAX_EDGES] {
    let mut out = [(0u8, 0u8); MAX_EDGES];
    let mut i = 0;
    while i < src.len() {
        out[i] = src[i];
        i += 1;
    }
    out
}

/// Self-feedback algorithm (`fb_src == fb_dst == fb`): the common case.
const fn spec(edges: &[(u8, u8)], carriers: &[u8], fb: u8) -> AlgoSpec {
    spec_fb(edges, carriers, fb, fb)
}

/// Two-operator feedback (`fb_src → fb_dst`): algorithms 4 and 6 only.
const fn spec_fb(edges: &[(u8, u8)], carriers: &[u8], fb_src: u8, fb_dst: u8) -> AlgoSpec {
    AlgoSpec {
        edges: edge_buf(edges),
        n_edges: edges.len() as u8,
        carriers: carrier_mask(carriers),
        fb_src,
        fb_dst,
    }
}

/// The canonical 32 algorithm graphs.
///
/// Indexed 0..=31 for algo numbers 1..=32. Cross-reference each entry with
/// the reference algorithm chart before editing.
pub const ALGOS: [AlgoSpec; N_ALGOS] = [
    // 1: stacks (6→5→4→3) + (2→1), fb op6, carriers {1,3}
    spec(&[(2, 1), (4, 3), (5, 4), (6, 5)], &[1, 3], 6),
    // 2: same edges as 1, fb op2
    spec(&[(2, 1), (4, 3), (5, 4), (6, 5)], &[1, 3], 2),
    // 3: stacks (6→5→4) + (3→2→1), fb op6, carriers {1,4}
    spec(&[(2, 1), (3, 2), (5, 4), (6, 5)], &[1, 4], 6),
    // 4: same edges as 3; two-op feedback OP4→OP6 (loop wraps ops 4,5,6)
    spec_fb(&[(2, 1), (3, 2), (5, 4), (6, 5)], &[1, 4], 4, 6),
    // 5: three 2-stacks (6→5),(4→3),(2→1), fb op6, carriers {1,3,5}
    spec(&[(2, 1), (4, 3), (6, 5)], &[1, 3, 5], 6),
    // 6: same edges as 5; two-op feedback OP5→OP6 (loop wraps ops 5,6)
    spec_fb(&[(2, 1), (4, 3), (6, 5)], &[1, 3, 5], 5, 6),
    // 7: (2→1) + (4→3, 5→3 with 6→5), fb op6, carriers {1,3}
    spec(&[(2, 1), (4, 3), (5, 3), (6, 5)], &[1, 3], 6),
    // 8: same edges as 7, fb op4
    spec(&[(2, 1), (4, 3), (5, 3), (6, 5)], &[1, 3], 4),
    // 9: same edges as 7/8, fb op2
    spec(&[(2, 1), (4, 3), (5, 3), (6, 5)], &[1, 3], 2),
    // 10: (3→2→1) + (5→4, 6→4 parallel), fb op3, carriers {1,4}
    spec(&[(2, 1), (3, 2), (5, 4), (6, 4)], &[1, 4], 3),
    // 11: (3→2→1) + (5→4, 6→4 parallel), fb op6, carriers {1,4}
    spec(&[(2, 1), (3, 2), (5, 4), (6, 4)], &[1, 4], 6),
    // 12: (2→1) + (4→3, 5→3, 6→3 parallel), fb op2, carriers {1,3}
    spec(&[(2, 1), (4, 3), (5, 3), (6, 3)], &[1, 3], 2),
    // 13: same edges as 12, fb op6
    spec(&[(2, 1), (4, 3), (5, 3), (6, 3)], &[1, 3], 6),
    // 14: (2→1) + (4→3, 5→4, 6→4 — op4 modded by 5+6 parallel), fb op6, carriers {1,3}
    spec(&[(2, 1), (4, 3), (5, 4), (6, 4)], &[1, 3], 6),
    // 15: same edges as 14, fb op2
    spec(&[(2, 1), (4, 3), (5, 4), (6, 4)], &[1, 3], 2),
    // 16: op1 sole carrier; modded by op2 and op3; op3 modded by op4 and op5; op5 modded by op6 (fb op6)
    spec(&[(2, 1), (3, 1), (4, 3), (5, 3), (6, 5)], &[1], 6),
    // 17: same edges as 16, fb op2
    spec(&[(2, 1), (3, 1), (4, 3), (5, 3), (6, 5)], &[1], 2),
    // 18: op1 sole carrier; op2→op1, op3→op1, op4→op3, op5→op4, op6→op4 (parallel into 4), fb op3
    spec(&[(2, 1), (3, 1), (4, 3), (5, 4), (6, 4)], &[1], 3),
    // 19: carriers {1,4,5}; (3→2→1) + (6→4, 6→5 — op6 mods 4 and 5), fb op6
    spec(&[(2, 1), (3, 2), (6, 4), (6, 5)], &[1, 4, 5], 6),
    // 20: carriers {1,2,4}; (3→1, 3→2 — op3 mods 1 and 2) + (5→4, 6→4 parallel), fb op3
    spec(&[(3, 1), (3, 2), (5, 4), (6, 4)], &[1, 2, 4], 3),
    // 21: carriers {1,2,4,5}; (3→1, 3→2) + (6→4, 6→5), fb op6
    spec(&[(3, 1), (3, 2), (6, 4), (6, 5)], &[1, 2, 4, 5], 6),
    // 22: carriers {1,3,4,5}; (2→1) + (6→3, 6→4, 6→5 — op6 fans out to 3 carriers), fb op6
    spec(&[(2, 1), (6, 3), (6, 4), (6, 5)], &[1, 3, 4, 5], 6),
    // 23: carriers {1,2,4,5}; op1 pure carrier; (3→2) + (6→4, 6→5), fb op6
    spec(&[(3, 2), (6, 4), (6, 5)], &[1, 2, 4, 5], 6),
    // 24: carriers {1,2,3,4,5}; op1,2 pure; (6→3, 6→4, 6→5), fb op6
    spec(&[(6, 3), (6, 4), (6, 5)], &[1, 2, 3, 4, 5], 6),
    // 25: carriers {1,2,3,4,5}; op1,2,3 pure; (6→4, 6→5), fb op6
    spec(&[(6, 4), (6, 5)], &[1, 2, 3, 4, 5], 6),
    // 26: carriers {1,2,4}; op1 pure; (3→2) + (5→4, 6→4 parallel), fb op6
    spec(&[(3, 2), (5, 4), (6, 4)], &[1, 2, 4], 6),
    // 27: same edges as 26, fb op3
    spec(&[(3, 2), (5, 4), (6, 4)], &[1, 2, 4], 3),
    // 28: (2→1) + (5→4→3), op6 pure carrier, fb op5, carriers {1,3,6}
    spec(&[(2, 1), (4, 3), (5, 4)], &[1, 3, 6], 5),
    // 29: carriers {1,2,3,5}; op1,2 pure; (4→3) + (6→5), fb op6
    spec(&[(4, 3), (6, 5)], &[1, 2, 3, 5], 6),
    // 30: carriers {1,2,3,6}; op1,2 pure; (4→3) + (5→4) — op5 mods op4 mods op3; op6 pure carrier; fb op5
    spec(&[(4, 3), (5, 4)], &[1, 2, 3, 6], 5),
    // 31: carriers {1,2,3,4,5}; op1..4 pure carriers; (6→5), fb op6
    spec(&[(6, 5)], &[1, 2, 3, 4, 5], 6),
    // 32: all six ops are carriers; no inter-op edges; fb op6
    spec(&[], &[1, 2, 3, 4, 5, 6], 6),
];

/// Signature of a per-algorithm specialised router. Takes the previous
/// sample's six op outputs by reference, returns each op's modulation
/// input and the carrier bus sum.
pub type RouteFn = fn(prev: &[f32; N_OPS]) -> ([f32; N_OPS], f32);

/// Generate one specialised route function. The body is straight-line
/// indexing — LLVM emits a sequence of loads + FMAs + a store, no branches
/// inside the algorithm. `#[inline(never)]` so each algorithm appears as a
/// distinct symbol in an asm dump.
macro_rules! impl_route {
    (
        $name:ident,
        edges = [$(($m:literal, $c:literal)),* $(,)?],
        carriers = [$($cs:literal),* $(,)?]
    ) => {
        #[inline(never)]
        #[allow(unused_mut)] // algo 32 has no edges; mi remains zero
        fn $name(prev: &[f32; N_OPS]) -> ([f32; N_OPS], f32) {
            let mut mi = [0.0_f32; N_OPS];
            $( mi[$c - 1] += prev[$m - 1]; )*
            let carrier_sum = 0.0_f32 $( + prev[$cs - 1] )*;
            (mi, carrier_sum)
        }
    };
}

impl_route!(route_algo_1,  edges = [(2,1),(4,3),(5,4),(6,5)],          carriers = [1,3]);
impl_route!(route_algo_2,  edges = [(2,1),(4,3),(5,4),(6,5)],          carriers = [1,3]);
impl_route!(route_algo_3,  edges = [(2,1),(3,2),(5,4),(6,5)],          carriers = [1,4]);
impl_route!(route_algo_4,  edges = [(2,1),(3,2),(5,4),(6,5)],          carriers = [1,4]);
impl_route!(route_algo_5,  edges = [(2,1),(4,3),(6,5)],                carriers = [1,3,5]);
impl_route!(route_algo_6,  edges = [(2,1),(4,3),(6,5)],                carriers = [1,3,5]);
impl_route!(route_algo_7,  edges = [(2,1),(4,3),(5,3),(6,5)],          carriers = [1,3]);
impl_route!(route_algo_8,  edges = [(2,1),(4,3),(5,3),(6,5)],          carriers = [1,3]);
impl_route!(route_algo_9,  edges = [(2,1),(4,3),(5,3),(6,5)],          carriers = [1,3]);
impl_route!(route_algo_10, edges = [(2,1),(3,2),(5,4),(6,4)],          carriers = [1,4]);
impl_route!(route_algo_11, edges = [(2,1),(3,2),(5,4),(6,4)],          carriers = [1,4]);
impl_route!(route_algo_12, edges = [(2,1),(4,3),(5,3),(6,3)],          carriers = [1,3]);
impl_route!(route_algo_13, edges = [(2,1),(4,3),(5,3),(6,3)],          carriers = [1,3]);
impl_route!(route_algo_14, edges = [(2,1),(4,3),(5,4),(6,4)],          carriers = [1,3]);
impl_route!(route_algo_15, edges = [(2,1),(4,3),(5,4),(6,4)],          carriers = [1,3]);
impl_route!(route_algo_16, edges = [(2,1),(3,1),(4,3),(5,3),(6,5)],    carriers = [1]);
impl_route!(route_algo_17, edges = [(2,1),(3,1),(4,3),(5,3),(6,5)],    carriers = [1]);
impl_route!(route_algo_18, edges = [(2,1),(3,1),(4,3),(5,4),(6,4)],    carriers = [1]);
impl_route!(route_algo_19, edges = [(2,1),(3,2),(6,4),(6,5)],          carriers = [1,4,5]);
impl_route!(route_algo_20, edges = [(3,1),(3,2),(5,4),(6,4)],          carriers = [1,2,4]);
impl_route!(route_algo_21, edges = [(3,1),(3,2),(6,4),(6,5)],          carriers = [1,2,4,5]);
impl_route!(route_algo_22, edges = [(2,1),(6,3),(6,4),(6,5)],          carriers = [1,3,4,5]);
impl_route!(route_algo_23, edges = [(3,2),(6,4),(6,5)],                carriers = [1,2,4,5]);
impl_route!(route_algo_24, edges = [(6,3),(6,4),(6,5)],                carriers = [1,2,3,4,5]);
impl_route!(route_algo_25, edges = [(6,4),(6,5)],                      carriers = [1,2,3,4,5]);
impl_route!(route_algo_26, edges = [(3,2),(5,4),(6,4)],                carriers = [1,2,4]);
impl_route!(route_algo_27, edges = [(3,2),(5,4),(6,4)],                carriers = [1,2,4]);
impl_route!(route_algo_28, edges = [(2,1),(4,3),(5,4)],                carriers = [1,3,6]);
impl_route!(route_algo_29, edges = [(4,3),(6,5)],                      carriers = [1,2,3,5]);
impl_route!(route_algo_30, edges = [(4,3),(5,4)],                      carriers = [1,2,3,6]);
impl_route!(route_algo_31, edges = [(6,5)],                            carriers = [1,2,3,4,5]);
impl_route!(route_algo_32, edges = [],                                 carriers = [1,2,3,4,5,6]);

/// Per-algorithm specialised route functions, indexed 0..=31 for algos
/// 1..=32. Caller resolves once per block via [`resolve_route`] and calls
/// the returned `fn` per sample — the algorithm match is then hoisted out
/// of the inner sample loop.
pub static ROUTE_FNS: [RouteFn; N_ALGOS] = [
    route_algo_1,  route_algo_2,  route_algo_3,  route_algo_4,
    route_algo_5,  route_algo_6,  route_algo_7,  route_algo_8,
    route_algo_9,  route_algo_10, route_algo_11, route_algo_12,
    route_algo_13, route_algo_14, route_algo_15, route_algo_16,
    route_algo_17, route_algo_18, route_algo_19, route_algo_20,
    route_algo_21, route_algo_22, route_algo_23, route_algo_24,
    route_algo_25, route_algo_26, route_algo_27, route_algo_28,
    route_algo_29, route_algo_30, route_algo_31, route_algo_32,
];

/// Resolve `algo` (1..=32) to its specialised router. Use this once per
/// block, then call the returned `fn` per sample inside the voice loop.
/// Inputs outside 1..=32 saturate to algo 1.
#[inline]
pub fn resolve_route(algo: u8) -> RouteFn {
    let idx = (algo.clamp(1, N_ALGOS as u8) - 1) as usize;
    ROUTE_FNS[idx]
}

/// One-shot convenience: dispatch and call in one go. Equivalent to
/// `resolve_route(algo)(prev)`. Block-level use should prefer
/// [`resolve_route`] to avoid a dispatch per sample.
#[inline]
pub fn route(algo: u8, prev: &[f32; N_OPS]) -> ([f32; N_OPS], f32) {
    resolve_route(algo)(prev)
}

/// Algorithm metadata accessor (for UI / diagram code).
#[inline]
pub fn spec_of(algo: u8) -> &'static AlgoSpec {
    let idx = (algo.clamp(1, N_ALGOS as u8) - 1) as usize;
    &ALGOS[idx]
}

/// Resolve the **pitch-coherent component** of `target_op` in algorithm
/// `algo`'s modulation graph: every op reachable from `target_op` when
/// `edges` are treated as **undirected** (modulation propagates pitch both
/// up to modulators and down to carriers), excluding walls.
///
/// `wall_mask` is a 6-bit op mask; bit `i` set means op `(i + 1)` is a wall
/// (fixed-frequency). A wall does not track key, so tuning coherence stops
/// there: it is removed as a graph *node* — excluded from the result AND
/// traversal cannot cross it. A fixed op mid-chain therefore splits the
/// graph into independent components.
///
/// Returns a 6-bit op mask (bit `i` set = op `(i + 1)` is in the component).
/// If `target_op` is itself a wall (or out of range), returns `0` — the
/// stack route no-ops.
///
/// The feedback path (`AlgoSpec::fb_src` / `fb_dst`) is not a modulation edge
/// and is irrelevant to connectivity; it is not consulted here.
///
/// Pure, allocation-free, `const` integer graph walk — safe from the cook
/// path. `algo` is 1-indexed (out-of-range saturates to algo 1); `target_op`
/// is 1-indexed (1..=6).
pub const fn pitch_stack_component(algo: u8, wall_mask: u8, target_op: u8) -> u8 {
    // Out-of-range or walled target → empty component (route no-ops).
    if target_op < 1 || target_op > N_OPS as u8 {
        return 0;
    }
    let target_bit = 1u8 << (target_op - 1);
    if wall_mask & target_bit != 0 {
        return 0;
    }

    // Saturate out-of-range algo IDs to algo 1, matching `resolve_route`.
    let idx = if algo >= 1 && algo <= N_ALGOS as u8 {
        (algo - 1) as usize
    } else {
        0
    };
    let spec = &ALGOS[idx];

    // Flood-fill from the target over surviving (non-wall) nodes. Each round
    // propagates one hop; N_OPS rounds reach a 6-node fixpoint (longest
    // possible chain is 5 hops). Undirected: an edge whose endpoints are both
    // non-wall pulls both endpoints in if either is already visited.
    let mut visited = target_bit;
    let mut round = 0;
    while round < N_OPS {
        let mut e = 0;
        while e < spec.n_edges as usize {
            let (m, c) = spec.edges[e];
            let mbit = 1u8 << (m - 1);
            let cbit = 1u8 << (c - 1);
            // An edge touching a wall is severed — cannot cross it.
            if wall_mask & mbit == 0
                && wall_mask & cbit == 0
                && (visited & mbit != 0 || visited & cbit != 0)
            {
                visited |= mbit | cbit;
            }
            e += 1;
        }
        round += 1;
    }
    visited
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent table: count of carriers per algorithm, sourced from the
    /// reference algorithm chart. If the [`ALGOS`] table is mis-encoded, this
    /// test catches the carrier-count discrepancy.
    const EXPECTED_CARRIER_COUNT: [u8; N_ALGOS] = [
        2, 2, 2, 2, 3, 3, 2, 2, 2, 2, // 1..=10
        2, 2, 2, 2, 2, 1, 1, 1, 3, 3, // 11..=20
        4, 4, 4, 5, 5, 3, 3, 3, 4, 4, // 21..=30
        5, 6, // 31..=32
    ];

    #[test]
    fn carrier_counts_match_reference_chart() {
        for (i, spec) in ALGOS.iter().enumerate() {
            let got = spec.carriers.count_ones() as u8;
            let want = EXPECTED_CARRIER_COUNT[i];
            assert_eq!(
                got,
                want,
                "algo {}: carrier-count mismatch (got {}, expected {})",
                i + 1,
                got,
                want
            );
        }
    }

    #[test]
    fn fb_endpoints_are_valid() {
        for (i, spec) in ALGOS.iter().enumerate() {
            assert!(
                (1..=N_OPS as u8).contains(&spec.fb_src),
                "algo {}: fb_src {} out of range",
                i + 1,
                spec.fb_src
            );
            assert!(
                (1..=N_OPS as u8).contains(&spec.fb_dst),
                "algo {}: fb_dst {} out of range",
                i + 1,
                spec.fb_dst
            );
        }
    }

    /// Exactly two algorithms (4 and 6) have a two-operator feedback loop
    /// (`fb_src != fb_dst`); all other 30 are single-op self-feedback. Guards
    /// against a data-entry slip re-introducing the self-feedback-only
    /// encoding for algos 4/6.
    #[test]
    fn only_algos_4_and_6_have_two_op_feedback() {
        for (i, spec) in ALGOS.iter().enumerate() {
            let algo = i + 1;
            let two_op = spec.fb_src != spec.fb_dst;
            match algo {
                4 => assert_eq!((spec.fb_src, spec.fb_dst), (4, 6), "algo 4 feedback"),
                6 => assert_eq!((spec.fb_src, spec.fb_dst), (5, 6), "algo 6 feedback"),
                _ => assert!(!two_op, "algo {algo}: unexpected two-op feedback"),
            }
        }
    }

    #[test]
    fn edges_well_formed() {
        for (i, spec) in ALGOS.iter().enumerate() {
            for e in 0..spec.n_edges as usize {
                let (m, c) = spec.edges[e];
                assert!(
                    (1..=N_OPS as u8).contains(&m) && (1..=N_OPS as u8).contains(&c),
                    "algo {}: edge {} ({}, {}) out of range",
                    i + 1,
                    e,
                    m,
                    c
                );
                assert_ne!(
                    m,
                    c,
                    "algo {}: edge {} is a self-loop (use per-op feedback instead)",
                    i + 1,
                    e
                );
            }
        }
    }

    /// Ping test.
    ///
    /// For each algorithm N and each op K, set prev_outputs to a "ping"
    /// (op K at 1.0, all others 0.0). The carrier-sum returned by the
    /// router must be nonzero iff op K is a carrier in that algorithm.
    /// This validates the algorithm table's carrier mask end-to-end through
    /// the dispatch path.
    #[test]
    fn ping_test_carrier_audibility() {
        for algo in 1..=N_ALGOS as u8 {
            let route_fn = resolve_route(algo);
            let spec = spec_of(algo);
            for op in 1..=N_OPS as u8 {
                let mut prev = [0.0_f32; N_OPS];
                prev[op as usize - 1] = 1.0;
                let (_mi, carrier_sum) = route_fn(&prev);
                let is_carrier = (spec.carriers >> (op - 1)) & 1 == 1;
                if is_carrier {
                    assert!(
                        carrier_sum > 0.0,
                        "algo {}: ping op {} (a carrier) produced silent bus",
                        algo,
                        op
                    );
                } else {
                    assert_eq!(
                        carrier_sum, 0.0,
                        "algo {}: ping op {} (a modulator) leaked into bus",
                        algo, op
                    );
                }
            }
        }
    }

    /// Modulation-routing ping: a ping on op K propagates only to ops K
    /// modulates per the algo's edge list. Validates the modulation_in
    /// output of the router (the edge encoding, not just the carrier mask).
    #[test]
    fn ping_test_modulation_routing() {
        for algo in 1..=N_ALGOS as u8 {
            let route_fn = resolve_route(algo);
            let spec = spec_of(algo);
            for op in 1..=N_OPS as u8 {
                let mut prev = [0.0_f32; N_OPS];
                prev[op as usize - 1] = 1.0;
                let (mi, _) = route_fn(&prev);
                for car in 1..=N_OPS as u8 {
                    let want_edge = (0..spec.n_edges as usize)
                        .any(|e| spec.edges[e] == (op, car));
                    let got_nonzero = mi[car as usize - 1] > 0.0;
                    assert_eq!(
                        want_edge,
                        got_nonzero,
                        "algo {}: op {}→{} edge mismatch (spec says {}, router emitted {})",
                        algo,
                        op,
                        car,
                        want_edge,
                        got_nonzero
                    );
                }
            }
        }
    }

    /// Zero-input zero-output: silence in, silence out, every algorithm.
    #[test]
    fn silence_in_silence_out() {
        for algo in 1..=N_ALGOS as u8 {
            let (mi, cs) = route(algo, &[0.0; N_OPS]);
            assert_eq!(cs, 0.0);
            for v in mi {
                assert_eq!(v, 0.0);
            }
        }
    }

    /// Multi-op ping: when several modulators feed one carrier in parallel
    /// (e.g. algo 12's op3 ← op4 + op5 + op6), the carrier's mod_in is the
    /// sum of those modulator outputs.
    #[test]
    fn algo_12_parallel_modulators_into_op3() {
        // Algo 12: edges include (4,3),(5,3),(6,3). Set ops 4,5,6 to
        // distinct values and verify mod_in[2] (= op3's input) is the sum.
        let prev = [0.0, 0.0, 0.0, 0.25, 0.5, 1.0];
        let (mi, _) = route(12, &prev);
        assert!((mi[2] - (0.25 + 0.5 + 1.0)).abs() < 1e-6);
    }

    /// Algo 32 (six parallel carriers, no edges): mod_in is all zero
    /// regardless of prev_outputs; carrier_sum is sum of all ops.
    #[test]
    fn algo_32_all_carriers_no_edges() {
        let prev = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6];
        let (mi, cs) = route(32, &prev);
        for v in mi {
            assert_eq!(v, 0.0);
        }
        assert!((cs - 2.1).abs() < 1e-6);
    }

    /// `resolve_route` saturates rather than panicking for out-of-range
    /// algo IDs. (Caller still owes a valid algo; this is a belt-and-braces
    /// guard against u8 wraparound bugs upstream.)
    #[test]
    fn resolve_route_clamps() {
        // algo 0 and algo 255 both saturate to algo 1 — they must resolve to
        // the exact same route function.  algo 33 saturates to algo 32.
        // Mirroring `component_range_guards` which tests the same saturation
        // contract for `pitch_stack_component`.
        let algo1 = resolve_route(1);
        let algo32 = resolve_route(32);
        assert_eq!(
            resolve_route(0) as usize, algo1 as usize,
            "algo 0 must clamp to algo 1's route"
        );
        assert_eq!(
            resolve_route(33) as usize, algo32 as usize,
            "algo 33 must clamp to algo 32's route"
        );
        assert_eq!(
            resolve_route(255) as usize, algo32 as usize,
            "algo 255 must clamp to algo 32's route"
        );
    }

    /// 1-indexed op list → 6-bit mask, for readable expected values.
    fn mask(ops: &[u8]) -> u8 {
        ops.iter().fold(0u8, |m, &op| m | (1 << (op - 1)))
    }

    /// Linear stack: algo 1's chart is (6→5→4→3) + (2→1), carriers {1,3}.
    /// The whole chain feeding carrier 3 is one component; carrier 1's pair
    /// is independent.
    #[test]
    fn component_linear_stack() {
        // Target the carrier of the 4-op chain.
        assert_eq!(pitch_stack_component(1, 0, 3), mask(&[3, 4, 5, 6]));
        // Targeting any mid-chain op gives the same component (undirected).
        assert_eq!(pitch_stack_component(1, 0, 5), mask(&[3, 4, 5, 6]));
        // The (2→1) pair is a separate component.
        assert_eq!(pitch_stack_component(1, 0, 1), mask(&[1, 2]));
        assert_eq!(pitch_stack_component(1, 0, 2), mask(&[1, 2]));
    }

    /// Shared modulator: algo 22 chart is (2→1) + op6 fanning to carriers
    /// {3,4,5}. Targeting the shared modulator (or any of its carriers)
    /// pulls the whole fan-out into one component — large by design.
    #[test]
    fn component_shared_modulator_spread() {
        let fan = mask(&[3, 4, 5, 6]);
        assert_eq!(pitch_stack_component(22, 0, 6), fan); // the shared mod
        assert_eq!(pitch_stack_component(22, 0, 3), fan); // via op6
        assert_eq!(pitch_stack_component(22, 0, 4), fan);
        // op1/op2 stay independent of the fan-out.
        assert_eq!(pitch_stack_component(22, 0, 1), mask(&[1, 2]));
    }

    /// A wall mid-chain severs the graph. Algo 1 chain (6→5→4→3) with op5
    /// fixed: op3 reaches op4 but not across op5; op6 is isolated past it.
    #[test]
    fn component_wall_splits_chain() {
        let wall5 = mask(&[5]);
        assert_eq!(pitch_stack_component(1, wall5, 3), mask(&[3, 4]));
        assert_eq!(pitch_stack_component(1, wall5, 6), mask(&[6]));
        // The unrelated (2→1) pair is untouched by the wall.
        assert_eq!(pitch_stack_component(1, wall5, 1), mask(&[1, 2]));
    }

    /// A walled target itself → empty component (route no-ops).
    #[test]
    fn component_walled_target_empty() {
        assert_eq!(pitch_stack_component(1, mask(&[3]), 3), 0);
        assert_eq!(pitch_stack_component(22, mask(&[6]), 6), 0);
    }

    /// An op with no surviving edges → just itself. Algo 32 has no edges, so
    /// every op is isolated; a walled neighbour likewise isolates.
    #[test]
    fn component_isolated_op_is_self() {
        for op in 1..=N_OPS as u8 {
            assert_eq!(pitch_stack_component(32, 0, op), mask(&[op]));
        }
        // Algo 1 op1 with its sole partner (op2) walled → {1}.
        assert_eq!(pitch_stack_component(1, mask(&[2]), 1), mask(&[1]));
    }

    /// The component never includes a wall, and is always a subset of the
    /// non-wall ops, for every algo / target / single-wall combination.
    #[test]
    fn component_excludes_walls_exhaustive() {
        for algo in 1..=N_ALGOS as u8 {
            for wall in 1..=N_OPS as u8 {
                let wm = mask(&[wall]);
                for target in 1..=N_OPS as u8 {
                    let comp = pitch_stack_component(algo, wm, target);
                    assert_eq!(comp & wm, 0, "algo {algo}: wall {wall} leaked");
                    if target == wall {
                        assert_eq!(comp, 0, "algo {algo}: walled target {target}");
                    } else {
                        assert!(comp & mask(&[target]) != 0,
                            "algo {algo}: target {target} missing from own component");
                    }
                }
            }
        }
    }

    /// Out-of-range target → empty; out-of-range algo saturates to algo 1.
    #[test]
    fn component_range_guards() {
        assert_eq!(pitch_stack_component(1, 0, 0), 0);
        assert_eq!(pitch_stack_component(1, 0, 7), 0);
        assert_eq!(pitch_stack_component(0, 0, 3), pitch_stack_component(1, 0, 3));
        assert_eq!(pitch_stack_component(99, 0, 3), pitch_stack_component(1, 0, 3));
    }
}
