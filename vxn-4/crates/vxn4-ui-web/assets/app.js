/* ═══════════════════════════════════════════════════════════════════════════
   VXN-4 faceplate — page assembly.

   The panels are the vocabulary; this is the sentence. It builds each panel
   out of the primitives in `panels/`, owns the selection state the operator
   tab needs (which operator, which route row is being pointed at), and routes
   the tab strip.

   ## What it deliberately does not have

   A bridge. Every control on this page holds its own value and nothing leaves
   the window: ticket 0387 is the chrome — the editor opens, resizes, closes
   and draws — and binding controls to the engine is 0388.

   That is why there is no mock patch data here either. The mockup invented a
   set of macro labels, a plausible PM topology and a list of modulation
   sources and destinations, which was the right thing for arguing about
   layout and the wrong thing to ship: an invented value that looks real is a
   bug report waiting to be filed. What the engine cannot yet answer shows a
   placeholder. What is structural — how many operators, how many matrix
   slots, the curve table — comes from Rust through `__VXN4_CONFIG__`, so the
   page cannot drift from the engine on the facts it does state.
   ═══════════════════════════════════════════════════════════════════════════ */

import {
  mapValue, invValue, fmtHz, fmtSec, fmtPct,
} from './panels/value.js';
import { clearPop } from './panels/pop.js';
import { cellDiv } from './panels/dom.js';
import { fader } from './panels/fader.js';
import { dial } from './panels/dial.js';
import { toggle, buttonGroup } from './panels/toggle.js';
import { combo, comboGrouped } from './panels/combo.js';
import { waveKnob, LFO_SHAPES } from './panels/wave-knob.js';
import { wavePicker, OP_WAVE_DEFS } from './panels/wave-picker.js';
import { hFader } from './panels/hfader.js';
import { meter } from './panels/meter.js';
import { createScope } from './panels/scope.js';
import { createEgGraph } from './panels/eg-graph.js';
import { createKsGraph } from './panels/ks-graph.js';
import { createPmGrid } from './panels/pm-grid.js';

/* ── Config from Rust ───────────────────────────────────────────────────── */

const CFG = window.__VXN4_CONFIG__;
const NOPS = CFG.n_ops;
const N_MATRIX_SLOTS = CFG.n_matrix_slots;
// Polarity × Shape, as `vxn_core_matrix::curve::CURVE_LABELS` orders them.
// Shipped rather than retyped: the flat code a slot stores is an index into
// that table, and a picker listing them in a different order would be a silent
// mis-selection rather than a visible error.
const CURVES = CFG.curves;

/* ── Rosters the bridge has not supplied yet ────────────────────────────── */

// Sources and destinations come from the engine's matrix roster once there is
// a bridge to carry them (0388). Until then every picker offers the empty
// selection and nothing else — which is not a placeholder in the decorative
// sense: a slot with no source IS off, and the table dims itself accordingly,
// so the page states the truth rather than a sample of it.
const MOD_SOURCES = ["—"];
const MOD_DESTS = [{ label: "", items: ["—"] }];

/* The preset bar is markup and nothing else here. The browser, the stepper and
   the save paths are 0389, on top of the preset store 0385 is building; the
   name field shows the empty selection until something can load one. */

/* ═══ PERFORM ══════════════════════════════════════════════════════════════ */

// Macro row. The label and range under each knob are patch data — switching
// preset relabels all eight — so until a patch can say, the knob shows its
// index, which is the one thing about it that never changes and is what host
// automation sees.
const macroBody = document.getElementById("macro-body");
for (let i = 0; i < CFG.n_macros; i++) {
  const wrap = document.createElement("div");
  wrap.className = "ctl macro-cell";
  const idxLbl = document.createElement("div");
  idxLbl.className = "macro-idx";
  idxLbl.textContent = "MACRO " + (i + 1);
  wrap.append(dial("—", { min: 0, max: 100, fmt: fmtPct }, 0.5, 46), idxLbl);
  macroBody.append(wrap);
}

// LFO 1 — note-synced. Delay holds the LFO at zero after a note-on, Fade ramps
// it in; Sync swaps Rate's Hz for a musical subdivision, and rides under the
// fader it re-maps rather than in the bottom strip. Retrig is the one quirk of
// the LFO as a whole, so it is the only thing in the strip.
function faderWith(f, tg) {
  const col = document.createElement("div");
  col.className = "ctl-col";
  col.append(f, tg);
  return col;
}

