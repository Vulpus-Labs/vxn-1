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
  // Compact subset of DX7 algorithm topologies. Each entry:
  //   edges: [modOp, carOp] pairs (modulator → carrier).
  //   carriers: ops whose output sums into the algo's bus.
  //   fb: op carrying the algorithm-wide feedback loop (0 = none).
  // 0027 will fill in the remaining algos as needed; cells without an
  // entry render as a placeholder.
  const ALGOS = {
    1:  { edges: [[2,1],[4,3],[5,4],[6,5]],         carriers: [1, 3],    fb: 6 },
    2:  { edges: [[2,1],[4,3],[5,4],[6,5]],         carriers: [1, 3],    fb: 2 },
    3:  { edges: [[2,1],[4,3],[5,4],[6,5]],         carriers: [1, 3],    fb: 6 },
    4:  { edges: [[2,1],[3,1],[5,4],[6,5]],         carriers: [1, 4],    fb: 4 },
    5:  { edges: [[2,1],[4,3],[6,5]],               carriers: [1, 3, 5], fb: 6 },
    6:  { edges: [[2,1],[4,3],[6,5]],               carriers: [1, 3, 5], fb: 5 },
    7:  { edges: [[2,1],[4,3],[5,3],[6,5]],         carriers: [1, 3],    fb: 6 },
    8:  { edges: [[2,1],[4,3],[5,3],[6,5]],         carriers: [1, 3],    fb: 4 },
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

    if (algo.fb && positions[algo.fb]) {
      const p = positions[algo.fb];
      out += '<path class="algo-fb-loop" d="M ' + (p.x + 14) + ' ' + (p.y - 6) +
        ' C ' + (p.x + 28) + ' ' + (p.y - 6) + ', ' + (p.x + 28) + ' ' + (p.y - 22) + ', ' + (p.x + 4) + ' ' + (p.y - 22) +
        ' C ' + (p.x - 8) + ' ' + (p.y - 22) + ', ' + (p.x - 2) + ' ' + (p.y - 12) + ', ' + (p.x + 1) + ' ' + (p.y - 13) +
        '" marker-end="url(#vxn-fb-arrow)" />';
      out += '<text x="' + (p.x + 30) + '" y="' + (p.y - 15) + '" fill="#d9701b" font-size="8" font-weight="700">FB</text>';
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
    if (algo.fb && positions[algo.fb]) {
      const p = positions[algo.fb];
      out += '<path stroke="#d9701b" stroke-width="1" fill="none" d="M ' + (p.x + 8) + ' ' + (p.y - 6) + ' Q ' + (p.x + 18) + ' ' + (p.y - 14) + ' ' + (p.x + 8) + ' ' + (p.y - 9) + '" />';
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
