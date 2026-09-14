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
  // grid.rs MIN_SLOT, shipped rather than guessed (0354): the strip draws the bound
  // a marker drag is about to clamp against, and a page holding its own idea of it
  // would draw the bound in one place while the engine clamped in another.
  var MIN_SLOT = CFG.min_slot || 1 / 64;
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
  // Grid::pos_of — the forward map, a (beat, sub, frac) triple's beat position.
  // The inverse of `locate`, and what the absolute-preserving marker edits capture
  // before the geometry moves under them.
  function posOf(g, beat, sub, frac) {
    var b = Math.min(beat, g.n_beats - 1);
    var k = Math.min(sub, subsAt(g, b) - 1);
    var p0 = subPos(g, b, k), p1 = subPos(g, b, k + 1);
    var f = isFinite(frac) ? frac : 0;
    if (f <= 0) return p0;
    if (f >= 1) return p1;  // exactly the next marker, so slot ends round-trip
    return p0 + f * (p1 - p0);
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
  // The mirror only works if it *starts* in agreement, and a page built from
  // scratch does not: reopening the editor over a lane that already holds hits
  // would rebuild this list from nothing while the model kept its own, and every
  // index sent afterwards would name the wrong hit. So the page is not built from
  // scratch — `CFG.lanes[t].hits` is the model's own list (0366), shipped in the
  // config the page is constructed from, and this list starts as a copy of it.
  // There is no request and no round trip, so there is no window in which the two
  // disagree; `applyLaneReadback` handles the one case that is left, the model
  // being replaced under a page already open.

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

  // ── marker edits: drag is relative, insert/delete absolute (0349, 0354) ──────
  // The page's half of ADR 0007 §5's two opposite rules. Ported from `sequencer.rs`
  // and `grid.rs` rather than approximated, for the reason the geometry port above
  // is: the page mirrors the engine's fire-order indices, so the two have to agree
  // on where every hit lands after a geometry edit, not merely nearly agree.
  //
  // Nothing here writes `markers[i]` directly — `setBeatMarker` is the single door,
  // exactly as it is in the engine, so no gesture can produce a slot narrower than
  // MIN_SLOT or unpin an outer marker.

  // Rebuild the resolved per-beat sub-counts after the overrides or the lane
  // default moved. `subs` is derived storage; `sub_override` is the truth.
  function syncSubs(g) {
    var subs = [];
    for (var b = 0; b < g.n_beats; b++) subs.push(g.sub_override[b] || g.default_subs);
    g.subs = subs;
  }
  // The bounds `set_beat_marker` clamps into — what the strip draws during a drag,
  // so the user sees the wall before hitting it.
  function markerBounds(g, i) {
    var lo = g.markers[i - 1] + MIN_SLOT, hi = g.markers[i + 1] - MIN_SLOT;
    return { lo: lo, hi: Math.max(hi, lo) };
  }
  // Grid::set_beat_marker — a clamped write, returning the position actually taken.
  // The outer markers are the pattern bounds and ignore this entirely.
  function setBeatMarker(g, i, pos) {
    if (i === 0 || i >= g.n_beats) return g.markers[Math.min(i, g.n_beats)];
    if (!isFinite(pos)) return g.markers[i];
    var b = markerBounds(g, i);
    g.markers[i] = clamp(pos, b.lo, b.hi);
    return g.markers[i];
  }
  // Grid::drop_marker — the body of a delete, and the inverse of an insert's shift.
  function dropMarker(g, i) {
    g.markers.splice(i, 1);
    g.sub_override.splice(i, 1);
    g.n_beats -= 1;
    syncSubs(g);
  }
  // Grid::insert_beat_marker — the two halves inherit the sub-count of the beat they
  // were cut from, and `pos` goes through the same clamp a drag takes. Returns the
  // index taken, or -1 for a refusal (the lane is full, or the slot is too narrow to
  // yield two of MIN_SLOT). A refused insert leaves the geometry as it was.
  function insertBeatMarker(g, i, pos) {
    if (g.n_beats >= MAX_BEATS || i === 0 || i > g.n_beats || !isFinite(pos)) return -1;
    if (pos <= g.markers[0] || pos >= g.markers[g.n_beats]) return -1;
    if (g.markers[i] - g.markers[i - 1] < 2 * MIN_SLOT) return -1;
    g.markers.splice(i, 0, g.markers[i - 1]); // overwritten by the clamped write below
    g.sub_override.splice(i, 0, g.sub_override[i - 1]);
    g.n_beats += 1;
    setBeatMarker(g, i, pos);
    if (g.markers[i] <= g.markers[i - 1] || g.markers[i + 1] <= g.markers[i]) {
      dropMarker(g, i);
      return -1;
    }
    syncSubs(g);
    return i;
  }

  // Pattern::edit_grid_preserving_times — the **absolute** door. Resolve every hit's
  // grid position, apply the edit, rebuild (beat, sub, f) from those positions, so an
  // insert or a delete moves nothing on screen.
  //
  // The nudge is held out of the sandwich (it is absolute by definition) but
  // *resolving* one is not geometry-free: `fireBeat` clamps it to ±½ of the hit's own
  // slot, so a slot that changed width changes how much of a large nudge survives.
  // Hence the second candidate — the grid part shifted by the change in the resolved
  // nudge — taken only when it lands closer to the fire time being preserved.
  function hitPos(g, h) {
    var b = Math.min(h.beat, g.n_beats - 1);
    return { beat: b, sub: Math.min(h.sub, subsAt(g, b) - 1), frac: h.f };
  }
  function resolvedNudge(g, at, nudge) {
    var half = 0.5 * (subPos(g, at.beat, at.sub + 1) - subPos(g, at.beat, at.sub));
    return clamp(nudge / TICKS_PER_BEAT, -half, half);
  }
  // Where a candidate would land once `f` has been through the `f32` it is stored in.
  function storedPosOf(g, at) { return posOf(g, at.beat, at.sub, f32(at.frac)); }
  // `edit` returns whether the geometry actually changed; a refused edit returns
  // early rather than rebuilding against a grid that did not move (that rebuild is
  // very nearly the identity, and "very nearly" is not what a refusal should do).
  function editPreservingTimes(lane, edit) {
    var g = lane.g, hits = lane.hits, at = [], nudged = [], i;
    for (i = 0; i < hits.length; i++) {
      var p0 = hitPos(g, hits[i]);
      at[i] = posOf(g, p0.beat, p0.sub, p0.frac);
      nudged[i] = resolvedNudge(g, p0, hits[i].nudge);
    }
    if (!edit(g)) return false;
    for (i = 0; i < hits.length; i++) {
      var nudge = hits[i].nudge;
      var p = locate(g, at[i]), np = resolvedNudge(g, p, nudge), best = p;
      if (np !== nudged[i]) {
        var q = locate(g, at[i] + nudged[i] - np), nq = resolvedNudge(g, q, nudge);
        var target = at[i] + nudged[i];
        if (Math.abs(storedPosOf(g, q) + nq - target) < Math.abs(storedPosOf(g, p) + np - target)) {
          best = q;
        }
      }
      hits[i].beat = best.beat; hits[i].sub = best.sub; hits[i].f = best.frac;
    }
    canonicaliseLane(lane);
    return true;
  }

  // Pattern::drag_beat_marker — **relative**: not one hit record is written, so the
  // slots either side stretch and squash and their hits rubber-band with them. The
  // list is still re-sorted, because `nudge` is absolute and a hit near a slot that
  // just narrowed can genuinely overtake its neighbour.
  function dragMarker(lane, i, pos) {
    var taken = setBeatMarker(lane.g, i, pos);
    canonicaliseLane(lane);
    return taken;
  }
  // `subs` **states** the new beat's sub-count override (0 = none) **inside** the same
  // absolute-preserving edit. It cannot be a following `setBeatSubs`: that takes the
  // relative door, so the pair composes to "preserve times, then move everything in
  // this beat".
  //
  // Stated and not defaulted, matching the opcode: `insertBeatMarker` gives the new
  // beat the split beat's override, so "leave it" and "clear it" differ on a tuplet.
  // If this skipped a zero the page would keep an inherited override the engine had
  // cleared, and the two grids would part company over an ordinary insert.
  function insertMarker(lane, i, pos, subs) {
    var taken = -1;
    editPreservingTimes(lane, function (g) {
      taken = insertBeatMarker(g, i, pos);
      if (taken < 0) return false;
      g.sub_override[taken] = clamp(subs | 0, 0, MAX_SUBS);
      syncSubs(g);
      return true;
    });
    return taken;
  }
  function deleteMarker(lane, i) {
    var g = lane.g;
    if (i === 0 || i >= g.n_beats) return false;
    return editPreservingTimes(lane, function (gg) { dropMarker(gg, i); return true; });
  }
  // Swing and the per-beat sub-count take the **relative** door, like a drag: the
  // markers move and the hits hanging off them move with them, which is what keeps a
  // welded hit (`f = 0`) welded across a swing sweep.
  function setLaneSwing(lane, swing) {
    lane.g.swing = { shape: swing.shape, amount: swing.amount, period: swing.period };
    canonicaliseLane(lane);
  }
  function setBeatSubs(lane, beat, subs) {
    if (beat < 0 || beat >= lane.g.n_beats) return;
    lane.g.sub_override[beat] = clamp(subs | 0, 0, MAX_SUBS);
    syncSubs(lane.g);
    canonicaliseLane(lane);
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
  // One reader for the `grid_json` shape, which arrives twice: in the config the
  // page is built from, and in a lane the model replaced under it.
  function gridFrom(src) {
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
  // …and one reader for a hit, for the same reason. `hit_json`'s field names are
  // the opcode field names, but the page's own model is older and differs in two
  // places: `probability` is `prob`, and retrig is a toggle (0353) with the engine's
  // actual macro parked beside it so switching it back on restores what the hit had
  // rather than the page's stock 4-over-2.
  //
  // `rgb` is `null` for an uncoloured hit and a triple otherwise — the engine's
  // NO_COLOUR sentinel, which is not a renderable colour. Carried verbatim, because
  // black is a real macro vector that sends zero to all three slots and the two must
  // not be conflated. 0355 draws it; until then the page holds it without rendering.
  function hitFrom(s) {
    var r = s.retrig || { n: 1, m: 1, curve: "even", vel_end: 1.0 };
    // `Retrig::is_retrig`, not `n > 1`: `m = 0` has no window to spread across.
    var on = r.n >= 2 && r.m >= 1;
    return {
      beat: s.beat, sub: s.sub, f: f32(s.f), nudge: s.nudge | 0, y: f32(s.y),
      note: s.note, velocity: s.velocity, prob: s.probability,
      retrig: on,
      retrigSpec: on ? { n: r.n, m: r.m, curve: r.curve, vel_end: r.vel_end } : null,
      rgb: s.rgb == null ? null : s.rgb.slice(),
    };
  }
  // A lane as the model states it. Adopted **in the order it arrives** and
  // deliberately not re-sorted: the list is already in fire order, every hit-keyed
  // opcode names a hit by its position in it, and re-deriving that order here would
  // hide a page/model arithmetic disagreement instead of inheriting the answer.
  function laneFrom(src) {
    var s = src || {};
    var g = s.grid;
    return {
      g: gridFrom(g && g.markers && g.subs && g.swing ? g : FALLBACK_GRID),
      hits: (s.hits || []).map(hitFrom),
    };
  }
  var lanes = [];
  for (var t = 0; t < NT; t++) {
    var v0 = voiceByName(DEFAULT_LANE[t]) || voices[t % voices.length] || voices[0];
    // Built from the model, not from defaults (0366): a reopened editor draws what
    // the instrument holds, and its hit indices name the model's hits from the
    // first gesture.
    var seed = laneFrom(CFG.lanes && CFG.lanes[t]);
    lanes.push({
      voiceId: v0 ? v0.id : 0,
      g: seed.g,
      hits: seed.hits,
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
  var railEls = [];       // railEls[t] — the grid rail: marker handles + beat cells
  var slotEls = [];       // slotEls[t] — the drag-feedback layer (slot highlight, clamp bounds)
  var swingEls = [];      // swingEls[t] — its swing control, refreshed by a lane readback
  var playEls = [];       // playEls[t] — its playhead line
  var voiceBoxEls = [];   // voiceBoxEls[t]
  var macroLabelEls = []; // macroLabelEls[t][slot]
  var macroKnobEls = [];  // macroKnobEls[t][slot] — the 3 performance-macro knob handles
  var beatsInputEls = []; // beatsInputEls[t] — the Bts box, refreshed by a lane readback
  var subsInputEls = [];  // subsInputEls[t] — the Sub box, likewise

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
  // ── the grid rail (0354): where the geometry is edited ──────────────────────
  // Beat markers are the stored tier (ADR 0007 §2) and the only draggable one, so
  // they get handles of their own in a rail above the hit surface. Putting them
  // there rather than on the marker lines themselves keeps a click on the strip a
  // placement, whatever piece of the grid it lands on.
  //
  // Every X here is `markers[i] / len_beats` — a real position, never a stride times
  // an index. The markers are unevenly spaced the moment one is dragged.
  function renderRail(t) {
    var lane = lanes[t], g = lane.g, rail = railEls[t], b, i;
    rail.innerHTML = "";
    // One cell per beat slot, carrying that beat's sub-count. The count is the
    // tuplet control: an overridden beat is marked, so a lane-wide sub edit does not
    // look as though it silently erased one.
    for (b = 0; b < g.n_beats; b++) {
      var cell = el("div", "rail-beat" + (b % 2 ? " alt" : ""));
      cell.style.left = pct(g.markers[b] / g.len_beats);
      cell.style.width = pct((g.markers[b + 1] - g.markers[b]) / g.len_beats);
      var ovr = g.sub_override[b] || 0;
      var badge = el("span", "subs-badge" + (ovr ? " ovr" : ""), String(g.subs[b]));
      badge.dataset.beat = String(b);
      badge.title = "beat " + (b + 1) + ": " + g.subs[b] + " subdivisions"
        + (ovr ? " (this beat's own — 3 is a triplet)" : " (the lane's)")
        + " · drag up/down to change it · click to follow the lane again";
      cell.appendChild(badge);
      rail.appendChild(cell);
    }
    var dragging = mdrag && mdrag.track === t;
    for (i = 0; i <= g.n_beats; i++) {
      var pinned = i === 0 || i === g.n_beats;
      var live = dragging && mdrag.i === i;
      var hnd = el("div", "mhandle" + (pinned ? " pinned" : "")
        + (live ? " active" : "") + (live && mdrag.clamped ? " clamped" : ""));
      hnd.style.left = pct(g.markers[i] / g.len_beats);
      hnd.dataset.marker = String(i);
      hnd.title = pinned
        ? "pinned — the outer markers are the pattern's bounds and cannot be dragged"
        : "drag: stretches the slot to the left and squashes the one to the right, "
          + "hits and all · right-click: deletes it, leaving every hit where it is";
      rail.appendChild(hnd);
    }
  }
  // Drag feedback, drawn over the hit surface because that is where the consequence
  // is: both adjacent slots lit at once — the two-sidedness is the part nobody
  // predicts — and the two positions the clamp will not let the marker past.
  function renderDragFeedback(t) {
    var layer = slotEls[t];
    layer.innerHTML = "";
    if (!mdrag || mdrag.track !== t) return;
    var g = lanes[t].g, s;
    for (s = mdrag.i - 1; s <= mdrag.i; s++) {
      if (s < 0 || s >= g.n_beats) continue;
      var hi = el("div", "slot-hi" + (s < mdrag.i ? " stretch" : " squash"));
      hi.style.left = pct(g.markers[s] / g.len_beats);
      hi.style.width = pct((g.markers[s + 1] - g.markers[s]) / g.len_beats);
      layer.appendChild(hi);
    }
    var at = g.markers[mdrag.i];
    [["lo", mdrag.lo], ["hi", mdrag.hi]].forEach(function (bound) {
      var line = el("div", "clamp-bound" + (at === bound[1] ? " at" : ""));
      line.style.left = pct(bound[1] / g.len_beats);
      layer.appendChild(line);
    });
  }
  function renderLaneStrip(t) {
    renderRail(t);
    renderMarkers(t);
    renderDragFeedback(t);
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
  // Returns the input alongside its wrapper: a lane readback carries the engine's
  // geometry, and a Bts/Sub box still showing the page's guess would be a lie the
  // user's next click would act on.
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
    return { wrap: wrap, input: inp };
  }

  // The swing control (0347, 0365): one per lane, driving the warp.
  //
  // It is **self-documenting**. The subdivision markers redraw unevenly as it moves
  // and every welded hit rides with them, so the feel is read off the strip rather
  // than off a percentage — the number in the label is the secondary readout, not
  // the primary one.
  //
  // The **period** is a control and not a constant (ADR 0007 Amendment, 0365): one
  // knee spans a pair of subdivisions or the whole beat, which on a 16ths lane is
  // 16th shuffle versus 8th swing. What the number means depends on it, so it is
  // beside the number rather than buried.
  function makeSwing(t) {
    var wrap = el("div", "swing");
    var lab = el("label", null, "Swg 0%");
    var inp = document.createElement("input");
    inp.type = "range"; inp.min = -1; inp.max = 1; inp.step = 0.01; inp.value = 0;
    inp.title = "swing — watch the subdivision markers, not the number: hits welded to a marker ride with it";
    var per = el("button", "swing-period", "pair");
    per.title = "the interval one knee spans — a pair of subdivisions (shuffle) or the whole beat";
    // The gesture, not the sample: a slider drag is one undo step however many
    // `input` events it fires.
    var before = null;
    function swingOf(t) {
      var s = lanes[t].g.swing;
      return { shape: s.shape, amount: s.amount, period: s.period };
    }
    function show(sw) {
      var n = Math.round(sw.amount * 100);
      lab.textContent = "Swg " + (n > 0 ? "+" : "") + n + "%";
      inp.value = sw.amount;
      per.textContent = sw.period === 0 ? "beat" : sw.period === 2 ? "pair" : String(sw.period);
      paintRange(inp);
    }
    inp.addEventListener("input", function () {
      if (!before) before = swingOf(t);
      var amount = parseFloat(inp.value);
      // Shape follows the amount. At zero the Mpc warp *is* the identity — bit for
      // bit, so nothing moves as it flips — and calling that Straight keeps a lane
      // nobody swung equal to one that was never swung.
      applySwing(t, { shape: amount === 0 ? 0 : 1, amount: amount, period: swingOf(t).period });
    });
    inp.addEventListener("change", function () {
      if (before) pushUndo(t, "a swing change", (function (prev) {
        return function () { applySwing(t, prev); };
      })(before));
      before = null;
    });
    per.addEventListener("click", function () {
      var prev = swingOf(t);
      applySwing(t, { shape: prev.shape, amount: prev.amount, period: prev.period === 0 ? 2 : 0 });
      pushUndo(t, "a swing period", function () { applySwing(t, prev); });
    });
    wrap.appendChild(lab); wrap.appendChild(inp); wrap.appendChild(per);
    // `cancel` is for the readback: `before` is closure state the resync cannot
    // reach, and a snapshot taken before a lane was replaced describes a swing that
    // lane no longer has.
    return { wrap: wrap, show: show, cancel: function () { before = null; } };
  }

  // ── strip interaction: place, drag, delete ──────────────────────────────────
  // A pointer position, resolved through the lane's geometry rather than through
  // any notion of a cell: `x` is a beat position, `y` is the lane's modulation axis.
  function pointerAt(t, ev) {
    // Measured against the **hit layer**, which is the surface the diamonds are
    // drawn on: it sits inside the strip's border and below the grid rail, so its
    // own box is the coordinate space without any of that having to be added back.
    // The rail shares its X, which is what lets a marker gesture and a placement
    // resolve the same pointer to the same beat position.
    var r = hitLayerEls[t].getBoundingClientRect();
    var u = clamp((ev.clientX - r.left) / Math.max(r.width, 1), 0, 1);
    var y = clamp((ev.clientY - r.top) / Math.max(r.height, 1), 0, 1);
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
    ev.preventDefault();
    var lane = lanes[t];
    var idx = -1;
    if (ev.target && ev.target.classList.contains("hit")) {
      idx = Array.prototype.indexOf.call(hitLayerEls[t].children, ev.target);
    }
    if (idx < 0) {
      // Empty strip: place a hit where the pointer is.
      var p = pointerAt(t, ev);
      var pos = positionFor(lane.g, p.t);
      var v = voiceById(lane.voiceId);
      var h = {
        beat: pos.beat, sub: pos.sub, f: pos.f, nudge: pos.nudge, y: p.y,
        note: v.note, velocity: 1.0, prob: 1.0, retrig: false,
        // Explicitly uncoloured, not merely absent: `null` is the shape a readback
        // hit carries (the engine's NO_COLOUR), and a fresh hit is the same thing.
        rgb: null,
        // No engine-side retrig macro behind it yet — see the dblclick toggle.
        retrigSpec: null,
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

  // ── geometry gestures: marker drag, insert, delete, sub-count ────────────────
  // Every one of these is a *request*: the page applies the engine's own clamped
  // edit locally and sends the same command, so the two land on the same geometry
  // rather than the page writing a position and hoping.
  //
  // The hits are never translated by a pixel delta. They are redrawn from `subPos`
  // after the geometry moved, which is the only thing that stays right on a grid
  // whose markers are unevenly spaced — and the only thing that does not
  // double-apply once the model has taken the same edit.
  var mdrag = null;  // { track, i, from, lo, hi, last, clamped, moved }
  var sdrag = null;  // { track, beat, y0, subs, was, moved }

  // A marker drag is one stored number however many hits appear to move (0349), so
  // its undo record is one number too — which is what lets a single step put every
  // apparent hit position back.
  var UNDO_MAX = 64;
  var undoStack = [];
  function pushUndo(track, label, fn) {
    undoStack.push({ track: track, label: label, undo: fn });
    if (undoStack.length > UNDO_MAX) undoStack.shift();
  }
  // A record is a marker index and a position *in a particular geometry*. Anything
  // that re-lays the markers wholesale — a beat-count change, a lane the model
  // replaced — leaves every stored index naming a different marker or none, so the
  // records for that lane go rather than being replayed against a grid they do not
  // describe.
  function dropUndo(track) {
    undoStack = undoStack.filter(function (r) { return r.track !== track; });
  }
  function undoLast() {
    var rec = undoStack.pop();
    if (!rec) { setStatus("Nothing to undo."); return; }
    rec.undo();
    setStatus("Undid " + rec.label + " on track " + (rec.track + 1) + ".");
  }

  // The four geometry edits, each as "tell the engine, apply the same thing here,
  // redraw". Shared by the gestures and by undo, so an undo cannot take a different
  // path from the edit it reverses.
  function applyMarkerDrag(t, i, pos) {
    send("drag_beat_marker", { track: t, marker: i, pos: pos });
    var taken = dragMarker(lanes[t], i, pos);
    renderLaneStrip(t);
    return taken;
  }
  // `subs` is the new beat's override, stated. An ordinary insert passes what the
  // split inherits — the beat being cut — so the page and the engine cannot read a
  // missing value two different ways.
  function applyMarkerInsert(t, i, pos, subs) {
    if (subs == null) subs = lanes[t].g.sub_override[i - 1] || 0;
    var at = insertMarker(lanes[t], i, pos, subs);
    if (at < 0) return -1;
    send("insert_beat_marker", { track: t, marker: i, pos: pos, subs: subs });
    refreshGeometry(t);
    renderLaneStrip(t);
    return at;
  }
  function applyMarkerDelete(t, i) {
    if (!deleteMarker(lanes[t], i)) return false;
    send("delete_beat_marker", { track: t, marker: i });
    refreshGeometry(t);
    renderLaneStrip(t);
    return true;
  }
  function applyBeatSubs(t, beat, subs) {
    send("set_beat_subs", { track: t, beat: beat, subs: subs });
    setBeatSubs(lanes[t], beat, subs);
    renderLaneStrip(t);
  }
  function applySwing(t, swing) {
    send("set_swing", { track: t, shape: swing.shape, amount: swing.amount, period: swing.period });
    setLaneSwing(lanes[t], swing);
    refreshGeometry(t);
    renderLaneStrip(t);
  }
  // The Bts / Sub / Swg controls are a readout of the lane's geometry as much as the
  // strip is, and a rail gesture changes that geometry. A Bts box still showing the
  // count from before an insert is not merely stale: the next click on its spinner
  // sends `set_grid_beats` from the *wrong* number, which re-lays every marker the
  // user placed by hand.
  function refreshGeometry(t) {
    var g = lanes[t].g;
    if (beatsInputEls[t]) beatsInputEls[t].value = g.n_beats;
    if (subsInputEls[t]) subsInputEls[t].value = g.default_subs;
    if (swingEls[t]) swingEls[t].show(g.swing);
  }

  function onRailDown(t, ev) {
    // The rail is not the hit surface: a gesture here must never fall through and
    // place a diamond.
    ev.preventDefault();
    ev.stopPropagation();
    // Left button only. `mousedown` fires for the right one too, and arming a drag
    // from it would leave a drag live underneath the delete that button is *for* —
    // the marker indices shift under a delete, so the still-held button would then
    // be dragging a different marker than the one it was pressed on.
    if (ev.button) return;
    var lane = lanes[t], g = lane.g, target = ev.target;
    if (target.classList.contains("subs-badge")) {
      var beat = parseInt(target.dataset.beat, 10);
      sdrag = {
        track: t, beat: beat, y0: ev.clientY,
        subs: g.subs[beat], was: g.sub_override[beat] || 0, moved: false,
      };
      return;
    }
    if (!target.classList.contains("mhandle")) return;
    var i = parseInt(target.dataset.marker, 10);
    if (i === 0 || i === g.n_beats) {
      setStatus("The outer markers are the pattern's bounds — use Bts to change the lane's length.");
      return;
    }
    var b = markerBounds(g, i);
    mdrag = { track: t, i: i, from: g.markers[i], lo: b.lo, hi: b.hi, last: g.markers[i], clamped: false, moved: false };
    renderLaneStrip(t);
    setStatus("Slot " + i + " stretches, slot " + (i + 1) + " squashes — both sets of hits move.");
  }

  function onMarkerDragMove(ev) {
    var t = mdrag.track, p = pointerAt(t, ev);
    // Clamped by the engine's own rule, in the engine's own arithmetic. The bound
    // being *hit* is worth showing even when nothing moves — that is the moment the
    // user needs to know the marker is not going any further.
    var taken = clamp(p.t, mdrag.lo, mdrag.hi);
    var wasClamped = mdrag.clamped;
    mdrag.clamped = taken !== p.t;
    if (taken === mdrag.last) {
      if (mdrag.clamped !== wasClamped) renderLaneStrip(t);
      return;
    }
    mdrag.last = taken;
    mdrag.moved = true;
    // `p.t` and not `taken`: the position is a request, and the clamp is the
    // engine's to apply. Sending the pre-clamp value keeps one implementation of the
    // bound instead of two that have to agree.
    applyMarkerDrag(t, mdrag.i, p.t);
  }
  function onMarkerDragUp() {
    var m = mdrag;
    mdrag = null;
    if (!m) return;
    if (m.moved) {
      var t = m.track, i = m.i, from = m.from;
      // Re-assert where the marker finished. A drag streams one command per
      // mousemove, and a full edit ring drops them (`EditQueue::push`) — so the
      // engine can be left on an older position with nothing following to correct
      // it. The last one is idempotent and costs a single command.
      //
      // Gated on `moved` alone, deliberately: a marker parked at its `MIN_SLOT`
      // wall, dragged away and back, ends where it started, so a `!== from` gate
      // would skip the re-assert on precisely the longest command stream — the
      // drag most likely to have lost one. The undo record still needs `!== from`,
      // since restoring a position the marker already holds is not an edit.
      applyMarkerDrag(t, i, lanes[t].g.markers[i]);
      if (lanes[t].g.markers[i] !== from) {
        pushUndo(t, "a marker drag", function () { applyMarkerDrag(t, i, from); });
      }
    }
    renderLaneStrip(m.track);
    setStatus("");
  }
  function onSubsDragMove(ev) {
    // Vertical, like a knob: 7px a subdivision, so the count is nudged rather than
    // flicked through. The strip redraws as it changes, which is the readout —
    // a beat set to 3 is three evenly-spaced subdivisions, visibly.
    var d = Math.round((sdrag.y0 - ev.clientY) / 7);
    if (d === 0 && !sdrag.moved) return;
    var n = clamp(sdrag.subs + d, 1, MAX_SUBS);
    sdrag.moved = true;
    if (n === lanes[sdrag.track].g.subs[sdrag.beat]) return;
    applyBeatSubs(sdrag.track, sdrag.beat, n);
  }
  function onSubsDragUp() {
    var s = sdrag;
    sdrag = null;
    if (!s) return;
    var g = lanes[s.track].g;
    // A click, not a drag: hand the beat back to the lane default. The two live on
    // one control on purpose — "this beat's own count" and "no, follow the lane"
    // are the same question.
    if (!s.moved && s.was) applyBeatSubs(s.track, s.beat, 0);
    if ((g.sub_override[s.beat] || 0) === s.was) return;
    pushUndo(s.track, "a sub-count", (function (t, b, v) {
      return function () { applyBeatSubs(t, b, v); };
    })(s.track, s.beat, s.was));
  }

  // Insert and delete are the **absolute**-preserving pair, and get affordances of
  // their own so they are not conflated with the drag: double-click the rail to
  // split a slot, right-click a handle to merge one. Both leave every hit exactly
  // where it is on screen, which is the opposite of what a drag does.
  function onRailDblClick(t, ev) {
    ev.preventDefault();
    ev.stopPropagation();
    // The rail's *background* splits a slot. A handle has its own gestures, and the
    // sub-count badge's single click already means something — double-clicking it is
    // a natural thing to do to a number, and it must not also cut the beat in two.
    if (ev.target.classList.contains("mhandle") || ev.target.classList.contains("subs-badge")) {
      return;
    }
    var lane = lanes[t], p = pointerAt(t, ev);
    // The index that splits the slot the pointer is *in*. Any other index would meet
    // the clamp and reshape a slot the user did not point at.
    var i = locate(lane.g, p.t).beat + 1;
    if (lane.g.n_beats >= MAX_BEATS) {
      setStatus("Track " + (t + 1) + " is at " + MAX_BEATS + " beats — the marker ceiling.");
      return;
    }
    if (applyMarkerInsert(t, i, p.t) < 0) {
      // Refused: either the slot cannot yield two of MIN_SLOT, or the position is
      // against a pattern bound. One message, because from the pointer's point of
      // view they are the same thing — there is no room here.
      setStatus("No room to split there — a slot cannot be thinner than the minimum.");
      return;
    }
    pushUndo(t, "a marker insert", function () { applyMarkerDelete(t, i); });
    setStatus("Marker inserted — the hits either side did not move.");
  }
  function onRailContext(t, ev) {
    ev.preventDefault();
    ev.stopPropagation();
    if (!ev.target.classList.contains("mhandle")) return;
    var g = lanes[t].g, i = parseInt(ev.target.dataset.marker, 10);
    if (i === 0 || i === g.n_beats) {
      setStatus("The outer markers are the pattern's bounds and cannot be deleted.");
      return;
    }
    var pos = g.markers[i];
    // The merged slot keeps the **left** beat's sub-count, so the right beat's own
    // override is what a delete discards — and an insert gives the new beat the
    // left's. Undo therefore has to put the override back explicitly, or a deleted
    // triplet would come back as a plain beat while the status line claimed nothing
    // had moved.
    var ovr = g.sub_override[i] || 0;
    if (!applyMarkerDelete(t, i)) return;
    pushUndo(t, "a marker delete", function () {
      // The override rides the insert rather than following it. As a separate
      // `set_beat_subs` it took the relative door and displaced every hit in the
      // restored beat — the undo put the triplet back and moved the hits it had just
      // promised not to touch.
      applyMarkerInsert(t, i, pos, ovr);
    });
    setStatus("Marker deleted — the hits either side did not move.");
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
      "strip: click places · drag moves · alt deletes · ctrl cycles probability · dbl toggles retrig · shift selects");
    bar.appendChild(hint);
    // The rail's gestures read differently from the strip's and are worth naming
    // apart: a marker drag carries its hits, an insert or a delete leaves them.
    var rail = el("span", "rb-hint rail",
      "rail: drag a marker (hits follow, both sides) · dbl inserts · right-click deletes (hits stay) · drag the number for tuplets · ⌘Z undoes");
    bar.appendChild(rail);
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
    slotEls[t] = el("div", "slots");
    strip.appendChild(slotEls[t]);
    playEls[t] = el("div", "playhead hidden");
    strip.appendChild(playEls[t]);
    hitLayerEls[t] = el("div", "hits");
    strip.appendChild(hitLayerEls[t]);
    // Last, so the handles sit over everything else — the rail is the only layer in
    // the strip that takes the pointer besides the diamonds themselves.
    railEls[t] = el("div", "rail");
    strip.appendChild(railEls[t]);
    (function (t) {
      railEls[t].addEventListener("mousedown", function (ev) { onRailDown(t, ev); });
      railEls[t].addEventListener("dblclick", function (ev) { onRailDblClick(t, ev); });
      railEls[t].addEventListener("contextmenu", function (ev) { onRailContext(t, ev); });
      strip.addEventListener("mousedown", function (ev) { onStripDown(t, ev); });
      strip.addEventListener("dblclick", function (ev) {
        if (!ev.target || !ev.target.classList.contains("hit")) return;
        var idx = Array.prototype.indexOf.call(hitLayerEls[t].children, ev.target);
        var h = lanes[t].hits[idx];
        if (!h) return;
        h.retrig = !h.retrig;
        // Toggling back on restores the macro the hit actually had, where the lane
        // was read back from an engine holding one. The strip's retrig is a single
        // toggle (0353), so without this a hit authored as 3-over-1 accelerating
        // would come back as the page's stock 4-over-2 even the first time it was
        // switched off and on — an edit the user never made.
        var spec = h.retrigSpec || { n: 4, m: 2, curve: "even", vel_end: 0.4 };
        send("set_hit_retrig", h.retrig
          ? { track: t, hit: idx, n: spec.n, m: spec.m, curve: spec.curve, vel_end: spec.vel_end }
          : { track: t, hit: idx, n: 1, m: 1, curve: "even", vel_end: 1.0 });
        if (h.retrig) h.retrigSpec = spec;
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
    var beatsNum = makeNumber("Bts", "beats in this lane (its loop length)", 1, MAX_BEATS, lanes[t].g.n_beats, function (n) {
      // Geometry re-times every hit hanging off it, so it is a lane edit like any
      // other and waits for the readback for the same reason.
      send("set_grid_beats", { track: t, beats: n });
      relayoutBeats(lanes[t].g, n);
      canonicaliseLane(lanes[t]);
      // A relayout throws the marker positions away and re-lays them evenly, so
      // every undo record's marker index names something else now.
      dropUndo(t);
      renderLaneStrip(t);
    });
    beatsInputEls[t] = beatsNum.input;
    knobs.appendChild(beatsNum.wrap);
    // Subdivisions per beat — the snap-target density, and what the sub markers
    // draw. Three inside an otherwise-16ths lane is where a tuplet lives.
    var subsNum = makeNumber("Sub", "subdivisions per beat (the snap targets)", 1, MAX_SUBS, lanes[t].g.default_subs, function (n) {
      send("set_grid_subs", { track: t, subs: n });
      setDefaultSubs(lanes[t].g, n);
      canonicaliseLane(lanes[t]);
      dropUndo(t); // a stored sub-count is a count in the geometry that just changed
      renderLaneStrip(t);
    });
    subsInputEls[t] = subsNum.input;
    knobs.appendChild(subsNum.wrap);
    // Swing: the lane's warp, and the per-beat override on the rail is the tuplet.
    swingEls[t] = makeSwing(t);
    swingEls[t].show(lanes[t].g.swing);
    knobs.appendChild(swingEls[t].wrap);
    // Choke group (0 = none). Tracks sharing a non-zero group cut each other.
    knobs.appendChild(makeNumber("Chk", "choke group (0 = none; shared group = mutual cut)", 0, 7, lanes[t].choke, function (g) {
      lanes[t].choke = g;
      send("set_choke_group", { track: t, group: g });
    }).wrap);
    row.appendChild(knobs);

    rack.appendChild(row);
    refreshLane(t);
    renderLaneStrip(t);
  }

  buildRackBar();
  for (var t2 = 0; t2 < NT; t2++) buildTrack(t2);
  document.addEventListener("mousemove", function (ev) {
    if (mdrag) { onMarkerDragMove(ev); return; }
    if (sdrag) { onSubsDragMove(ev); return; }
    onDragMove(ev);
  });
  document.addEventListener("mouseup", function () {
    if (mdrag) onMarkerDragUp();
    if (sdrag) onSubsDragUp();
    onDragUp();
  });
  // Undo, for the geometry gestures. A marker drag is the case that needs it: one
  // grab moves every hit in two slots, and putting them back by hand is not a thing
  // a user can do.
  document.addEventListener("keydown", function (ev) {
    if (!(ev.ctrlKey || ev.metaKey) || ev.shiftKey) return;
    if ((ev.key || "").toLowerCase() !== "z") return;
    // Not while a field has focus: the page has text boxes (voice and macro names)
    // and number boxes, and undo in one of those means undo the typing, not undo
    // somebody's marker drag two lanes away.
    var tag = ev.target && ev.target.tagName;
    if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return;
    ev.preventDefault();
    undoLast();
  });
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
  // ── the model replaced a lane under us ──────────────────────────────────────
  // The page is built from the model (0366), so this is *not* the ordinary path —
  // it fires only when the model was replaced rather than edited, which is what a
  // state restore does. An edit the user made came from here in the first place and
  // is never echoed back.
  //
  // A hard resync, not a merge: the model is the authority and this lane's local
  // copy is replaced whole, through the same `laneFrom` the page was built with.
  function applyLaneReadback(t, ev) {
    var lane = lanes[t];
    if (!lane) return;
    var next = laneFrom(ev);
    lane.g = next.g;
    lane.hits = next.hits;
    // Any index a gesture is holding named the *old* list, so no gesture survives —
    // and neither does an undo record, whose whole content is a position in a
    // geometry this lane no longer has.
    if (drag && drag.track === t) drag = null;
    if (mdrag && mdrag.track === t) mdrag = null;
    if (sdrag && sdrag.track === t) sdrag = null;
    if (swingEls[t]) swingEls[t].cancel();
    dropUndo(t);
    // The selection holds hit *objects*; the ones this replaced are now in no lane
    // at all, so drop exactly those and leave other lanes' alone.
    selection = selection.filter(stillPlaced);
    // The same three boxes a rail gesture refreshes, through the same function: two
    // copies of "what the geometry controls read" would only have to be kept in step.
    refreshGeometry(t);
    renderLaneStrip(t);
  }
  function stillPlaced(h) {
    for (var k = 0; k < NT; k++) if (lanes[k].hits.indexOf(h) >= 0) return true;
    return false;
  }

  var transport = document.getElementById("transport");
  window.__vxn = window.__vxn || {};
  window.__vxn.applyViewEvents = function (events) {
    for (var i = 0; i < events.length; i++) {
      var ev = events[i];
      if (ev.kind === "lane") {
        applyLaneReadback(ev.track, ev);
        continue;
      }
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