document.getElementById("lfo1-body").append(
  waveKnob("Shape", LFO_SHAPES, 0),
  faderWith(
    fader("Rate", { min: 0.01, max: 40, exp: 1, unit: " Hz", dp: 2 }, 0.45),
    toggle("Sync", false),
  ),
  fader("Delay", { min: 0, max: 5, exp: 1, fmt: fmtSec }, 0),
  fader("Fade", { min: 0, max: 5, exp: 1, fmt: fmtSec }, 0),
  fader("Depth", { min: 0, max: 100, fmt: fmtPct }, 0),
);
document.getElementById("lfo1-strip").append(toggle("Retrig", true));

// LFO 2 is free-running: no onset to delay, nothing to retrigger, and nothing
// to phase-lock to.
document.getElementById("lfo2-body").append(
  waveKnob("Shape", LFO_SHAPES, 0),
  faderWith(
    fader("Rate", { min: 0.01, max: 40, exp: 1, unit: " Hz", dp: 2 }, 0.3),
    toggle("Sync", false),
  ),
  fader("Depth", { min: 0, max: 100, fmt: fmtPct }, 0),
);

// Filter — two stages. The HP is a fixed high-pass ahead of the multimode
// section; the main stage is vxn-2's control set (Cutoff + Tuned, Reso, Drive,
// Key, with Mode and Slope as button groups).
function subgroup(caption, children, extra) {
  const group = document.createElement("div");
  group.className = "subgroup";
  const body = document.createElement("div");
  body.className = "subgroup-body";
  body.append(...children);
  const cap = document.createElement("div");
  cap.className = "subgroup-cap";
  cap.textContent = caption;
  group.append(body, cap);
  if (extra) group.append(extra);
  return group;
}

(() => {
  const cutCol = faderWith(
    fader("Cutoff", { min: 20, max: 20000, exp: 1, fmt: fmtHz }, 1.0),
    toggle("Tuned", false),
  );
  const sep = document.createElement("div");
  sep.className = "panel-sep";
  document.getElementById("filter-body").append(
    subgroup("HP stage", [
      fader("Cutoff", { min: 10, max: 2000, exp: 1, fmt: fmtHz }, 0),
      fader("Reso", { min: 0, max: 100, fmt: fmtPct }, 0),
    ]),
    sep,
    subgroup("Main stage", [
      buttonGroup("Mode", ["LP", "HP", "BP", "Notch"], 0),
      buttonGroup("Slope", ["2P", "4P"], 1),
      cutCol,
      fader("Reso", { min: 0, max: 100, fmt: fmtPct }, 0),
      fader("Drive", { min: 0, max: 100, fmt: fmtPct }, 0),
      fader("Key", { min: 0, max: 100, fmt: fmtPct }, 0),
    ]),
  );
})();

// ADHSR × 2. Hold sits between Decay and Sustain: the level holds at the decay
// target before falling to sustain, which is the segment the DX-style four-rate
// EG already has and plain ADSR does not.
["env1-body", "env2-body"].forEach((id) => {
  document.getElementById(id).append(
    fader("A", { min: 0.001, max: 8, exp: 1, fmt: fmtSec }, 0),
    fader("D", { min: 0.001, max: 12, exp: 1, fmt: fmtSec }, 0.4),
    fader("H", { min: 0, max: 8, exp: 1, fmt: fmtSec }, 0),
    fader("S", { min: 0, max: 100, fmt: fmtPct }, 1.0),
    fader("R", { min: 0.001, max: 20, exp: 1, fmt: fmtSec }, 0.35),
    buttonGroup("Curve", ["Lin", "Exp"], 1),
  );
});

const scope = createScope(document.getElementById("scope"));

/* ═══ MIXER ════════════════════════════════════════════════════════════════ */

// Held so 0388 can drive them from the engine's level tap. Until then they
// read zero, because nothing is measuring anything.
const METERS = [];
function addMeter(label, kind) {
  const m = meter(label, kind);
  METERS.push(m);
  return m;
}

