// VXN2 read-only algorithm-graph renderer.
//
// Layout port of `ui-mockup/index.html` renderAlgoDiagram /
// buildMiniAlgoSvg — DX7 algorithm graph: ops as boxes (carriers
// orange, modulators blue), edges modulator → carrier, feedback loop
// over the algo's FB op, sink arrows from carriers.
//
// 0026 lands the primitive only — bound by 0027 (op-row badge +
// algorithm picker overlay). Knowledge of the 32 algo topologies lives
// here as a compact table.

(function () {
  // Full 32-entry DX7 algorithm table. Each entry:
  //   edges: [modOp, carOp] pairs (modulator → carrier).
  //   carriers: ops whose output sums into the algo's bus.
  //   fb: source op of the algorithm's single feedback path (0 = none).
  //   fbTo: dest op the feedback lands on. Defaults to `fb` (self-loop) when
  //         absent; only algos 4 (OP4→OP6) and 6 (OP5→OP6) differ — the DX7
  //         two-operator feedback loops.
  // Sourced from `vxn2-dsp::algo::ALGOS` — keep this in lock-step with
  // that table; the Rust side is authoritative.
  const ALGOS = {
    1:  { edges: [[2,1],[4,3],[5,4],[6,5]],          carriers: [1, 3],          fb: 6 },
    2:  { edges: [[2,1],[4,3],[5,4],[6,5]],          carriers: [1, 3],          fb: 2 },
    3:  { edges: [[2,1],[3,2],[5,4],[6,5]],          carriers: [1, 4],          fb: 6 },
    4:  { edges: [[2,1],[3,2],[5,4],[6,5]],          carriers: [1, 4],          fb: 4, fbTo: 6 },
    5:  { edges: [[2,1],[4,3],[6,5]],                carriers: [1, 3, 5],       fb: 6 },
    6:  { edges: [[2,1],[4,3],[6,5]],                carriers: [1, 3, 5],       fb: 5, fbTo: 6 },
    7:  { edges: [[2,1],[4,3],[5,3],[6,5]],          carriers: [1, 3],          fb: 6 },
    8:  { edges: [[2,1],[4,3],[5,3],[6,5]],          carriers: [1, 3],          fb: 4 },
    9:  { edges: [[2,1],[4,3],[5,3],[6,5]],          carriers: [1, 3],          fb: 2 },
    10: { edges: [[2,1],[3,2],[5,4],[6,4]],          carriers: [1, 4],          fb: 3 },
    11: { edges: [[2,1],[3,2],[5,4],[6,4]],          carriers: [1, 4],          fb: 6 },
    12: { edges: [[2,1],[4,3],[5,3],[6,3]],          carriers: [1, 3],          fb: 2 },
    13: { edges: [[2,1],[4,3],[5,3],[6,3]],          carriers: [1, 3],          fb: 6 },
    14: { edges: [[2,1],[4,3],[5,4],[6,4]],          carriers: [1, 3],          fb: 6 },
    15: { edges: [[2,1],[4,3],[5,4],[6,4]],          carriers: [1, 3],          fb: 2 },
    16: { edges: [[2,1],[3,1],[4,3],[5,3],[6,5]],    carriers: [1],             fb: 6 },
    17: { edges: [[2,1],[3,1],[4,3],[5,3],[6,5]],    carriers: [1],             fb: 2 },
    18: { edges: [[2,1],[3,1],[4,3],[5,4],[6,4]],    carriers: [1],             fb: 3 },
    19: { edges: [[2,1],[3,2],[6,4],[6,5]],          carriers: [1, 4, 5],       fb: 6 },
    20: { edges: [[3,1],[3,2],[5,4],[6,4]],          carriers: [1, 2, 4],       fb: 3 },
    21: { edges: [[3,1],[3,2],[6,4],[6,5]],          carriers: [1, 2, 4, 5],    fb: 6 },
    22: { edges: [[2,1],[6,3],[6,4],[6,5]],          carriers: [1, 3, 4, 5],    fb: 6 },
    23: { edges: [[3,2],[6,4],[6,5]],                carriers: [1, 2, 4, 5],    fb: 6 },
    24: { edges: [[6,3],[6,4],[6,5]],                carriers: [1, 2, 3, 4, 5], fb: 6 },
    25: { edges: [[6,4],[6,5]],                      carriers: [1, 2, 3, 4, 5], fb: 6 },
    26: { edges: [[3,2],[5,4],[6,4]],                carriers: [1, 2, 4],       fb: 6 },
    27: { edges: [[3,2],[5,4],[6,4]],                carriers: [1, 2, 4],       fb: 3 },
    28: { edges: [[2,1],[4,3],[5,4]],                carriers: [1, 3, 6],       fb: 5 },
    29: { edges: [[4,3],[6,5]],                      carriers: [1, 2, 3, 5],    fb: 6 },
    30: { edges: [[4,3],[5,4]],                      carriers: [1, 2, 3, 6],    fb: 5 },
    31: { edges: [[6,5]],                            carriers: [1, 2, 3, 4, 5], fb: 6 },
    32: { edges: [],                                 carriers: [1, 2, 3, 4, 5, 6], fb: 6 },
  };

  function isCarrier(op, algo) {
    return algo.carriers.indexOf(op) >= 0;
  }

  function computeDepths(algo) {
    const depth = Object.create(null);
    algo.carriers.forEach(function (c) { depth[c] = 0; });
    for (let it = 0; it < 8; it++) {
      algo.edges.forEach(function (e) {
        const m = e[0], c = e[1];
        if (depth[c] !== undefined) {
          const d = depth[c] + 1;
          if (depth[m] === undefined || depth[m] < d) depth[m] = d;
        }
      });
    }
    return depth;
  }

  function layout(W, H, algo, vMargin) {
    const depth = computeDepths(algo);
    const byDepth = Object.create(null);
    for (let op = 1; op <= 6; op++) {
      const d = depth[op] === undefined ? 0 : depth[op];
      if (!byDepth[d]) byDepth[d] = [];
      byDepth[d].push(op);
    }
    const depths = Object.keys(byDepth).map(Number).sort(function (a, b) { return a - b; });
    const maxD = depths.length ? depths[depths.length - 1] : 0;
    const rowH = (H - vMargin * 2) / (maxD + 1);
    const positions = Object.create(null);
    depths.forEach(function (d) {
      const ops = byDepth[d];
      const y = H - vMargin - d * rowH - rowH / 2;
      const slotW = W / (ops.length + 1);
      ops.forEach(function (op, i) {
        positions[op] = { x: slotW * (i + 1), y: y };
      });
    });
    return positions;
  }

  function renderFull(svg, algoNum, selectedOp) {
    const algo = ALGOS[algoNum];
    if (!algo) {
      svg.innerHTML = '<text x="100" y="110" fill="#444" text-anchor="middle" font-size="12">algo ' + algoNum + '</text>';
      return;
    }
    const W = 200, H = 220;
    svg.setAttribute("viewBox", "0 0 " + W + " " + H);
    const positions = layout(W, H, algo, 20);

    let out = '<defs><marker id="vxn-fb-arrow" viewBox="0 0 6 6" refX="3" refY="3" markerWidth="5" markerHeight="5" orient="auto"><path d="M0,0 L6,3 L0,6 z" fill="#d9701b"/></marker></defs>';

    algo.edges.forEach(function (e) {
      const pm = positions[e[0]], pc = positions[e[1]];
      if (!pm || !pc) return;
      out += '<line class="algo-edge" x1="' + pm.x + '" y1="' + (pm.y + 12) + '" x2="' + pc.x + '" y2="' + (pc.y - 12) + '" />';
    });

    const fbDst = algo.fbTo || algo.fb;
    if (algo.fb && positions[algo.fb] && positions[fbDst]) {
      const ps = positions[algo.fb];
      if (fbDst === algo.fb) {
        // Self-feedback: small loop on the source op's top-right.
        out += '<path class="algo-fb-loop" d="M ' + (ps.x + 14) + ' ' + (ps.y - 6) +
          ' C ' + (ps.x + 28) + ' ' + (ps.y - 6) + ', ' + (ps.x + 28) + ' ' + (ps.y - 22) + ', ' + (ps.x + 4) + ' ' + (ps.y - 22) +
          ' C ' + (ps.x - 8) + ' ' + (ps.y - 22) + ', ' + (ps.x - 2) + ' ' + (ps.y - 12) + ', ' + (ps.x + 1) + ' ' + (ps.y - 13) +
          '" marker-end="url(#vxn-fb-arrow)" />';
        out += '<text x="' + (ps.x + 30) + '" y="' + (ps.y - 15) + '" fill="#d9701b" font-size="8" font-weight="700">FB</text>';
      } else {
        // Two-operator feedback (algos 4, 6): source (carrier) → dest (top of
        // the branch), bowing out to the right past the intervening ops.
        const pd = positions[fbDst];
        const bx = Math.max(ps.x, pd.x) + 34;
        out += '<path class="algo-fb-loop" d="M ' + (ps.x + 14) + ' ' + ps.y +
          ' C ' + bx + ' ' + ps.y + ', ' + bx + ' ' + pd.y + ', ' + (pd.x + 14) + ' ' + pd.y +
          '" marker-end="url(#vxn-fb-arrow)" />';
        out += '<text x="' + (bx - 2) + '" y="' + ((ps.y + pd.y) / 2) + '" fill="#d9701b" font-size="8" font-weight="700">FB</text>';
      }
    }

    for (let op = 1; op <= 6; op++) {
      const p = positions[op];
      if (!p) continue;
      const role = isCarrier(op, algo) ? "carrier" : "modulator";
      const sel = (op === selectedOp) ? "selected" : "";
      out += '<g class="algo-op ' + role + ' ' + sel + '" data-op="' + op + '">';
      out += '<rect class="algo-op-box" x="' + (p.x - 14) + '" y="' + (p.y - 12) + '" width="28" height="24" rx="3" />';
      out += '<text class="algo-op-label" x="' + p.x + '" y="' + (p.y + 1) + '">' + op + '</text>';
      out += '</g>';
    }

    algo.carriers.forEach(function (c) {
      const p = positions[c];
      if (!p) return;
      out += '<line class="algo-edge" x1="' + p.x + '" y1="' + (p.y + 12) + '" x2="' + p.x + '" y2="' + (H - 4) + '" stroke="#d9701b" />';
    });

    svg.innerHTML = out;
  }

  function renderMini(svg, algoNum) {
    const algo = ALGOS[algoNum];
    if (!algo) {
      svg.innerHTML = '<text x="55" y="58" fill="#444" text-anchor="middle" font-size="10">algo ' + algoNum + '</text>';
      return;
    }
    const W = 110, H = 100;
    svg.setAttribute("viewBox", "0 0 " + W + " " + H);
    const positions = layout(W, H, algo, 12);
    let out = "";
    algo.edges.forEach(function (e) {
      const pm = positions[e[0]], pc = positions[e[1]];
      if (!pm || !pc) return;
      out += '<line stroke="#666" stroke-width="1" x1="' + pm.x + '" y1="' + (pm.y + 8) + '" x2="' + pc.x + '" y2="' + (pc.y - 8) + '" />';
    });
    const fbDst = algo.fbTo || algo.fb;
    if (algo.fb && positions[algo.fb] && positions[fbDst]) {
      const ps = positions[algo.fb];
      if (fbDst === algo.fb) {
        out += '<path stroke="#d9701b" stroke-width="1" fill="none" d="M ' + (ps.x + 8) + ' ' + (ps.y - 6) + ' Q ' + (ps.x + 18) + ' ' + (ps.y - 14) + ' ' + (ps.x + 8) + ' ' + (ps.y - 9) + '" />';
      } else {
        // Two-op feedback: source → dest arc bowing right.
        const pd = positions[fbDst];
        const bx = Math.max(ps.x, pd.x) + 20;
        out += '<path stroke="#d9701b" stroke-width="1" fill="none" d="M ' + (ps.x + 9) + ' ' + ps.y + ' C ' + bx + ' ' + ps.y + ', ' + bx + ' ' + pd.y + ', ' + (pd.x + 9) + ' ' + pd.y + '" />';
      }
    }
    for (let op = 1; op <= 6; op++) {
      const p = positions[op];
      if (!p) continue;
      const stroke = isCarrier(op, algo) ? "#d9701b" : "#87afb2";
      out += '<rect x="' + (p.x - 9) + '" y="' + (p.y - 8) + '" width="18" height="16" rx="2" fill="#1c1c1c" stroke="' + stroke + '" stroke-width="1.2" />';
      out += '<text x="' + p.x + '" y="' + (p.y + 3) + '" fill="#d6d6d6" text-anchor="middle" font-size="9" font-weight="700">' + op + '</text>';
    }
    algo.carriers.forEach(function (c) {
      const p = positions[c];
      if (!p) return;
      out += '<line stroke="#d9701b" stroke-width="1" x1="' + p.x + '" y1="' + (p.y + 8) + '" x2="' + p.x + '" y2="' + (H - 2) + '" />';
    });
    svg.innerHTML = out;
  }

  window.__vxn.panels.algoDiagram = {
    renderFull: renderFull,
    renderMini: renderMini,
    isCarrier: isCarrier,
    algos: ALGOS,
  };
})();
