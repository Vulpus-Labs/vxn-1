// VXN3 faceplate. Structured grid edits → IPC ops; playhead ← view events.
(function () {
  "use strict";

  var CFG = window.__VXN3_CONFIG__ || { tracks: 8, steps: 16, engines: [] };
  var NT = CFG.tracks, NS = CFG.steps;
  var ENGINES = CFG.engines; // [{id:"kick",label:"Kick"}, ...]
  var PROBS = [1.0, 0.75, 0.5, 0.25]; // shift-click cycle

  function send(op, extra) {
    var msg = Object.assign({ op: op }, extra || {});
    try { window.ipc.postMessage(JSON.stringify(msg)); }
    catch (e) { /* host without ipc (preview) */ }
  }

  // Per-track UI mirror (backend is source of truth, but we render locally).
  var state = [];
  for (var t = 0; t < NT; t++) {
    var steps = [];
    for (var s = 0; s < NS; s++) steps.push({ on: false, prob: 1.0, retrig: false });
    state.push({ engine: ENGINES.length ? ENGINES[0].id : "kick", len: NS, steps: steps });
  }

  var rack = document.getElementById("rack");
  var cellEls = []; // cellEls[t][s]

  function makeKnob(label, min, max, step, value, oninput) {
    var wrap = document.createElement("div"); wrap.className = "knob";
    var lab = document.createElement("label"); lab.textContent = label;
    var inp = document.createElement("input");
    inp.type = "range"; inp.min = min; inp.max = max; inp.step = step; inp.value = value;
    inp.addEventListener("input", function () { oninput(parseFloat(inp.value)); });
    wrap.appendChild(lab); wrap.appendChild(inp);
    return wrap;
  }

  function renderCell(t, s) {
    var el = cellEls[t][s], st = state[t].steps[s];
    el.className = "cell" + (s % 4 === 0 ? " beat" : "")
      + (s >= state[t].len ? " off" : "")
      + (st.on ? " on" : "")
      + (st.retrig ? " retrig" : "");
    el.style.opacity = st.on ? String(st.prob) : "";
  }

  function buildTrack(t) {
    var row = document.createElement("div"); row.className = "track";

    // Engine selector.
    var sel = document.createElement("div"); sel.className = "engine-sel";
    ENGINES.forEach(function (eng) {
      var b = document.createElement("button");
      b.textContent = eng.label; b.className = eng.id;
      if (state[t].engine === eng.id) b.classList.add("active");
      b.addEventListener("click", function () {
        state[t].engine = eng.id;
        Array.prototype.forEach.call(sel.children, function (c) { c.classList.remove("active"); });
        b.classList.add("active");
        send("set_engine", { track: t, kind: eng.id });
      });
      sel.appendChild(b);
    });
    row.appendChild(sel);

    // Steps.
    var steps = document.createElement("div"); steps.className = "steps";
    cellEls[t] = [];
    for (var s = 0; s < NS; s++) {
      (function (s) {
        var c = document.createElement("div");
        cellEls[t][s] = c;
        c.addEventListener("mousedown", function (ev) {
          var st = state[t].steps[s];
          if (ev.shiftKey && st.on) {
            // cycle probability
            var i = (PROBS.indexOf(st.prob) + 1) % PROBS.length;
            st.prob = PROBS[i];
            send("set_probability", { track: t, step: s, probability: st.prob });
          } else if (ev.altKey && st.on) {
            st.retrig = !st.retrig;
            if (st.retrig) send("set_retrig", { track: t, step: s, n: 4, m: 2, curve: "even", vel_end: 0.4 });
            else send("set_retrig", { track: t, step: s, n: 1, m: 1, curve: "even", vel_end: 1.0 });
          } else {
            st.on = !st.on;
            if (st.on) send("set_step", { track: t, step: s, note: 36.0, velocity: 1.0 });
            else send("toggle_step", { track: t, step: s }); // clears (was on)
          }
          renderCell(t, s);
        });
        steps.appendChild(c);
        renderCell(t, s);
      })(s);
    }
    row.appendChild(steps);

    // Knobs + length.
    var knobs = document.createElement("div"); knobs.className = "knobs";
    // Three generic macro slots (0/1/2). The active engine reinterprets each
    // onto its patch (ADR 0003 §2); labels here are the generic slot roles.
    knobs.appendChild(makeKnob("Decay", 0, 1, 0.01, 0.5, function (v) {
      send("set_macro", { track: t, slot: 0, value: v });
    }));
    knobs.appendChild(makeKnob("Tone", 0, 1, 0.01, 0.5, function (v) {
      send("set_macro", { track: t, slot: 1, value: v });
    }));
    knobs.appendChild(makeKnob("Pitch", 0, 1, 0.01, 0.5, function (v) {
      send("set_macro", { track: t, slot: 2, value: v });
    }));
    knobs.appendChild(makeKnob("Gain", 0, 1.5, 0.01, 1.0, function (v) {
      send("set_gain", { track: t, gain: v });
    }));
    knobs.appendChild(makeKnob("Pan", -1, 1, 0.01, 0.0, function (v) {
      send("set_pan", { track: t, pan: v });
    }));
    knobs.appendChild(makeKnob("Send", 0, 1, 0.01, 0.0, function (v) {
      send("set_send", { track: t, amount: v });
    }));
    var len = document.createElement("div"); len.className = "len";
    var ll = document.createElement("label"); ll.textContent = "Len";
    var li = document.createElement("input");
    li.type = "number"; li.min = 1; li.max = NS; li.value = state[t].len;
    li.addEventListener("change", function () {
      var n = Math.max(1, Math.min(NS, parseInt(li.value, 10) || NS));
      state[t].len = n; li.value = n;
      send("set_length", { track: t, len: n });
      for (var s = 0; s < NS; s++) renderCell(t, s);
    });
    len.appendChild(ll); len.appendChild(li);
    knobs.appendChild(len);
    row.appendChild(knobs);

    rack.appendChild(row);
  }

  for (var t2 = 0; t2 < NT; t2++) buildTrack(t2);

  // Master strip: the dub delay + (always-on) limiter.
  (function buildMaster() {
    var m = document.getElementById("master");
    var label = document.createElement("span");
    label.className = "master-label";
    label.textContent = "DELAY";
    m.appendChild(label);
    m.appendChild(makeKnob("Time", 0.125, 1.5, 0.005, 0.75, function (v) {
      send("set_delay_sync", { beats: v });
    }));
    m.appendChild(makeKnob("Fbk", 0, 1.25, 0.01, 0.5, function (v) {
      send("set_delay_feedback", { value: v });
    }));
    m.appendChild(makeKnob("Return", 0, 1, 0.01, 0.35, function (v) {
      send("set_delay_return", { value: v });
    }));
    var lim = document.createElement("span");
    lim.className = "master-label limiter-on";
    lim.textContent = "LIMITER ●";
    m.appendChild(lim);
  })();

  // Playhead + view-event sink (the core calls window.__vxn.applyViewEvents).
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
      // other kinds (status, preset_*) ignored in the MVP faceplate
    }
  };
})();