(() => {
  const grid = document.createElement("div");
  grid.className = "dial-grid";
  grid.append(
    dial("Thresh", { min: -48, max: 0, unit: " dB", dp: 1 }, 1.0),
    dial("Ratio", { min: 1, max: 20, exp: 1, unit: ":1", dp: 1 }, 0.4),
    dial("Atk", { min: 0.1, max: 200, exp: 1, unit: " ms", dp: 1 }, 0.3),
    dial("Rel", { min: 5, max: 2000, exp: 1, unit: " ms", dp: 0 }, 0.5),
    dial("Mkup", { min: 0, max: 24, unit: " dB", dp: 1 }, 0),
    dial("Drive", { min: 0, max: 100, fmt: fmtPct }, 0),
  );
  document.getElementById("dyn-body").append(
    addMeter("In"), grid, fader("Mix", { min: 0, max: 100, fmt: fmtPct }, 1.0),
    addMeter("GR", "gr"), addMeter("Out"),
  );

  document.getElementById("master-body").append(
    fader("Tune", { min: -100, max: 100, unit: " ct", signed: true }, 0.5),
    fader("Volume", { min: -60, max: 6, unit: " dB", dp: 1 }, 0.9),
    addMeter("Out"),
  );
  document.getElementById("master-strip").append(
    buttonGroup("Quality", ["8x", "16x"], 0, "row"),
    toggle("Limit", true),
  );

  document.getElementById("phaser-body").append(
    fader("Rate", { min: 0.01, max: 10, exp: 1, unit: " Hz", dp: 2 }, 0.3),
    fader("Depth", { min: 0, max: 100, fmt: fmtPct }, 0.5),
    fader("FB", { min: 0, max: 100, fmt: fmtPct }, 0.4),
    fader("Stereo", { min: 0, max: 100, fmt: fmtPct }, 0.5),
    fader("Mix", { min: 0, max: 100, fmt: fmtPct }, 0),
  );
  document.getElementById("chorus-body").append(
    fader("Rate", { min: 0.01, max: 10, exp: 1, unit: " Hz", dp: 2 }, 0.25),
    fader("Depth", { min: 0, max: 100, fmt: fmtPct }, 0.45),
    fader("Mix", { min: 0, max: 100, fmt: fmtPct }, 0),
  );
  document.getElementById("delay-body").append(
    faderWith(fader("Time", { min: 0.005, max: 2, exp: 1, fmt: fmtSec }, 0.5), toggle("Sync", true)),
    fader("FB", { min: 0, max: 100, fmt: fmtPct }, 0.4),
    faderWith(fader("Mix", { min: 0, max: 100, fmt: fmtPct }, 0), toggle("Ping-Pong", true)),
  );
  document.getElementById("reverb-body").append(
    fader("Size", { min: 0, max: 100, fmt: fmtPct }, 0.6),
    fader("Decay", { min: 0.1, max: 30, exp: 1, fmt: fmtSec }, 0.5),
    fader("Damp", { min: 0, max: 100, fmt: fmtPct }, 0.5),
    fader("Mix", { min: 0, max: 100, fmt: fmtPct }, 0),
  );
})();

// Header on/off switches. A bypassed section dims its whole body — including
// at load, so a patch that ships with the phaser off looks off.
document.querySelectorAll(".hdr-switch").forEach((s) => {
  const body = s.closest(".panel").querySelector(".panel-body");
  const sync = () => body.classList.toggle("dim", !s.classList.contains("on"));
  s.addEventListener("click", () => { s.classList.toggle("on"); sync(); });
  sync();
});

/* ═══ OPERATORS ════════════════════════════════════════════════════════════ */

/* Per-operator page state.
 *
 * Everything sits at its neutral position: no routes wired, no operator
 * sending to the bus, every depth on its centre. The mockup opened on a
 * plausible topology, which made the grid worth looking at and would make this
 * page lie about what the engine is doing. 0388 replaces this whole factory
 * with the descriptor table's defaults and then with the live patch.
 *
 * `routes[d]` is this operator's depth into operator `d` — `d === self` being
 * its own feedback — and `bus` / `pan` are its send into the stereo sum and its
 * position there. Depths are stored as the normalised position the faders and
 * the grid both read, not as plain values, because those two views have to
 * agree to the pixel.
 */
function emptyRoute() {
  return { on: false, k: 0.5, offSrc: 0, offCur: 0, sclSrc: 0, sclCur: 0 };
}

