// panels/algo-data.js — static algorithm tables for the op-row coordinator
// (split out of op-row.js in ticket 0141).
//
// Source of truth for both tables is `vxn2_dsp::algo::ALGOS`; drift is caught
// by the `algo_data_carriers_match_engine_table` and
// `algo_data_fb_ops_match_engine_table` Rust tests in lib.rs, which parse the
// `ALGO_CARRIERS` / `ALGO_FB_OPS` literals straight out of this file.
//
// Per [ADR 0002] dual-layer voicing is gone — every param id is flat.
(function () {
  window.__vxn = window.__vxn || {};
  window.__vxn.panels = window.__vxn.panels || {};

  // 1-indexed op that carries the algorithm's structural feedback loop, by
  // algo number (1-indexed). Mirrors `AlgoSpec::structural_fb_op` in
  // `vxn2_dsp::algo::ALGOS`.
  const ALGO_FB_OPS = [
    6, 2, 6, 4, 6, 5, 6, 4, 2, 3, 6, 2, 6, 6, 2, 6,
    2, 3, 6, 3, 6, 6, 6, 6, 6, 6, 3, 5, 6, 5, 6, 6,
  ];

  // 32 algos x 6 ops; true if op (1-indexed) is a carrier in algo
  // (1-indexed). Mirrors the carrier mask of `vxn2_dsp::algo::ALGOS`.
  const ALGO_CARRIERS = [
    // 1:  carriers {1,3}
    [true,  false, true,  false, false, false],
    // 2:  carriers {1,3}
    [true,  false, true,  false, false, false],
    // 3:  carriers {1,4}
    [true,  false, false, true,  false, false],
    // 4:  carriers {1,4}
    [true,  false, false, true,  false, false],
    // 5:  carriers {1,3,5}
    [true,  false, true,  false, true,  false],
    // 6:  carriers {1,3,5}
    [true,  false, true,  false, true,  false],
    // 7:  carriers {1,3}
    [true,  false, true,  false, false, false],
    // 8:  carriers {1,3}
    [true,  false, true,  false, false, false],
    // 9:  carriers {1,3}
    [true,  false, true,  false, false, false],
    // 10: carriers {1,4}
    [true,  false, false, true,  false, false],
    // 11: carriers {1,4}
    [true,  false, false, true,  false, false],
    // 12: carriers {1,3}
    [true,  false, true,  false, false, false],
    // 13: carriers {1,3}
    [true,  false, true,  false, false, false],
    // 14: carriers {1,3}
    [true,  false, true,  false, false, false],
    // 15: carriers {1,3}
    [true,  false, true,  false, false, false],
    // 16: carriers {1}
    [true,  false, false, false, false, false],
    // 17: carriers {1}
    [true,  false, false, false, false, false],
    // 18: carriers {1}
    [true,  false, false, false, false, false],
    // 19: carriers {1,4,5}
    [true,  false, false, true,  true,  false],
    // 20: carriers {1,2,4}
    [true,  true,  false, true,  false, false],
    // 21: carriers {1,2,4,5}
    [true,  true,  false, true,  true,  false],
    // 22: carriers {1,3,4,5}
    [true,  false, true,  true,  true,  false],
    // 23: carriers {1,2,4,5}
    [true,  true,  false, true,  true,  false],
    // 24: carriers {1,2,3,4,5}
    [true,  true,  true,  true,  true,  false],
    // 25: carriers {1,2,3,4,5}
    [true,  true,  true,  true,  true,  false],
    // 26: carriers {1,2,4}
    [true,  true,  false, true,  false, false],
    // 27: carriers {1,2,4}
    [true,  true,  false, true,  false, false],
    // 28: carriers {1,3,6}
    [true,  false, true,  false, false, true ],
    // 29: carriers {1,2,3,5}
    [true,  true,  true,  false, true,  false],
    // 30: carriers {1,2,3,6}
    [true,  true,  true,  false, false, true ],
    // 31: carriers {1,2,3,4,5}
    [true,  true,  true,  true,  true,  false],
    // 32: carriers {1,2,3,4,5,6}
    [true,  true,  true,  true,  true,  true ],
  ];

  const OP_PARAMS = {
    num:      { kind: "fader" },
    denom:    { kind: "fader" },
    "fixed-hz":{ kind: "fader" },
    fine:     { kind: "fader" },
    detune:   { kind: "fader" },
    level:    { kind: "fader" },
    "vel-sens":{ kind: "fader" },
    pan:      { kind: "fader" },
    phase:    { kind: "fader" },
    feedback: { kind: "fader" },
    "eg-r1":  { kind: "eg-rate", idx: 0 },
    "eg-r2":  { kind: "eg-rate", idx: 1 },
    "eg-r3":  { kind: "eg-rate", idx: 2 },
    "eg-r4":  { kind: "eg-rate", idx: 3 },
    "eg-l1":  { kind: "eg-level", idx: 0 },
    "eg-l2":  { kind: "eg-level", idx: 1 },
    "eg-l3":  { kind: "eg-level", idx: 2 },
    "eg-l4":  { kind: "eg-level", idx: 3 },
    "ks-break-pt": { kind: "ks-bp" },
    "ks-l-depth":  { kind: "ks-l-depth" },
    "ks-r-depth":  { kind: "ks-r-depth" },
    "ks-rate":     { kind: "ks-rate" },
    "ratio-mode":  { kind: "button-group" },
  };

  function isCarrier(algoNum, op) {
    if (algoNum < 1 || algoNum > 32) return false;
    if (op < 1 || op > 6) return false;
    return ALGO_CARRIERS[algoNum - 1][op - 1];
  }

  window.__vxn.panels.algoData = {
    ALGO_CARRIERS: ALGO_CARRIERS,
    ALGO_FB_OPS: ALGO_FB_OPS,
    OP_PARAMS: OP_PARAMS,
    isCarrier: isCarrier,
  };
})();
