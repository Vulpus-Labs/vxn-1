// VXN3 faceplate. Voice-library model: named voices (engine + flavour) live in a
// library and are edited in the Voices tab; lanes reference a voice. Structured edits
// → IPC ops; playhead ← view events.
//
// The pattern surface is the continuous lane strip of ADR 0007 §1 (0353): one
// rectangular strip per track, X time and Y a modulation value, hits as freely
// draggable diamonds. The grid is **drawn, not stored into** — a diamond's
// position is the engine's (beat, sub, f, nudge) resolved for display through the
// same geometry the engine fires it at.
(function () {
  "use strict";

  var CFG = window.__VXN3_CONFIG__ || { tracks: 8, lanes: [], engines: [], macro_slots: 3 };
  var NT = CFG.tracks;
  var ENGINES = CFG.engines; // [{id,label,params:[...],flavours:[...]}]
  var NSLOT = CFG.macro_slots || 3;
  // Engine-side limits, shipped rather than duplicated: the editor enforces the
  // hit ceiling itself so the user sees it, instead of an over-capacity add being
  // dropped silently on the audio thread.
  var MAX_HITS = CFG.max_hits || 64;
  var MAX_BEATS = CFG.max_beats || 16;
  var MAX_SUBS = CFG.max_subs || 16;
  var TICKS_PER_BEAT = CFG.ticks_per_beat || 30720;
  var MAX_NUDGE_TICKS = CFG.max_nudge_ticks || 240;
  var Y_CENTRE = CFG.y_centre != null ? CFG.y_centre : 0.5;
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
  function clamp(x, lo, hi) { return x < lo ? lo : x > hi ? hi : x; }
  function pct(x) { return (x * 100).toFixed(4) + "%"; }
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

  // Fill indicator: set each range's `--pct` (thumb position) so the CSS track shows
  // the green→orange→red gradient up to the thumb and grey beyond.
  function paintRange(inp) {
    var min = parseFloat(inp.min), max = parseFloat(inp.max), val = parseFloat(inp.value);
    var p = max > min ? ((val - min) / (max - min)) * 100 : 0;
    inp.style.setProperty("--pct", p.toFixed(2) + "%");
  }
  function paintAllRanges() {
    Array.prototype.forEach.call(document.querySelectorAll('input[type="range"]'), paintRange);
  }
  document.addEventListener("input", function (e) {
    if (e.target && e.target.type === "range") paintRange(e.target);
  });

  // ── lane geometry: a port of the engine's grid.rs ────────────────────────────
  // The strip has to place a marker, and there is no wasm build to ask, so the
  // geometry is ported rather than approximated. Every X below comes from
  // `subPos`; **none** comes from a step width times an index. That is not
  // fastidiousness — 0365 applies the swing warp per *pair*, so one beat at swing
  // runs long-short-long-short and any pixels-per-step assumption is simply wrong.
  //
  // The port is exact, not merely close: the page and the engine agree on a hit's
  // resolved time bit for bit (`Math.fround` where the engine stores an `f32`),
  // which is what lets the page mirror the engine's fire-order indices instead of
  // reading them back over a channel that does not exist.

  var MPC_MAX_RATIO = 0.75;   // Swing's knee ceiling
  var MIN_SLOT = 1 / 64;      // grid.rs MIN_SLOT
  var F_MAX = Math.fround(1 - 1.1920928955078125e-7); // 1 - f32::EPSILON
  var f32 = Math.fround;      // one `f32` rounding, where the engine stores one
  // Rust's `f32::round` breaks ties away from zero; `Math.round` breaks them
  // toward +∞. Only `nudge` is ever rounded, and only in the quantise path, but a
  // half-tick disagreement there is a fire-order disagreement.
  function roundTiesAway(x) { return x < 0 ? -Math.round(-x) : Math.round(x); }

  // Swing::w — one knee on [0, 1], monotonic with fixed endpoints.
  function warp(sw, u) {
    if (!(u > 0)) return 0; // <= 0 and NaN
    if (u >= 1) return 1;
    if (sw.shape !== 1 || !isFinite(sw.amount)) return u; // Straight
    var a = clamp(sw.amount, -1, 1);
    var s = 0.5 + a * (MPC_MAX_RATIO - 0.5);
    return u <= 0.5 ? (u + u) * s : s + (u + u - 1) * (1 - s);
  }
  // SwingPeriod::subs — the interval the warp spans, in subdivisions. Tag 0 is the
  // whole beat; any other tag is that many subdivisions (2 = the classic pair).
  function periodSubs(sw, n) {
    var c = sw.period === 0 ? n : sw.period;
    return clamp(c, 1, Math.max(n, 1));
  }
  function subsAt(g, beat) { return g.subs[clamp(beat, 0, g.n_beats - 1)]; }

  // Grid::sub_pos — position in beats of subdivision marker `k` of `beat`.
  function subPos(g, beat, k) {
    var b = Math.min(beat, g.n_beats - 1);
    var n = subsAt(g, b);
    if (k === 0) return g.markers[b];          // the beat marker itself, exactly
    if (k >= n) return g.markers[b + 1];       // the next one, exactly
    var lo = g.markers[b], hi = g.markers[b + 1];
    var c = periodSubs(g.swing, n);
    var g0 = Math.floor(k / c) * c;            // first subdivision of k's period
    var width = Math.min(c, n - g0);           // a trailing period can be short
    return lo + ((g0 + width * warp(g.swing, (k - g0) / width)) / n) * (hi - lo);
  }
  function totalSubs(g) {
    var n = 0;
    for (var b = 0; b < g.n_beats; b++) n += g.subs[b];
    return n;
  }
  function subOfIndex(g, index) {
    var rem = index;
    for (var b = 0; b < g.n_beats; b++) {
      if (rem < g.subs[b]) return { beat: b, sub: rem };
      rem -= g.subs[b];
    }
    var last = g.n_beats - 1;
    return { beat: last, sub: g.subs[last] - 1 };
  }
  // Grid::locate — resolve a beat position to its owning (beat, sub, frac).
  function locate(g, t) {
    var last = g.n_beats - 1;
    if (!(t > g.markers[0])) return { beat: 0, sub: 0, frac: 0 };
    if (t >= g.markers[g.n_beats]) return { beat: last, sub: subsAt(g, last) - 1, frac: 1 };
    var beat = last;
    for (var i = 0; i < g.n_beats; i++) {
      if (t < g.markers[i + 1]) { beat = i; break; }
    }
    var sub = 0;
    for (var k = subsAt(g, beat) - 1; k >= 0; k--) {
      if (subPos(g, beat, k) <= t) { sub = k; break; }
    }
    var p0 = subPos(g, beat, sub), span = subPos(g, beat, sub + 1) - p0;
    return { beat: beat, sub: sub, frac: span > 0 ? clamp((t - p0) / span, 0, 1) : 0 };
  }
  // The snap-target set is exactly the subdivision markers (ADR 0007 §2), beat
  // markers included. The pattern *end* is not among them: a hit there would have
  // no owning slot, which is also why `quantise_x` never moves one off the end.
  function nearestMarker(g, t) {
    var best = { beat: 0, sub: 0 }, bd = Infinity;
    for (var b = 0; b < g.n_beats; b++) {
      for (var k = 0; k < g.subs[b]; k++) {
        var d = Math.abs(subPos(g, b, k) - t);
        if (d < bd) { bd = d; best = { beat: b, sub: k }; }
      }
    }
    return best;
  }

  function saneLen(len, n) {
    var floor = n * MIN_SLOT;
    return (isFinite(len) && len > floor) ? len : floor;
  }
  // Pattern::set_grid_beats — `set_n_beats` (uniform over the current length) then
  // `set_len_beats(n)`. Ported step for step, rather than shortcut to `m[i] = i`,
  // so the page's markers are the engine's markers and not a rounding away.
  function relayoutBeats(g, nBeats) {
    var n = clamp(Math.round(nBeats), 1, MAX_BEATS);
    var len0 = saneLen(g.markers[g.n_beats], n);
    var step = len0 / n, m = [];
    for (var i = 0; i < n; i++) m.push(i * step);
    m.push(len0);
    var len = saneLen(n, n);
    if (len0 > 0) {
      var k = len / len0;
      for (var j = 1; j < n; j++) m[j] *= k;
    }
    m[n] = len;
    g.markers = m;
    g.n_beats = n;
    g.len_beats = len;
    // Overrides past the new end are dropped, not parked: a shrink must not leave
    // a value that springs back on a later grow.
    var ov = [], subs = [];
    for (var b = 0; b < n; b++) {
      var o = g.sub_override[b] || 0;
      ov.push(o);
      subs.push(o || g.default_subs);
    }
    g.sub_override = ov;
    g.subs = subs;
  }
  function setDefaultSubs(g, subs) {
    g.default_subs = clamp(Math.round(subs), 1, MAX_SUBS);
    for (var b = 0; b < g.n_beats; b++) g.subs[b] = g.sub_override[b] || g.default_subs;
  }

  // ── the hit list, in fire order ─────────────────────────────────────────────
  // The page mirrors the engine's ordering exactly, because every hit-keyed edit
  // names a hit by its fire-order index and there is no readback. Both sides sort
  // on the same key, computed by the same arithmetic, with the same insertion
  // rule — see the note on the geometry port above.
  //
  // KNOWN GAP, and it is the mirror's one weak point: the page is seeded empty
  // and from `build_html`'s default geometry, so it mirrors an engine that starts
  // empty. Reopening the editor over a lane that already holds hits — a GUI
  // close/reopen, or a `clap.state` restore — rebuilds this list from nothing
  // while the engine keeps its own, and every index sent afterwards then names
  // the wrong hit. The fix is a hit-list readback in `serialise_custom_view`,
  // which needs its own ticket: the view channel carries only the playhead today,
  // and the alternative (clearing the engine's lanes on load) would throw away a
  // restored pattern to buy agreement.

  function canonicalHit(g, h) {
    var b = clamp(h.beat | 0, 0, g.n_beats - 1);
    var k = clamp(h.sub | 0, 0, subsAt(g, b) - 1);
    h.beat = b;
    h.sub = k;
    // Narrow to `f32` **before** clamping, in that order: the engine receives an
    // already-narrowed value and clamps that, and `fround` of a hair under 1 is 1,
    // which the clamp would then have to catch rather than let through.
    var f = Math.fround(h.f);
    h.f = isFinite(f) ? clamp(f, 0, F_MAX) : 0;
    h.nudge = clamp(roundTiesAway(h.nudge || 0), -MAX_NUDGE_TICKS, MAX_NUDGE_TICKS);
    return h;
  }
  // Pattern::fire_beat — the resolved position, which is also where the diamond
  // is drawn. One mapping for both, so a drag across a marker cannot make the hit
  // jump: the picture is the model.
  function fireBeat(g, h) {
    var p0 = subPos(g, h.beat, h.sub);
    var span = subPos(g, h.beat, h.sub + 1) - p0;
    var half = 0.5 * span;
    var nudge = clamp(h.nudge / TICKS_PER_BEAT, -half, half);
    var t = h.f <= 0 ? p0 + nudge : p0 + h.f * span + nudge;
    return clamp(t, 0, g.len_beats);
  }
  // Insert in fire order, returning the index — or -1 at the ceiling, which the
  // caller turns into visible feedback rather than a silent drop.
  function insertHit(lane, h) {
    if (lane.hits.length >= MAX_HITS) return -1;
    canonicalHit(lane.g, h);
    var t = fireBeat(lane.g, h), j = lane.hits.length;
    while (j > 0 && fireBeat(lane.g, lane.hits[j - 1]) > t) {
      lane.hits[j] = lane.hits[j - 1];
      j--;
    }
    lane.hits[j] = h;
    return j;
  }
  // Pattern::canonicalise — clamp every hit into the current geometry and
  // re-establish fire order. Run after a geometry edit, which moves all of them.
  function canonicaliseLane(lane) {
    var g = lane.g, hits = lane.hits, key = [];
    for (var i = 0; i < hits.length; i++) key[i] = fireBeat(g, canonicalHit(g, hits[i]));
    for (var a = 1; a < hits.length; a++) {
      var kk = key[a], hh = hits[a], j = a;
      while (j > 0 && key[j - 1] > kk) {
        key[j] = key[j - 1];
        hits[j] = hits[j - 1];
        j--;
      }
      key[j] = kk;
      hits[j] = hh;
    }
  }
  // Pattern::set_position — remove and re-insert, so the new index falls out of
  // the same code that keeps the order.
  function moveHit(lane, index, beat, sub, f, nudge) {
    var h = lane.hits[index];
    if (!h) return -1;
    h.beat = beat; h.sub = sub; h.f = f; h.nudge = nudge;
    lane.hits.splice(index, 1);
    return insertHit(lane, h);
  }
  // Pattern::next_marker — the marker after (beat, sub), or null at the lane's
  // last one: the end marker is a bound, not a slot, so there is nothing beyond
  // it to quantise or snap to.
  function nextMarker(g, beat, sub) {
    var b = Math.min(beat, g.n_beats - 1), k = Math.min(sub, subsAt(g, b) - 1);
    if (k + 1 < subsAt(g, b)) return { beat: b, sub: k + 1 };
    if (b + 1 < g.n_beats) return { beat: b + 1, sub: 0 };
    return null;
  }

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

  // ── lanes (geometry + hits + a voice reference) ──────────────────────────────
  var DEFAULT_LANE = ["Kick", "Closed Hat", "Snare", "Tom", "Clap", "Open Hat", "Ride", "Crash"];
  // Choke groups (0 = none). Closed + Open Hat share group 1 → a closed hit cuts the open
  // ring, the 808 relationship, as a cross-track routing link (not a per-hit note change).
  var DEFAULT_CHOKE = [0, 1, 0, 0, 0, 1, 0, 0];
  var FALLBACK_GRID = {
    n_beats: 4, len_beats: 4, markers: [0, 1, 2, 3, 4], subs: [4, 4, 4, 4],
    sub_override: [0, 0, 0, 0], default_subs: 4, swing: { shape: 0, amount: 0, period: 2 },
  };
  function laneGeometry(t) {
    var src = (CFG.lanes && CFG.lanes[t]) || FALLBACK_GRID;
    return {
      n_beats: src.n_beats,
      len_beats: src.len_beats,
      markers: src.markers.slice(),
      subs: src.subs.slice(),
      sub_override: (src.sub_override || []).slice(),
      default_subs: src.default_subs,
      swing: { shape: src.swing.shape, amount: src.swing.amount, period: src.swing.period },
    };
  }
  var lanes = [];
  for (var t = 0; t < NT; t++) {
    var v0 = voiceByName(DEFAULT_LANE[t]) || voices[t % voices.length] || voices[0];
    lanes.push({
      voiceId: v0 ? v0.id : 0,
      g: laneGeometry(t),
      hits: [],
      choke: DEFAULT_CHOKE[t] || 0,
    });
  }

  // Assign a voice to a lane: update the reference, tell the backend (engine + the
  // full flavour, self-contained), refresh the lane UI.
  function assignVoice(track, voiceId) {
    lanes[track].voiceId = voiceId;
    var v = voiceById(voiceId);
    // Re-pitch every hit to the new voice's note (a hat's open/closed identity lives
    // in the note, so reassigning must re-note or the drum plays at the old pitch).
    // Only the hits that actually change: this runs on every `input` of a voice
    // editor slider, and a full lane's worth of no-op commands per slider sample
    // would fill the edit ring and start dropping the edits that *do* matter.
    var hits = lanes[track].hits;
    for (var i = 0; i < hits.length; i++) {
      if (hits[i].note === v.note) continue;
      hits[i].note = v.note;
      send("set_hit_note", { track: track, hit: i, note: v.note, velocity: hits[i].velocity });
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
  var stripEls = [];      // stripEls[t] — the lane strip
  var markerEls = [];     // markerEls[t] — its marker layer
  var hitLayerEls = [];   // hitLayerEls[t] — its diamond layer
  var playEls = [];       // playEls[t] — its playhead line
  var voiceBoxEls = [];   // voiceBoxEls[t]
  var macroLabelEls = []; // macroLabelEls[t][slot]
  var macroKnobEls = [];  // macroKnobEls[t][slot] — the 3 performance-macro knob handles

  // Selection holds hit **objects**, not indices: every position edit re-sorts the
  // lane, so an index is only valid until the next drag.
  var selection = [];
  var snapOn = true;
  var quantAmount = 1.0;
  var statusEl = null;

  function isSelected(h) { return selection.indexOf(h) >= 0; }
  function setStatus(msg) { if (statusEl) statusEl.textContent = msg || ""; }

  function renderHits(t) {
    var lane = lanes[t], layer = hitLayerEls[t];
    layer.innerHTML = "";
    for (var i = 0; i < lane.hits.length; i++) {
      var h = lane.hits[i];
      var d = el("div", "hit" + (h.retrig ? " retrig" : "") + (isSelected(h) ? " sel" : ""));
      d.style.left = pct(fireBeat(lane.g, h) / lane.g.len_beats);
      d.style.top = pct(1 - clamp(h.y, 0, 1));
      // Probability fades the diamond, but off a floor: opacity takes the
      // selection ring with it, and a hollow low-probability hit at full fade is
      // invisible against the strip.
      d.style.opacity = (0.4 + 0.6 * clamp(h.prob, 0, 1)).toFixed(3);
      // Welded hits read differently from placed ones — `f = 0` is the stored form
      // that survives a groove edit, and it is worth being able to see which is which.
      if (h.f === 0 && h.nudge === 0) d.classList.add("welded");
      layer.appendChild(d);
    }
  }
  function renderMarkers(t) {
    var lane = lanes[t], g = lane.g, layer = markerEls[t];
    layer.innerHTML = "";
    for (var b = 0; b < g.n_beats; b++) {
      for (var k = 0; k < g.subs[b]; k++) {
        var m = el("div", "marker " + (k === 0 ? "beat" : "sub"));
        m.style.left = pct(subPos(g, b, k) / g.len_beats);
        layer.appendChild(m);
      }
    }
    var end = el("div", "marker beat end");
    end.style.left = "100%";
    layer.appendChild(end);
  }
  function renderLaneStrip(t) {
    renderMarkers(t);
    renderHits(t);
  }

  // The ceiling, shown rather than swallowed: the engine's insert drops an
  // over-capacity hit, so the editor refuses first and says so.
  var fullTimers = [];
  function laneFull(t) {
    var s = stripEls[t];
    s.classList.add("full");
    setStatus("Track " + (t + 1) + " is full — " + MAX_HITS + " hits is the lane ceiling.");
    // One timer per lane, restarted: a second refusal inside the window must
    // extend the flash, not cancel it and clear a message still being read.
    if (fullTimers[t]) window.clearTimeout(fullTimers[t]);
    fullTimers[t] = window.setTimeout(function () {
      s.classList.remove("full");
      fullTimers[t] = 0;
      setStatus("");
    }, 900);
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
  function makeNumber(label, title, min, max, value, onchange) {
    var wrap = el("div", "len");
    wrap.appendChild(el("label", null, label));
    var inp = document.createElement("input");
    inp.type = "number"; inp.min = min; inp.max = max; inp.value = value; inp.title = title;
    inp.addEventListener("change", function () {
      var n = clamp(parseInt(inp.value, 10) || min, min, max);
      inp.value = n;
      onchange(n);
    });
    wrap.appendChild(inp);
    return wrap;
  }

  // ── strip interaction: place, drag, delete ──────────────────────────────────
  // A pointer position, resolved through the lane's geometry rather than through
  // any notion of a cell: `x` is a beat position, `y` is the lane's modulation axis.
  function pointerAt(t, ev) {
    var s = stripEls[t], r = s.getBoundingClientRect();
    // The **padding** box, not the border box: the marker and diamond layers are
    // `inset: 0` inside the strip's 1px border, so measuring the pointer against
    // the outer box would offset placement from what is drawn by that border.
    var w = s.clientWidth || r.width, h = s.clientHeight || r.height;
    var u = clamp((ev.clientX - r.left - (s.clientLeft || 0)) / Math.max(w, 1), 0, 1);
    var y = clamp((ev.clientY - r.top - (s.clientTop || 0)) / Math.max(h, 1), 0, 1);
    return { t: u * lanes[t].g.len_beats, y: f32(1 - y) };
  }
  // Where a drag or a placement lands, as the stored form. With snap on the hit is
  // **welded** to the nearest marker (`f = 0`, no nudge), which is the form that
  // survives a later groove edit; with snap off the position is expressed as a
  // fraction of the slot the pointer is over, so crossing a beat marker changes
  // `(beat, sub)` and recomputes `f` in one step and the diamond does not jump.
  function positionFor(g, beatPos) {
    if (snapOn) {
      var m = nearestMarker(g, beatPos);
      return { beat: m.beat, sub: m.sub, f: 0, nudge: 0 };
    }
    var at = locate(g, beatPos);
    return { beat: at.beat, sub: at.sub, f: at.frac, nudge: 0 };
  }

  var drag = null; // { track, hit, moved }

  function onStripDown(t, ev) {
    var lane = lanes[t];
    var idx = -1;
    if (ev.target && ev.target.classList.contains("hit")) {
      idx = Array.prototype.indexOf.call(hitLayerEls[t].children, ev.target);
    }
    ev.preventDefault();
    if (idx < 0) {
      // Empty strip: place a hit where the pointer is.
      var p = pointerAt(t, ev);
      var pos = positionFor(lane.g, p.t);
      var v = voiceById(lane.voiceId);
      var h = {
        beat: pos.beat, sub: pos.sub, f: pos.f, nudge: pos.nudge, y: p.y,
        note: v.note, velocity: 1.0, prob: 1.0, retrig: false,
      };
      var at = insertHit(lane, h);
      if (at < 0) { laneFull(t); return; }
      send("add_hit", {
        track: t, beat: h.beat, sub: h.sub, f: h.f, nudge: h.nudge,
        y: h.y, note: h.note, velocity: h.velocity,
      });
      selection = [h];
      renderAllHits();
      setStatus("");
      return;
    }
    var hit = lane.hits[idx];
    if (ev.altKey) {
      send("remove_hit", { track: t, hit: idx });
      lane.hits.splice(idx, 1);
      selection = selection.filter(function (x) { return x !== hit; });
      renderHits(t);
      return;
    }
    if (ev.ctrlKey || ev.metaKey) {
      hit.prob = PROBS[(PROBS.indexOf(hit.prob) + 1) % PROBS.length];
      send("set_hit_probability", { track: t, hit: idx, probability: hit.prob });
      renderHits(t);
      return;
    }
    if (ev.shiftKey) {
      if (isSelected(hit)) selection = selection.filter(function (x) { return x !== hit; });
      else selection.push(hit);
    } else if (!isSelected(hit)) {
      selection = [hit];
    }
    drag = { track: t, hit: hit };
    renderAllHits();
  }

  function onDragMove(ev) {
    if (!drag) return;
    var t = drag.track, lane = lanes[t];
    var idx = lane.hits.indexOf(drag.hit);
    if (idx < 0) { drag = null; return; }
    var p = pointerAt(t, ev);
    var pos = positionFor(lane.g, p.t);
    var y = p.y;
    var moved = pos.beat !== drag.hit.beat || pos.sub !== drag.hit.sub
      || f32(pos.f) !== drag.hit.f || y !== drag.hit.y;
    if (!moved) return;
    // Y first: it cannot reorder the lane, so it is keyed by the index the X edit
    // is about to invalidate.
    if (y !== drag.hit.y) {
      drag.hit.y = y;
      send("set_hit_y", { track: t, hit: idx, y: y });
    }
    if (pos.beat !== drag.hit.beat || pos.sub !== drag.hit.sub || f32(pos.f) !== drag.hit.f) {
      send("set_hit_position", {
        track: t, hit: idx, beat: pos.beat, sub: pos.sub, f: pos.f, nudge: pos.nudge,
      });
      moveHit(lane, idx, pos.beat, pos.sub, pos.f, pos.nudge);
    }
    renderHits(t);
  }
  function onDragUp() { drag = null; }

  function renderAllHits() {
    for (var t = 0; t < NT; t++) renderHits(t);
  }

  // ── quantise: two independent verbs, applied to the selection ───────────────
  // Editor verbs, not storage constraints (ADR 0007 §1). The page applies the same
  // arithmetic the engine does so the two lists stay in step; a full quantise-X
  // lands the hit **on** its marker (`f = 0`, no nudge) rather than at `f ≈ 1`
  // against the marker before it, because only the former welds.
  // `f32` at every step, because the engine's arithmetic is `f32` at every step
  // and the two lists have to agree on a fire time to agree on an index.
  function quantiseSelectionX(amount) {
    var a = f32(clamp(amount, 0, 1));
    for (var t = 0; t < NT; t++) {
      var lane = lanes[t];
      for (var s = 0; s < selection.length; s++) {
        var h = selection[s];
        var idx = lane.hits.indexOf(h);
        if (idx < 0) continue;
        send("quantise_hit_x", { track: t, hit: idx, amount: a });
        var next = nextMarker(lane.g, h.beat, h.sub);
        var towardNext = h.f > 0.5 && next !== null;
        if (a >= 1) {
          var m = towardNext ? next : { beat: h.beat, sub: h.sub };
          moveHit(lane, idx, m.beat, m.sub, 0, 0);
        } else {
          var f = towardNext
            ? f32(h.f + f32(f32(1 - h.f) * a))
            : f32(h.f * f32(1 - a));
          moveHit(lane, idx, h.beat, h.sub, f, roundTiesAway(f32(h.nudge * f32(1 - a))));
        }
      }
      renderHits(t);
    }
  }
  function quantiseSelectionY(amount) {
    var a = f32(clamp(amount, 0, 1));
    for (var t = 0; t < NT; t++) {
      var lane = lanes[t];
      for (var s = 0; s < selection.length; s++) {
        var h = selection[s];
        var idx = lane.hits.indexOf(h);
        if (idx < 0) continue;
        send("quantise_hit_y", { track: t, hit: idx, amount: a });
        h.y = f32(h.y + f32(f32(Y_CENTRE - h.y) * a)); // the curve is flat until 0350
      }
      renderHits(t);
    }
  }

  function buildRackBar() {
    var bar = el("div", "rackbar");
    var snapWrap = el("label", "rb-toggle");
    var snapBox = document.createElement("input");
    snapBox.type = "checkbox";
    snapBox.checked = snapOn;
    snapBox.addEventListener("change", function () { snapOn = snapBox.checked; });
    snapWrap.appendChild(snapBox);
    snapWrap.appendChild(el("span", null, "Snap"));
    snapWrap.title = "drop a diamond welded to the nearest subdivision marker";
    bar.appendChild(snapWrap);

    var amt = el("div", "rb-amount");
    amt.appendChild(el("label", null, "Amt"));
    var slider = document.createElement("input");
    slider.type = "range"; slider.min = 0; slider.max = 1; slider.step = 0.01; slider.value = quantAmount;
    slider.title = "quantise strength — a partial amount lerps toward the marker";
    slider.addEventListener("input", function () { quantAmount = parseFloat(slider.value); });
    amt.appendChild(slider);
    bar.appendChild(amt);

    var qx = el("button", "rb-btn", "Quantise X");
    qx.title = "pull the selection toward the nearest subdivision marker";
    qx.addEventListener("click", function () { quantiseSelectionX(quantAmount); });
    bar.appendChild(qx);

    var qy = el("button", "rb-btn", "Quantise Y");
    qy.title = "pull the selection toward the groove's centre curve";
    qy.addEventListener("click", function () { quantiseSelectionY(quantAmount); });
    bar.appendChild(qy);

    statusEl = el("span", "rb-status", "");
    bar.appendChild(statusEl);

    var hint = el("span", "rb-hint",
      "click places · drag moves · alt deletes · ctrl cycles probability · dbl toggles retrig · shift selects");
    bar.appendChild(hint);
    rack.parentNode.insertBefore(bar, rack);
  }

  function buildTrack(t) {
    var row = el("div", "track");

    // Voice box — shows the assigned voice; click opens the voice browser.
    var box = el("button", "voice-box");
    voiceBoxEls[t] = box;
    box.addEventListener("click", function () { openBrowser(t); });
    row.appendChild(box);

    // The lane strip: X is time, Y is a modulation value.
    var strip = el("div", "strip");
    strip.title = "lane " + (t + 1) + " — X is time, Y is modulation";
    stripEls[t] = strip;
    markerEls[t] = el("div", "markers");
    strip.appendChild(markerEls[t]);
    var centre = el("div", "centre");
    centre.style.top = pct(1 - Y_CENTRE);
    strip.appendChild(centre);
    playEls[t] = el("div", "playhead hidden");
    strip.appendChild(playEls[t]);
    hitLayerEls[t] = el("div", "hits");
    strip.appendChild(hitLayerEls[t]);
    (function (t) {
      strip.addEventListener("mousedown", function (ev) { onStripDown(t, ev); });
      strip.addEventListener("dblclick", function (ev) {
        if (!ev.target || !ev.target.classList.contains("hit")) return;
        var idx = Array.prototype.indexOf.call(hitLayerEls[t].children, ev.target);
        var h = lanes[t].hits[idx];
        if (!h) return;
        h.retrig = !h.retrig;
        send("set_hit_retrig", h.retrig
          ? { track: t, hit: idx, n: 4, m: 2, curve: "even", vel_end: 0.4 }
          : { track: t, hit: idx, n: 1, m: 1, curve: "even", vel_end: 1.0 });
        renderHits(t);
      });
    })(t);
    row.appendChild(strip);

    // Knobs: 3 performance macros (labelled from the voice's bindings) + gain/pan/send,
    // then the lane's own geometry (beats, subdivisions) and its choke group.
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

    // Lane length is a beat count (0348): a lane of fewer beats loops sooner and
    // phases against its neighbours — polymeter as geometry. Changing it re-lays
    // the markers, which re-times every hit hanging off them.
    knobs.appendChild(makeNumber("Bts", "beats in this lane (its loop length)", 1, MAX_BEATS, lanes[t].g.n_beats, function (n) {
      send("set_grid_beats", { track: t, beats: n });
      relayoutBeats(lanes[t].g, n);
      canonicaliseLane(lanes[t]);
      renderLaneStrip(t);
    }));
    // Subdivisions per beat — the snap-target density, and what the sub markers
    // draw. Three inside an otherwise-16ths lane is where a tuplet lives.
    knobs.appendChild(makeNumber("Sub", "subdivisions per beat (the snap targets)", 1, MAX_SUBS, lanes[t].g.default_subs, function (n) {
      send("set_grid_subs", { track: t, subs: n });
      setDefaultSubs(lanes[t].g, n);
      canonicaliseLane(lanes[t]);
      renderLaneStrip(t);
    }));
    // Choke group (0 = none). Tracks sharing a non-zero group cut each other.
    knobs.appendChild(makeNumber("Chk", "choke group (0 = none; shared group = mutual cut)", 0, 7, lanes[t].choke, function (g) {
      lanes[t].choke = g;
      send("set_choke_group", { track: t, group: g });
    }));
    row.appendChild(knobs);

    rack.appendChild(row);
    refreshLane(t);
    renderLaneStrip(t);
  }

  buildRackBar();
  for (var t2 = 0; t2 < NT; t2++) buildTrack(t2);
  document.addEventListener("mousemove", onDragMove);
  document.addEventListener("mouseup", onDragUp);
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
      // The note must be carried: a voice without one sends `note: undefined`,
      // which drops out of the JSON and makes the engine reject the whole
      // `add_hit` — a hit the page shows and the engine never received, after
      // which every fire-order index the page sends for that lane is off by one.
      var copy = {
        id: nextVoiceId++, name: v.name + " copy", engine: v.engine,
        flavour: cloneFlavour(v.flavour), note: v.note,
      };
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
  // The engine publishes a subdivision-slot index per lane; the strip places it
  // through the lane's own geometry, so the line tracks the **swung** grid rather
  // than a nominal fraction of the bar.
  function setPlay(t, slot) {
    var line = playEls[t], g = lanes[t].g;
    if (slot < 0 || slot >= totalSubs(g)) {
      line.classList.add("hidden");
      return;
    }
    var at = subOfIndex(g, slot);
    line.style.left = pct(subPos(g, at.beat, at.sub) / g.len_beats);
    line.classList.remove("hidden");
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