function makeOp() {
  return {
    wave: 0,
    // Tuning, in vxn-2's scheme (OpParams / PARAMETERS.md): a rational ratio
    // num/den with a fine offset on the numerator, a separate cents detune, or
    // a fixed frequency that ignores the played note.
    mode: 0,                       // 0 = Ratio, 1 = Fixed
    num: 0, den: 0, fine: 0.5, cents: 0.5, hz: 0.5,
    level: 1.0, damp: 1.0, phase: 0, decorr: 0,
    vel: 0,
    // Key scaling, in the vxn-2 idiom: one break point, independent depth and
    // curve either side of it, plus a rate-scaling amount that pivots
    // separately (A3, as the DSP does).
    ks: { bp: 60, l: 0, r: 0, lExp: false, rExp: false, lBoost: false, rBoost: false, rate: 0 },
    routes: Array.from({ length: NOPS }, emptyRoute),
    bus: emptyRoute(),
    pan: emptyRoute(),
    // The operator's own amplitude EG — the four-rate / four-level DX shape
    // the engine runs, one per operator per voice. Not the same thing as
    // PERFORM's two ADHSR mod sources.
    eg: { t: [0.12, 0.45, 0.6, 0.35], l: [1.0, 0.7, 0.5, 0.0] },
  };
}

const OPS = Array.from({ length: NOPS }, makeOp);
let selOp = 0;
let hiRoute = null;   // route row to band after arriving from the PM grid

// Same specs the route table and the grid both use, so a depth dragged on one
// and read on the other is one number in one unit.
const PM_SPEC = { min: -8, max: 8, dp: 2, signed: true, bipolar: true };
const BUS_SPEC = { min: 0, max: 100, fmt: fmtPct };
const PAN_SPEC = { min: -1, max: 1, dp: 2, signed: true, bipolar: true };

/* Tuning specs, matching vxn-2's OpParams exactly (PARAMETERS.md §Operator):
   frequency = note × (num + fine/100) / den × 2^(detune/1200), or fixed_hz flat
   if the mode is Fixed. Num and Den are integers — the whole point of a
   rational ratio is that it lands on detents, so they step rather than sweep,
   and Fine is what gets you between them (its sweep width scales inversely with
   den, which is why it belongs on the numerator rather than being a second
   cents control). Detune is the log-domain one, independent of the ratio, for
   thickening and beating. */
const TUNE_SPECS = {
  num:   { min: 1, max: 32, step: 1, dp: 0 },
  den:   { min: 1, max: 8, step: 1, dp: 0 },
  fine:  { min: -100, max: 100, step: 1, dp: 0, signed: true, bipolar: true },
  cents: { min: -100, max: 100, step: 1, dp: 0, signed: true, bipolar: true, unit: " ct" },
  hz:    { min: 1, max: 9772, exp: 1, fmt: fmtHz },
};

/** What the tuning controls come out to, for the panel header. */
function opTuning(op) {
  if (op.mode === 1) return fmtHz(mapValue(TUNE_SPECS.hz, op.hz));
  const num = mapValue(TUNE_SPECS.num, op.num);
  const den = mapValue(TUNE_SPECS.den, op.den);
  const fine = mapValue(TUNE_SPECS.fine, op.fine);
  const cents = mapValue(TUNE_SPECS.cents, op.cents);
  const ratio = ((num + fine / 100) / den) * Math.pow(2, cents / 1200);
  return `${num}/${den}${fine ? (fine > 0 ? "+" : "−") + Math.abs(fine) : ""}` +
         `${cents ? (cents > 0 ? " +" : " −") + Math.abs(cents) + "ct" : ""}` +
         `  (${ratio.toFixed(3)}×)`;
}

/* ── Operator tabs ──────────────────────────────────────────────────────── */

const opTabsEl = document.getElementById("op-tabs");
function buildOpTabs() {
  opTabsEl.innerHTML = "";
  for (let i = 0; i < NOPS; i++) {
    const b = document.createElement("button");
    b.type = "button";
    // The dot says "this operator reaches the output". On an 8-operator bank
    // where most operators are modulators most of the time, that is the fact
    // you want before you start editing one.
    b.className = "op-tab" + (i === selOp ? " active" : "") + (OPS[i].bus.on ? " audible" : "");
    b.append(document.createTextNode(`OP ${i + 1}`), cellDiv("op-dot", ""));
    b.onclick = () => { selOp = i; hiRoute = null; renderOperators(); };
    opTabsEl.append(b);
  }
}

document.getElementById("op-global-strip").append(
  buttonGroup("Quality", ["8x", "16x"], 0, "row"),
);

