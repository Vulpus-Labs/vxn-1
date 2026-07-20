// VXN3 faceplate. Voice-library model: named voices (engine + flavour) live in a
// library and are edited in the Voices tab; lanes reference a voice. Structured edits
// → IPC ops; playhead ← view events.
(function () {
  "use strict";

  var CFG = window.__VXN3_CONFIG__ || { tracks: 8, steps: 16, engines: [], macro_slots: 3 };
  var NT = CFG.tracks, NS = CFG.steps;
  var ENGINES = CFG.engines; // [{id,label,params:[...],flavours:[...]}]
  var NSLOT = CFG.macro_slots || 3;
  var PROBS = [1.0, 0.75, 0.5, 0.25];
  var CURVES = ["linear", "exp"];

  function send(op, extra) {
    var msg = Object.assign({ op: op }, extra || {});
    try { window.ipc.postMessage(JSON.stringify(msg)); }
    catch (e) { /* standalone preview: no host ipc */ }
  }

  // ── small DOM + data helpers ────────────────────────────────────────────────
  function el(tag, cls, txt) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    if (txt != null) e.textContent = txt;
    return e;
  }
  function engineById(id) {
    for (var i = 0; i < ENGINES.length; i++) if (ENGINES[i].id === id) return ENGINES[i];
    return ENGINES[0] || { id: "kick", label: "Kick", params: [], flavours: [] };
  }
  function cloneFlavour(f) {
    return {
      base: (f.base || []).slice(),
      bindings: (f.bindings || []).map(function (b) { return { slot: b.slot, param: b.param, depth: b.depth, curve: b.curve }; }),
      macro_defaults: (f.macro_defaults || []).slice(),
      macro_names: (f.macro_names || []).slice(), // per-slot user override; "" = derive
    };
  }
  // Bindings a macro slot drives (a slot may bind several params, each its own depth).
  function slotBindings(flav, slot) {
    return flav.bindings.filter(function (b) { return b.slot === slot; });
  }
  // A macro's display name: user override, else the first bound param's name, else "M<n>".
  function macroName(flav, eng, slot) {
    var override = (flav.macro_names && flav.macro_names[slot]) || "";
    if (override) return override;
    var bs = slotBindings(flav, slot);
    if (bs.length && eng.params[bs[0].param]) return eng.params[bs[0].param].name;
    return "M" + (slot + 1);
  }
  function defaultFlavour(engId) {
    var e = engineById(engId);
    var f = (e.flavours && e.flavours[0]) || { base: [], bindings: [], macro_defaults: [] };
    return cloneFlavour(f);
  }
  function fmtVal(v, unit) {
    var s = Math.abs(v) >= 100 ? v.toFixed(0) : Math.abs(v) >= 1 ? v.toFixed(2) : v.toFixed(4);
    return unit ? s + " " + unit : s;
  }
  function bindingForSlot(flav, slot) {
    for (var i = 0; i < flav.bindings.length; i++) if (flav.bindings[i].slot === slot) return flav.bindings[i];
    return null;
  }

  // Fill indicator: set each range's `--pct` (thumb position) so the CSS track shows
  // the green→orange→red gradient up to the thumb and grey beyond.
  function paintRange(inp) {
    var min = parseFloat(inp.min), max = parseFloat(inp.max), val = parseFloat(inp.value);
    var pct = max > min ? ((val - min) / (max - min)) * 100 : 0;
    inp.style.setProperty("--pct", pct.toFixed(2) + "%");
  }
  function paintAllRanges() {
    Array.prototype.forEach.call(document.querySelectorAll('input[type="range"]'), paintRange);
  }
  document.addEventListener("input", function (e) {
    if (e.target && e.target.type === "range") paintRange(e.target);
  });

  // ── voice library ───────────────────────────────────────────────────────────
  // voice = { id, name, engine, flavour, note }. Seeded from the factory flavours (one
  // voice per authored flavour); the per-engine "default" flavour is named for the
  // engine. Users add/edit voices in the Voices tab.
  //
  // `note` is the drum's pitch (MIDI): the sine/struck body tracks it, and — crucially —
  // Metal reads open-vs-closed from note-vs-split (44), so a hat's open/closed identity IS
  // its note (closed < 44 ≤ open). Keyed "engine|name"; falls back to the engine default.
  var NOTE_BY_VOICE = {
    "kick|Kick": 33, "kick|Sub Kick": 26, "kick|Tom": 47, "kick|Conga": 55, "kick|Zap": 64,
    "metal|Metal": 46, "metal|Closed Hat": 38, "metal|Open Hat": 50, "metal|Ride": 50, "metal|Crash": 50,
    "noise|Noise": 54, "noise|Snare": 54, "noise|Clap": 48,
    "struck|Struck": 45, "struck|Kick": 33, "struck|Tom": 47, "struck|Claves": 72, "struck|Cymbal": 60,
  };
  var NOTE_BY_ENGINE = { kick: 36, metal: 46, noise: 54, struck: 45 };
  function noteForVoice(engineId, name) {
    var k = engineId + "|" + name;
    if (NOTE_BY_VOICE[k] != null) return NOTE_BY_VOICE[k];
    return NOTE_BY_ENGINE[engineId] != null ? NOTE_BY_ENGINE[engineId] : 36;
  }
  var voices = [];
  var nextVoiceId = 1;
  ENGINES.forEach(function (e) {
    (e.flavours || []).forEach(function (f) {
      var nm = f.name === "default" ? e.label : f.name;
      voices.push({ id: nextVoiceId++, name: nm, engine: e.id, flavour: cloneFlavour(f), note: noteForVoice(e.id, nm) });
    });
  });
  function voiceById(id) {
    for (var i = 0; i < voices.length; i++) if (voices[i].id === id) return voices[i];
    return voices[0];
  }
  // Displayed name: engine-prefixed (e.g. "Metal · ride").
  function voiceLabel(v) { return engineById(v.engine).label + " · " + v.name; }
  function voiceByName(nm) {
    for (var i = 0; i < voices.length; i++) if (voices[i].name === nm) return voices[i];
    return null;
  }

  // ── lanes (reference a voice) ────────────────────────────────────────────────
  var DEFAULT_LANE = ["Kick", "Closed Hat", "Snare", "Tom", "Clap", "Open Hat", "Ride", "Crash"];
  // Choke groups (0 = none). Closed + Open Hat share group 1 → a closed hit cuts the open
  // ring, the 808 relationship, as a cross-track routing link (not a per-hit note change).
  var DEFAULT_CHOKE = [0, 1, 0, 0, 0, 1, 0, 0];
  var lanes = [];
  for (var t = 0; t < NT; t++) {
    var steps = [];
    for (var s = 0; s < NS; s++) steps.push({ on: false, prob: 1.0, retrig: false });
    var v = voiceByName(DEFAULT_LANE[t]) || voices[t % voices.length] || voices[0];
    lanes.push({ voiceId: v ? v.id : 0, len: NS, steps: steps, choke: DEFAULT_CHOKE[t] || 0 });
  }

  // Assign a voice to a lane: update the reference, tell the backend (engine + the
  // full flavour, self-contained), refresh the lane UI.
  function assignVoice(track, voiceId) {
    lanes[track].voiceId = voiceId;
    var v = voiceById(voiceId);
    // Re-pitch any already-lit steps to the new voice's note (a hat's open/closed identity
    // lives in the note, so reassigning must re-note or the drum plays at the old pitch).
    var st = lanes[track].steps;
    for (var s = 0; s < st.length; s++) {
      if (st[s].on) send("set_step", { track: track, step: s, note: v.note, velocity: 1.0 });
    }
    send("assign_voice", {
      track: track, engine: v.engine,
      base: v.flavour.base, bindings: v.flavour.bindings,
      macro_defaults: v.flavour.macro_defaults, macro_names: v.flavour.macro_names,
    });
    // Snap the track's performance macros to the voice's shipped defaults — the engine keeps
    // live macros across a flavour swap, so without this the voice would sound at whatever the
    // knobs last were (0.5), not its authored point. `set` also sends `set_macro` to the audio.
    var md = v.flavour.macro_defaults || [];
    if (macroKnobEls[track]) {
      for (var m = 0; m < NSLOT; m++) {
        if (macroKnobEls[track][m]) macroKnobEls[track][m].set(md[m] != null ? md[m] : 0.5);
      }
    }
    refreshLane(track);
  }
  // Re-push a voice to every lane using it (after a voice edit) so audio tracks it.
  function reassignLanesUsing(voiceId) {
    for (var t = 0; t < NT; t++) if (lanes[t].voiceId === voiceId) assignVoice(t, voiceId);
  }

  // ── Pattern tab: the rack ────────────────────────────────────────────────────
  var rack = document.getElementById("rack");
  var cellEls = [];       // cellEls[t][s]
  var voiceBoxEls = [];   // voiceBoxEls[t]
  var macroLabelEls = []; // macroLabelEls[t][slot]
  var macroKnobEls = [];  // macroKnobEls[t][slot] — the 3 performance-macro knob handles

  function renderCell(t, s) {
    var el2 = cellEls[t][s], st = lanes[t].steps[s];
    el2.className = "cell" + (s % 4 === 0 ? " beat" : "")
      + (s >= lanes[t].len ? " off" : "")
      + (st.on ? " on" : "")
      + (st.retrig ? " retrig" : "");
    el2.style.opacity = st.on ? String(st.prob) : "";
  }

  // A macro slot's lane label = the assigned voice's macro name (override / first bound
  // param / "M<n>").
  function macroLabel(t, slot) {
    var v = voiceById(lanes[t].voiceId);
    return macroName(v.flavour, engineById(v.engine), slot);
  }
  function refreshLane(t) {
    var v = voiceById(lanes[t].voiceId);
    var eng = engineById(v.engine);
    var box = voiceBoxEls[t];
    box.textContent = v.name; // engine shown by colour, so no prefix here
    box.title = voiceLabel(v);
    box.className = "voice-box " + eng.id;
    for (var slot = 0; slot < NSLOT; slot++) macroLabelEls[t][slot].textContent = macroLabel(t, slot);
  }

  function makeKnob(label, min, max, step, value, oninput) {
    var wrap = el("div", "knob");
    var lab = el("label", null, label);
    var inp = document.createElement("input");
    inp.type = "range"; inp.min = min; inp.max = max; inp.step = step; inp.value = value;
    inp.addEventListener("input", function () { oninput(parseFloat(inp.value)); });
    wrap.appendChild(lab); wrap.appendChild(inp);
    // `set` moves the knob programmatically AND fires its callback (so loading a voice can
    // snap its performance macros to the shipped defaults).
    return { wrap: wrap, label: lab, input: inp, set: function (x) { inp.value = x; oninput(parseFloat(inp.value)); } };
  }

  function buildTrack(t) {
    var row = el("div", "track");

    // Voice box — shows the assigned voice; click opens the voice browser.
    var box = el("button", "voice-box");
    voiceBoxEls[t] = box;
    box.addEventListener("click", function () { openBrowser(t); });
    row.appendChild(box);

    // Steps.
    var stepsEl = el("div", "steps");
    cellEls[t] = [];
    for (var s = 0; s < NS; s++) {
      (function (s) {
        var c = document.createElement("div");
        cellEls[t][s] = c;
        c.addEventListener("mousedown", function (ev) {
          var st = lanes[t].steps[s];
          if (ev.shiftKey && st.on) {
            st.prob = PROBS[(PROBS.indexOf(st.prob) + 1) % PROBS.length];
            send("set_probability", { track: t, step: s, probability: st.prob });
          } else if (ev.altKey && st.on) {
            st.retrig = !st.retrig;
            if (st.retrig) send("set_retrig", { track: t, step: s, n: 4, m: 2, curve: "even", vel_end: 0.4 });
            else send("set_retrig", { track: t, step: s, n: 1, m: 1, curve: "even", vel_end: 1.0 });
          } else {
            st.on = !st.on;
            if (st.on) send("set_step", { track: t, step: s, note: voiceById(lanes[t].voiceId).note, velocity: 1.0 });
            else send("toggle_step", { track: t, step: s });
          }
          renderCell(t, s);
        });
        stepsEl.appendChild(c);
      })(s);
    }
    row.appendChild(stepsEl);

    // Knobs: 3 performance macros (labelled from the voice's bindings) + gain/pan/send/len.
    var knobs = el("div", "knobs");
    macroLabelEls[t] = [];
    macroKnobEls[t] = [];
    for (var slot = 0; slot < NSLOT; slot++) {
      (function (slot) {
        var k = makeKnob("M" + (slot + 1), 0, 1, 0.01, 0.5, function (v) {
          send("set_macro", { track: t, slot: slot, value: v });
        });
        macroLabelEls[t][slot] = k.label;
        macroKnobEls[t][slot] = k;
        knobs.appendChild(k.wrap);
      })(slot);
    }
    knobs.appendChild(makeKnob("Gain", 0, 1.5, 0.01, 1.0, function (v) { send("set_gain", { track: t, gain: v }); }).wrap);
    knobs.appendChild(makeKnob("Pan", -1, 1, 0.01, 0.0, function (v) { send("set_pan", { track: t, pan: v }); }).wrap);
    knobs.appendChild(makeKnob("Send", 0, 1, 0.01, 0.0, function (v) { send("set_send", { track: t, amount: v }); }).wrap);

    var len = el("div", "len");
    len.appendChild(el("label", null, "Len"));
    var li = document.createElement("input");
    li.type = "number"; li.min = 1; li.max = NS; li.value = lanes[t].len;
    li.addEventListener("change", function () {
      var n = Math.max(1, Math.min(NS, parseInt(li.value, 10) || NS));
      lanes[t].len = n; li.value = n;
      send("set_length", { track: t, len: n });
      for (var s = 0; s < NS; s++) renderCell(t, s);
    });
    len.appendChild(li);
    knobs.appendChild(len);

    // Choke group (0 = none). Tracks sharing a non-zero group cut each other.
    var chk = el("div", "len");
    chk.appendChild(el("label", null, "Chk"));
    var ci = document.createElement("input");
    ci.type = "number"; ci.min = 0; ci.max = 7; ci.value = lanes[t].choke;
    ci.title = "choke group (0 = none; shared group = mutual cut)";
    ci.addEventListener("change", function () {
      var g = Math.max(0, Math.min(7, parseInt(ci.value, 10) || 0));
      lanes[t].choke = g; ci.value = g;
      send("set_choke_group", { track: t, group: g });
    });
    chk.appendChild(ci);
    knobs.appendChild(chk);
    row.appendChild(knobs);

    rack.appendChild(row);
    refreshLane(t);
    for (var s2 = 0; s2 < NS; s2++) renderCell(t, s2);
  }
  for (var t2 = 0; t2 < NT; t2++) buildTrack(t2);
  // Push the seeded kit to the backend: each track's default engine is generic, so without
  // this the loaded pattern would play default voices, not the labelled ones. Sends engine +
  // flavour + snaps the macro knobs for every lane.
  for (var t3 = 0; t3 < NT; t3++) assignVoice(t3, lanes[t3].voiceId);
  // Push default choke groups (hat pair share group 1).
  for (var t4 = 0; t4 < NT; t4++) send("set_choke_group", { track: t4, group: lanes[t4].choke });

  (function buildMaster() {
    var m = document.getElementById("master");
    m.appendChild(el("span", "master-label", "DELAY"));
    m.appendChild(makeKnob("Time", 0.125, 1.5, 0.005, 0.75, function (v) { send("set_delay_sync", { beats: v }); }).wrap);
    m.appendChild(makeKnob("Fbk", 0, 1.25, 0.01, 0.5, function (v) { send("set_delay_feedback", { value: v }); }).wrap);
    m.appendChild(makeKnob("Return", 0, 1, 0.01, 0.35, function (v) { send("set_delay_return", { value: v }); }).wrap);
    var lim = el("span", "master-label limiter-on", "LIMITER ●");
    m.appendChild(lim);
  })();
  paintAllRanges(); // initial fill for the rack + master sliders

  // ── Voice browser overlay ────────────────────────────────────────────────────
  var browser = document.getElementById("voice-browser");
  function openBrowser(track) {
    browser.innerHTML = "";
    var head = el("div", "browser-head");
    head.appendChild(el("span", "browser-title", "Assign voice → Track " + (track + 1)));
    var x = el("button", "browser-close", "✕");
    x.addEventListener("click", closeBrowser);
    head.appendChild(x);
    browser.appendChild(head);

    ENGINES.forEach(function (eng) {
      var group = voices.filter(function (v) { return v.engine === eng.id; });
      if (!group.length) return;
      var sec = el("div", "browser-group");
      sec.appendChild(el("div", "browser-legend " + eng.id, eng.label));
      var list = el("div", "browser-voices");
      group.forEach(function (v) {
        var b = el("button", "browser-voice" + (lanes[track].voiceId === v.id ? " sel" : ""), voiceLabel(v));
        b.addEventListener("click", function () { assignVoice(track, v.id); closeBrowser(); });
        list.appendChild(b);
      });
      sec.appendChild(list);
      browser.appendChild(sec);
    });
    browser.classList.remove("hidden");
  }
  function closeBrowser() { browser.classList.add("hidden"); browser.innerHTML = ""; }

  // ── Voices tab: library list + voice editor ──────────────────────────────────
  var voiceListEl = document.getElementById("voice-list");
  var voiceEditorEl = document.getElementById("voice-editor");
  var editingVoiceId = voices.length ? voices[0].id : 0;

  function renderVoiceList() {
    voiceListEl.innerHTML = "";
    var head = el("div", "vl-head");
    head.appendChild(el("span", null, "VOICES"));
    var add = el("button", "vl-new", "＋");
    add.title = "new voice";
    add.addEventListener("click", function () {
      var eng = ENGINES[0];
      var v = { id: nextVoiceId++, name: "new voice", engine: eng.id, flavour: defaultFlavour(eng.id), note: noteForVoice(eng.id, "new voice") };
      voices.push(v); editingVoiceId = v.id; renderVoiceList(); renderVoiceEditor();
    });
    head.appendChild(add);
    voiceListEl.appendChild(head);

    ENGINES.forEach(function (eng) {
      var group = voices.filter(function (v) { return v.engine === eng.id; });
      if (!group.length) return;
      voiceListEl.appendChild(el("div", "vl-legend " + eng.id, eng.label));
      group.forEach(function (v) {
        var b = el("button", "vl-item" + (v.id === editingVoiceId ? " sel" : ""), voiceLabel(v));
        b.addEventListener("click", function () { editingVoiceId = v.id; renderVoiceList(); renderVoiceEditor(); });
        voiceListEl.appendChild(b);
      });
    });
  }

  function renderVoiceEditor() {
    voiceEditorEl.innerHTML = "";
    var v = voiceById(editingVoiceId);
    if (!v) return;
    var eng = engineById(v.engine);
    var params = eng.params || [];

    // Header: name + engine-type selector.
    var head = el("div", "ve-head");
    var nm = document.createElement("input");
    nm.type = "text"; nm.className = "ve-name"; nm.value = v.name;
    nm.addEventListener("change", function () { v.name = nm.value.trim() || v.name; renderVoiceList(); });
    head.appendChild(nm);

    var engs = el("div", "ve-engines");
    ENGINES.forEach(function (e) {
      var b = el("button", "ve-eng " + e.id + (e.id === v.engine ? " active" : ""), e.label);
      b.addEventListener("click", function () {
        if (e.id === v.engine) return;
        v.engine = e.id; v.flavour = defaultFlavour(e.id); // new family → its default params
        reassignLanesUsing(v.id); renderVoiceList(); renderVoiceEditor();
      });
      engs.appendChild(b);
    });
    head.appendChild(engs);

    var dup = el("button", "ve-dup", "Duplicate");
    dup.addEventListener("click", function () {
      var copy = { id: nextVoiceId++, name: v.name + " copy", engine: v.engine, flavour: cloneFlavour(v.flavour) };
      voices.push(copy); editingVoiceId = copy.id; renderVoiceList(); renderVoiceEditor();
    });
    head.appendChild(dup);

    var del = el("button", "ve-del", "Delete");
    del.addEventListener("click", function () {
      if (voices.length <= 1) return;
      voices = voices.filter(function (x) { return x.id !== v.id; });
      editingVoiceId = voices[0].id; renderVoiceList(); renderVoiceEditor();
    });
    head.appendChild(del);
    voiceEditorEl.appendChild(head);

    // Base sliders.
    var flav = v.flavour;
    var baseWrap = el("div", "ve-section");
    baseWrap.appendChild(el("div", "ve-legend", "BASE"));
    var grid = el("div", "base-grid");
    params.forEach(function (p, i) {
      var row = el("div", "base-row");
      var val = el("span", "base-val", fmtVal(flav.base[i], p.unit));
      var inp = document.createElement("input");
      inp.type = "range"; inp.min = p.min; inp.max = p.max;
      inp.step = (p.max - p.min) / 200 || 0.001; inp.value = flav.base[i];
      inp.addEventListener("input", function () {
        flav.base[i] = parseFloat(inp.value);
        val.textContent = fmtVal(flav.base[i], p.unit);
        reassignLanesUsing(v.id);
      });
      row.appendChild(el("label", "base-label", p.name));
      row.appendChild(inp); row.appendChild(val);
      grid.appendChild(row);
    });
    baseWrap.appendChild(grid);
    voiceEditorEl.appendChild(baseWrap);

    // Macro bindings — one block per host macro slot. A macro is renameable and may
    // drive several params, each with its own depth + curve (they sum, additive-from-base).
    var bindWrap = el("div", "ve-section");
    bindWrap.appendChild(el("div", "ve-legend", "MACRO BINDINGS"));
    for (var slot = 0; slot < NSLOT; slot++) {
      (function (slot) {
        var slotEl = el("div", "macro-slot");
        var hdr = el("div", "macro-hdr");
        hdr.appendChild(el("span", "bind-slot", "M" + (slot + 1)));

        var nameInp = document.createElement("input");
        nameInp.type = "text"; nameInp.className = "macro-name";
        nameInp.value = (flav.macro_names[slot] || "");
        nameInp.placeholder = macroName(flav, eng, slot); // derived default
        nameInp.addEventListener("change", function () {
          flav.macro_names[slot] = nameInp.value.trim();
          reassignLanesUsing(v.id); // refresh lane knob labels
        });
        hdr.appendChild(nameInp);

        var add = el("button", "macro-add", "+ param");
        add.title = "bind another param to this macro";
        add.addEventListener("click", function () {
          var used = slotBindings(flav, slot).map(function (b) { return b.param; });
          var pi = 0; while (pi < params.length && used.indexOf(pi) >= 0) pi++;
          if (pi >= params.length) return; // every param already bound
          var sp = params[pi].max - params[pi].min;
          flav.bindings.push({ slot: slot, param: pi, depth: sp / 3, curve: "linear" });
          reassignLanesUsing(v.id); renderVoiceEditor();
        });
        hdr.appendChild(add);
        slotEl.appendChild(hdr);

        slotBindings(flav, slot).forEach(function (b) {
          var row = el("div", "bind-row");
          row.appendChild(el("span", "bind-arrow", "→"));

          var tgt = document.createElement("select");
          tgt.className = "bind-tgt";
          params.forEach(function (p, i) { var o = el("option", null, p.name); o.value = String(i); tgt.appendChild(o); });
          tgt.value = String(b.param);

          var sp = params[b.param].max - params[b.param].min;
          var depth = document.createElement("input");
          depth.type = "range"; depth.min = -sp; depth.max = sp; depth.step = sp / 100 || 0.01; depth.value = b.depth;

          var curve = document.createElement("select");
          curve.className = "bind-curve";
          CURVES.forEach(function (c) { curve.appendChild(el("option", null, c)); });
          curve.value = b.curve;

          var rm = el("button", "bind-rm", "✕");
          rm.title = "remove binding";

          tgt.addEventListener("change", function () {
            b.param = parseInt(tgt.value, 10);
            reassignLanesUsing(v.id); renderVoiceEditor(); // rescale depth + refresh default name
          });
          depth.addEventListener("input", function () { b.depth = parseFloat(depth.value); reassignLanesUsing(v.id); });
          curve.addEventListener("change", function () { b.curve = curve.value; reassignLanesUsing(v.id); });
          rm.addEventListener("click", function () {
            flav.bindings = flav.bindings.filter(function (x) { return x !== b; });
            reassignLanesUsing(v.id); renderVoiceEditor();
          });

          row.appendChild(tgt); row.appendChild(depth); row.appendChild(curve); row.appendChild(rm);
          slotEl.appendChild(row);
        });

        bindWrap.appendChild(slotEl);
      })(slot);
    }
    voiceEditorEl.appendChild(bindWrap);
    paintAllRanges();
  }
  renderVoiceList();
  renderVoiceEditor();

  // ── tabs ──────────────────────────────────────────────────────────────────
  var tabviews = { pattern: document.getElementById("tab-pattern"), voices: document.getElementById("tab-voices") };
  Array.prototype.forEach.call(document.querySelectorAll("#tabs .tab"), function (tb) {
    tb.addEventListener("click", function () {
      Array.prototype.forEach.call(document.querySelectorAll("#tabs .tab"), function (o) { o.classList.remove("active"); });
      tb.classList.add("active");
      for (var k in tabviews) tabviews[k].classList.toggle("hidden", k !== tb.dataset.tab);
    });
  });

  // ── playhead + view-event sink ──────────────────────────────────────────────
  var lastPlay = new Array(NT).fill(-1);
  function setPlay(t, step) {
    if (lastPlay[t] >= 0 && cellEls[t][lastPlay[t]]) cellEls[t][lastPlay[t]].classList.remove("play");
    if (step >= 0 && step < NS && cellEls[t][step]) cellEls[t][step].classList.add("play");
    lastPlay[t] = step;
  }
  var transport = document.getElementById("transport");
  window.__vxn = window.__vxn || {};
  window.__vxn.applyViewEvents = function (events) {
    for (var i = 0; i < events.length; i++) {
      var ev = events[i];
      if (ev.kind === "playhead") {
        transport.textContent = ev.playing ? "▶ playing" : "■ stopped";
        transport.classList.toggle("playing", !!ev.playing);
        for (var t = 0; t < NT; t++) {
          var step = ev.steps[t];
          setPlay(t, (step === 4294967295 || !ev.playing) ? -1 : step);
        }
      }
    }
  };
})();
