// VXN2 page bootstrap. Initialises `window.__vxn` with the param table,
// IPC dispatch, and stub `applyViewEvents` / `applyPresetCorpus` handlers.
// `main.js` (loaded later in the same <script>) overwrites the handlers
// with real DOM-binding ones and fires the `ready` opcode.
//
// `__PARAMS_JSON__` and `__MATRIX_LISTS_JSON__` are spliced at HTML
// build time by `vxn2_ui_web::build_faceplate_html` — the descriptor
// array produced by `build_params_json` and the source/dest/curve
// pick-lists produced by `build_matrix_lists_json`.

(function () {
  const PARAMS = __PARAMS_JSON__;
  const MATRIX_LISTS = __MATRIX_LISTS_JSON__;
  const DEFAULT_PATCH = __DEFAULT_PATCH_JSON__;
  const SUBDIVISIONS = __SUBDIVISIONS_JSON__;
  const byName = Object.create(null);
  for (let i = 0; i < PARAMS.length; i++) {
    byName[PARAMS[i].name] = PARAMS[i];
  }

  function emptyRow() {
    return { source: 0, dest: 0, curve: 0, active: false, depth: 0.0 };
  }
  function emptyTable() {
    const t = new Array(16);
    for (let i = 0; i < 16; i++) t[i] = emptyRow();
    return t;
  }

  function dispatch(opcode, payload) {
    const msg = Object.assign({ op: opcode }, payload || {});
    try {
      window.ipc.postMessage(JSON.stringify(msg));
    } catch (e) {
      console.error("vxn2 ipc post failed", e, msg);
    }
  }

  // MIDI note number → name (e.g. 60 → "C4"). Shared util; mirrors the
  // copy in main.js so panels (KS graph) can label keyboard axes without
  // reaching into main's IIFE.
  var NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
  function noteName(m) {
    var n = Math.round(m);
    return NOTE_NAMES[((n % 12) + 12) % 12] + (Math.floor(n / 12) - 1);
  }

  window.__vxn = {
    params: PARAMS,
    paramsByName: byName,
    defaultPatch: DEFAULT_PATCH,
    noteName: noteName,
    // Tempo-sync subdivision labels (coarse→fine). A synced rate/time fader
    // maps its normalised position into this list while dragging.
    subdivisions: SUBDIVISIONS,
    matrix: {
      sources: (MATRIX_LISTS && MATRIX_LISTS.sources) || [],
      dests: (MATRIX_LISTS && MATRIX_LISTS.dests) || [],
      curves: (MATRIX_LISTS && MATRIX_LISTS.curves) || [],
      rows: emptyTable(),
    },
    // Per-op [left, right] KS level-curve discriminants (KsCurve: 0 NegLin,
    // 1 PosLin, 2 NegExp, 3 PosExp). Seeded to the legacy frozen default;
    // a KsCurveSnapshot from the engine overwrites it at boot / preset load.
    ksCurves: [[0, 2], [0, 2], [0, 2], [0, 2], [0, 2], [0, 2]],
    // Per-op EG level-curve discriminants (EgCurve: 0 Exp, 1 Lin). Seeded to
    // the shipped default (Exp = DX7 log curve); an EgCurveSnapshot from the
    // engine overwrites it at boot / preset load (ticket 0128).
    egCurves: [0, 0, 0, 0, 0, 0],
    dispatch: dispatch,
    panels: Object.create(null),
    primitives: [],
    /// Promise resolvers for in-flight text-input popups, keyed by the
    /// correlation `id` shipped with each `request_text_input`. main.js's
    /// `dispatchTextInput` stashes one resolver per call; the
    /// `text_input_result` arm in `applyViewEvents` looks it up, calls
    /// `resolve(value | null)`, and deletes the entry.
    pendingTextInputs: Object.create(null),
    applyViewEvents: function (events) {
      // main.js replaces this once primitives bind.
      window.__vxn._pendingBatch = (window.__vxn._pendingBatch || []).concat(events);
    },
    applyPresetCorpus: function (corpus) {
      window.__vxn._lastCorpus = corpus;
    },
  };
})();