/* ── Per-operator parameters ────────────────────────────────────────────────

   `Decorr` is `OpConfig::phase_spread` — how far this operator's onset phase is
   scattered from its siblings, 1.0 being the historical unconditional scatter
   and 0.0 starting it phase-coherent. It is NOT stereo width (that is Pan, on
   the bus route): coherent is loud and full but cannot be widened by panning,
   scattered is wide but comb-filtered and quieter. `supersaw` lost 5 dB to
   seven saws cancelling before this was dialable. Named Decorr here precisely
   because "Spread" read as a stereo control. */

const opcfgBody = document.getElementById("opcfg-body");
function buildOpCfg() {
  const op = OPS[selOp];
  opcfgBody.innerHTML = "";
  document.getElementById("routes-header").textContent = `ROUTES OUT — OPERATOR ${selOp + 1}`;
  document.getElementById("opeg-header").textContent = `OPERATOR ${selOp + 1} — AMP EG`;

  // The header carries the resolved tuning. It is the one figure no single
  // control holds — four faders go into it, and nobody works out
  // (7 + 12/100) / 4 in their head — so it is derived state rather than a
  // printed control value, and does not break the popup convention.
  const header = () => {
    document.getElementById("opcfg-header").textContent =
      `OPERATOR ${selOp + 1}  —  ${opTuning(op)}`;
  };

  // Tuning group. Ratio mode greys the Hz fader and Fixed mode greys the four
  // rational controls, following vxn-2: the inert set stays visible so the
  // mode switch reads as "these, not those" rather than as controls vanishing.
  const mk = (label, key) => {
    const f = fader(label, TUNE_SPECS[key], op[key]);
    f.api.onChange((t) => { op[key] = t; header(); });
    return f;
  };
  const numF = mk("Num", "num"), denF = mk("Den", "den");
  const fineF = mk("Fine", "fine"), centsF = mk("Cents", "cents");
  const hzF = mk("Hz", "hz");
  const modeG = buttonGroup("Mode", ["Ratio", "Fixed"], op.mode, "row", (i) => {
    op.mode = i; syncMode(); header();
  });
  function syncMode() {
    const fixed = op.mode === 1;
    [numF, denF, fineF, centsF].forEach((f) => f.classList.toggle("dim", fixed));
    hzF.classList.toggle("dim", !fixed);
  }
  syncMode();

  const sep = document.createElement("div");
  sep.className = "panel-sep";

  // Every control writes back into `op`, because this whole panel is rebuilt
  // when the selected operator changes — a control that only held its own
  // value would forget it on the way to operator 2 and back.
  const scalar = (label, key, spec) => {
    const f = fader(label, spec, op[key]);
    f.api.onChange((t) => { op[key] = t; });
    return f;
  };
  opcfgBody.append(
    wavePicker("Wave", OP_WAVE_DEFS, op.wave, (i) => { op.wave = i; }),
    subgroup("Tuning", [numF, denF, fineF, centsF, hzF], modeG),
    sep,
    scalar("Level", "level", { min: 0, max: 100, fmt: fmtPct }),
    scalar("Damp", "damp", { min: 200, max: 20000, exp: 1, fmt: fmtHz }),
    scalar("Phase", "phase", { min: 0, max: 360, unit: "°", dp: 0 }),
    scalar("Decorr", "decorr", { min: 0, max: 100, fmt: fmtPct }),
    scalar("Vel", "vel", { min: 0, max: 100, fmt: fmtPct }),
  );
  header();
}

/* ── Route table ────────────────────────────────────────────────────────────

   Ten rows: eight into the operator bank (the self row is the operator's own
   feedback), then the bus send and its pan. Each row is a constant plus an
   offset (source + curve) and a scaling (source + curve), which is the shape a
   vxn-core-matrix slot already has. */

const routesEl = document.getElementById("routes");
// The grid and the table are two views of one route. An edit on the grid pushes
// into the table's controls rather than rebuilding it — a rebuild mid-drag
// would drop the pointer capture and lose the gesture.
const routeControls = new Map();
let hiCells = null;

function buildRoutes() {
  const op = OPS[selOp];
  routesEl.innerHTML = "";
  hiCells = null;
  routeControls.clear();
  ["On", "To", "Constant", "Offset src", "Curve", "Scale src", "Curve"].forEach((h) => {
    routesEl.append(cellDiv("routes-hd", h));
  });

  const rows = [];
  for (let d = 0; d < NOPS; d++) {
    rows.push({
      key: d,
      r: op.routes[d],
      label: d === selOp ? "SELF" : "OP " + (d + 1),
      cls: d === selOp ? "route-row-self" : "",
      spec: PM_SPEC,
    });
  }
  rows.push({ key: "bus", r: op.bus, label: "BUS", cls: "route-row-bus", spec: BUS_SPEC });
  rows.push({ key: "pan", r: op.pan, label: "PAN", cls: "route-row-bus", spec: PAN_SPEC });

  rows.forEach((row) => {
    const tg = toggle("", row.r.on, (on) => { row.r.on = on; sync(); });
    tg.classList.add("ctl-tg-cell");
    const target = cellDiv("route-target " + row.cls, row.label);
    const kf = hFader(row.r.k, row.spec, row.label, (t) => { row.r.k = t; pmGrid.paint(); });
    const offSrc = combo(MOD_SOURCES, row.r.offSrc, (v) => { row.r.offSrc = v; sync(); });
    const offCur = combo(CURVES, row.r.offCur, (v) => { row.r.offCur = v; });
    const sclSrc = combo(MOD_SOURCES, row.r.sclSrc, (v) => { row.r.sclSrc = v; sync(); });
    const sclCur = combo(CURVES, row.r.sclCur, (v) => { row.r.sclCur = v; });
    const cells = [tg, target, kf, offSrc, offCur, sclSrc, sclCur];

    // A curve with no source behind it does nothing, so it dims until one is
    // picked — the same rule the mod matrix rows use.
    function sync() {
      const live = row.r.on;
      [target, kf, offSrc, sclSrc].forEach((e) => e.classList.toggle("dim", !live));
      offCur.classList.toggle("dim", !live || row.r.offSrc === 0);
      sclCur.classList.toggle("dim", !live || row.r.sclSrc === 0);
      buildOpTabs();
      pmGrid.paint();
    }

    routesEl.append(...cells);
    routeControls.set(row.r, { kf, tg, sync });
    if (row.key === hiRoute) hiCells = cells;
    sync();
  });

  paintRouteBand();
}

// The band is measured, not laid out: the grid owns the row geometry, so the
// highlight reads its extent back off the row's cells once that geometry
// exists. Cells in a row are not all the same height — a 13px toggle sits
// beside a 17px combo — so the band takes the union rather than any one cell.
function paintRouteBand() {
  const old = routesEl.querySelector(".route-band");
  if (old) old.remove();
  if (!hiCells || hiRoute === null) return;
  const top = Math.min(...hiCells.map((c) => c.offsetTop));
  const bottom = Math.max(...hiCells.map((c) => c.offsetTop + c.offsetHeight));
  const band = document.createElement("div");
  band.className = "route-band";
  band.style.top = (top - 2) + "px";
  band.style.height = (bottom - top + 4) + "px";
  routesEl.append(band);
}

/* ── PM grid ────────────────────────────────────────────────────────────── */

const pmGrid = createPmGrid(document.getElementById("pm-grid"), {
  nOps: NOPS,
  model: {
    route: (src, dst) => OPS[src].routes[dst],
    bus: (op) => OPS[op].bus,
  },
  pmSpec: PM_SPEC,
  busSpec: BUS_SPEC,
  onSelect: (srcOp, rowKey) => { selOp = srcOp; hiRoute = rowKey; renderOperators(); },
  onEdit: (route) => {
    const c = routeControls.get(route);
    if (!c) return;
    c.kf.api.set(route.k);
    c.tg.api.set(route.on);
    c.sync();
  },
});

/* ── Operator amp EG ────────────────────────────────────────────────────── */

const opegBody = document.getElementById("opeg-body");
const opegCells = [];
const EG_TIME_SPECS = [
  { min: 0.001, max: 8, exp: 1, fmt: fmtSec },
  { min: 0.001, max: 12, exp: 1, fmt: fmtSec },
  { min: 0.001, max: 20, exp: 1, fmt: fmtSec },
  { min: 0.001, max: 20, exp: 1, fmt: fmtSec },
];
const EG_LEVEL_SPEC = { min: 0, max: 100, fmt: fmtPct };

// Rebuilt per selected operator, so the faders carry that operator's EG. The
// cells are the store the graph reads through — one set of numbers, two views.
function buildOpEg() {
  const op = OPS[selOp];
  opegBody.innerHTML = "";
  opegCells.length = 0;
  ["A", "D1", "D2", "R"].forEach((l, i) => {
    const c = fader(l, EG_TIME_SPECS[i], op.eg.t[i]);
    c.api.onChange((t) => { op.eg.t[i] = t; egGraph.draw(); });
    opegCells.push(c);
    opegBody.append(c);
  });
  const sep = document.createElement("div");
  sep.className = "panel-sep";
  opegBody.append(sep);
  ["L1", "L2", "L3", "L4"].forEach((l, i) => {
    const c = fader(l, EG_LEVEL_SPEC, op.eg.l[i]);
    c.api.onChange((t) => { op.eg.l[i] = t; egGraph.draw(); });
    opegCells.push(c);
    opegBody.append(c);
  });
}

const egGraph = createEgGraph(document.getElementById("eg-graph"), {
  times: () => (opegCells.length < 8 ? [] : [0, 1, 2, 3].map((i) => mapValue(EG_TIME_SPECS[i], opegCells[i].api.get()))),
  levels: () => (opegCells.length < 8 ? [] : [4, 5, 6, 7].map((i) => opegCells[i].api.get())),
  setTime: (i, secs) => opegCells[i].api.set(invValue(EG_TIME_SPECS[i], secs)),
  setLevel: (i, norm) => opegCells[4 + i].api.set(norm),
});

/* ── Key scaling ────────────────────────────────────────────────────────── */

const ksSide = document.getElementById("ks-side");
// No `onChange`: nothing beside the graph shows anything a graph drag changes,
// so rebuilding that column per pointermove would be DOM churn for no repaint.
// The hook is where a bound build reports the edit onward (0388).
const ksGraph = createKsGraph(document.getElementById("ks-graph"), {
  ks: () => OPS[selOp].ks,
});

// The graph carries depth, sign and break point; what is left beside it is the
// per-side curve shape and the separately-pivoted EG rate scaling.
function buildKsSide() {
  const ks = OPS[selOp].ks;
  ksSide.innerHTML = "";
  ksSide.append(
    buttonGroup("Left", ["Lin", "Exp"], ks.lExp ? 1 : 0, "row",
                (i) => { ks.lExp = i === 1; ksGraph.draw(); }),
    buttonGroup("Right", ["Lin", "Exp"], ks.rExp ? 1 : 0, "row",
                (i) => { ks.rExp = i === 1; ksGraph.draw(); }),
  );
  // A dial, not a fader: the column beside the graph has no height to spare,
  // and rate scaling is a set-and-forget amount rather than a performed one.
  const rate = dial("EG Rate", { min: 0, max: 100, fmt: fmtPct }, ks.rate);
  rate.api.onChange((t) => { ks.rate = t; });
  rate.style.marginTop = "2px";
  ksSide.append(rate);
}

/**
 * Everything that follows the selected operator. The PM grid is NOT rebuilt
 * here — it is built once and repainted, so its cells survive a selection
 * change; rebuilding between the two clicks of a double-click would replace
 * the element under the pointer and the toggle would never land.
 */
function renderOperators() {
  buildOpTabs();
  pmGrid.refreshSelection(selOp);
  pmGrid.paint();
  buildOpCfg();
  buildRoutes();
  buildOpEg();
  egGraph.draw();
  ksGraph.draw();
  buildKsSide();
}

/* ═══ Mod matrix overlay ═══════════════════════════════════════════════════
   The voice-level routing table, one row per engine slot. Opens over the whole
   faceplate from the tab strip rather than living on a panel: it is reached
   rarely, it is wide, and it is not tied to one tab. */

const mmGrid = document.getElementById("mm-grid");
function buildMatrix() {
  mmGrid.innerHTML = "";
  ["On", "#", "Source", "Destination", "Depth", "Curve", "Scale src", "Curve"].forEach((h) => {
    mmGrid.append(cellDiv("mm-hd", h));
  });

  for (let i = 0; i < N_MATRIX_SLOTS; i++) {
    const slot = { on: false, src: 0, dest: 0, depth: 0.5, curve: 0, sclSrc: 0, sclCurve: 0 };
    const tg = toggle("", slot.on, (on) => { slot.on = on; sync(); });
    tg.classList.add("ctl-tg-cell");
    const num = cellDiv("mm-num", String(i + 1));
    const src = combo(MOD_SOURCES, slot.src, (v) => { slot.src = v; sync(); });
    const dest = comboGrouped(MOD_DESTS, slot.dest, (v) => { slot.dest = v; sync(); });
    const depth = hFader(slot.depth, { min: -100, max: 100, dp: 0, signed: true, bipolar: true },
                         "Depth", (t) => { slot.depth = t; });
    const curve = combo(CURVES, slot.curve, (v) => { slot.curve = v; });
    const sclSrc = combo(MOD_SOURCES, slot.sclSrc, (v) => { slot.sclSrc = v; sync(); });
    const sclCurve = combo(CURVES, slot.sclCurve, (v) => { slot.sclCurve = v; });

    // An empty slot, or one with no scaling source, dims the parts of the row
    // that cannot do anything yet.
    function sync() {
      const live = slot.on && slot.src !== 0 && slot.dest !== 0;
      [num, dest, depth, curve, sclSrc].forEach((e) => e.classList.toggle("dim", !live));
      src.classList.toggle("dim", !slot.on);
      sclCurve.classList.toggle("dim", !live || slot.sclSrc === 0);
    }
    mmGrid.append(tg, num, src, dest, depth, curve, sclSrc, sclCurve);
    sync();
  }
}

const mmBackdrop = document.getElementById("matrix-backdrop");
const mmBtn = document.getElementById("matrix-open");
function setMatrixOpen(open) {
  mmBackdrop.hidden = !open;
  mmBtn.classList.toggle("lit", open);
  // The overlay covers whatever the pointer was over, which will never get its
  // own `pointerleave`, so the readout has to be dropped explicitly.
  clearPop();
}
mmBtn.onclick = () => setMatrixOpen(mmBackdrop.hidden);
document.getElementById("matrix-close").onclick = () => setMatrixOpen(false);
mmBackdrop.addEventListener("pointerdown", (e) => {
  if (e.target === mmBackdrop) setMatrixOpen(false);
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") setMatrixOpen(false);
});

/* ═══ Tabs ═════════════════════════════════════════════════════════════════ */

function selectTab(name) {
  document.querySelectorAll(".tab-btn").forEach((b) => b.classList.toggle("active", b.dataset.tab === name));
  document.querySelectorAll(".tab-pane").forEach((p) => p.classList.toggle("active", p.dataset.tabPane === name));
  clearPop();
  // A canvas in a `display: none` pane has no layout, so it could not be sized
  // when it was last painted. Repaint on the way in rather than on a timer.
  drawGraphs();
}
document.getElementById("tab-strip").addEventListener("click", (e) => {
  const btn = e.target.closest(".tab-btn");
  if (btn) selectTab(btn.dataset.tab);
});

// `#mixer` / `#operators` open straight onto a tab and `#matrix` opens the
// overlay — the screenshotting aid the mockup carries, kept because it is how
// design review looks at one surface at a time. It is deliberately NOT what
// the tab strip uses: the strip calls `selectTab` directly, so routing through
// the hash cannot become load-bearing and a host that rewrites the page's URL
// cannot move the player off their tab.
function applyHash() {
  const h = location.hash.slice(1);
  if (h === "matrix") { selectTab("perform"); setMatrixOpen(true); return; }
  if (h) { setMatrixOpen(false); selectTab(h); }
}
window.addEventListener("hashchange", applyHash);

/* ═══ Paint ════════════════════════════════════════════════════════════════

   No animation loop. The mockup ran one to wander the meters and roll a
   stand-in scope trace, which is what made it read as alive; here there is
   nothing to animate, and a webview burning a frame callback inside a host's
   main thread to draw a fabricated waveform is a cost with a lie on the end of
   it. 0388 drives these from the engine's frames, at the host timer's rate. */

function drawGraphs() {
  scope.draw(null);
  egGraph.draw();
  ksGraph.draw();
  METERS.forEach((m) => m.tick(0, 0));
}

buildMatrix();
renderOperators();
applyHash();
drawGraphs();

// The window is fixed-size, so this is not a layout responder: it is the
// device-pixel-ratio path. Dragging a host window between a retina and a
// non-retina display changes the backing store every canvas needs without
// changing a single CSS pixel of the layout.
window.addEventListener("resize", drawGraphs);
