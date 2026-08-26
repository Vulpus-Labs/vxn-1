// On-screen piano keyboard — SHARED across the browser ports (ticket 0308).
//
// A note producer and nothing else: it calls `host.noteOn(note, velocity, 0)` /
// `host.noteOff(note, 0)` on the same coordinator surface the computer keyboard
// and Web MIDI use, so the ring stays source-agnostic and the synth cannot tell
// which producer played it. It knows nothing about either synth's model, which
// is why it can be shared rather than forked — it was vxn-2's, moved here
// unchanged when VXN1b needed one (0284's rule: parameterise, don't copy).
//
// Self-contained DOM with inline styles, deliberately: it is a WEB-ONLY
// affordance (the plugin has a host and a real keyboard), so it must not depend
// on anything in a synth's faceplate.css and must not appear in the plugin
// build at all. No-ops without a document body, so headless tests are
// unaffected.
//
// Drag is MONOPHONIC — each new key releases the previous note. That is a
// glissando, which is what dragging a mouse across keys means; holding a chord
// needs the computer keyboard or MIDI.

// MIDI note -> true if it's a black (accidental) key. Pattern within an octave:
// C# D# _ F# G# A# are the five black keys (pitch classes 1,3,6,8,10).
export function isBlackKey(note) {
  const pc = ((note % 12) + 12) % 12;
  return pc === 1 || pc === 3 || pc === 6 || pc === 8 || pc === 10;
}

// Build the key layout for the inclusive MIDI range [startNote, endNote]:
// an ordered list of { note, black }. White keys lay out left-to-right; each
// black key floats between its neighbouring whites.
export function pianoLayout(startNote, endNote) {
  const keys = [];
  for (let n = startNote; n <= endNote; n++) keys.push({ note: n, black: isBlackKey(n) });
  return keys;
}

const PIANO_DEFAULT_START = 48; // C3
const PIANO_DEFAULT_END = 84; // C6 (inclusive) — three octaves
const PIANO_VELOCITY = 0.8; // no pressure sensing on click; match keyboard-input

export function createPianoKeyboard(doc = globalThis.document, host = null, opts = {}) {
  if (!doc || !doc.body) return { el: null, detach() {}, allNotesOff() {} };
  const ID = "vxn-piano";
  if (doc.getElementById(ID)) return { el: doc.getElementById(ID), detach() {}, allNotesOff() {} };

  const startNote = opts.startNote != null ? opts.startNote : PIANO_DEFAULT_START;
  const endNote = opts.endNote != null ? opts.endNote : PIANO_DEFAULT_END;
  const velocity = opts.velocity != null ? opts.velocity : PIANO_VELOCITY;
  const layout = pianoLayout(startNote, endNote);
  const whites = layout.filter((k) => !k.black);

  const bar = doc.createElement("div");
  bar.id = ID;
  bar.style.cssText =
    "position:fixed;left:0;right:0;bottom:0;z-index:9998;height:92px;" +
    "display:flex;background:#14161a;border-top:1px solid rgba(255,255,255,.10);" +
    "box-shadow:0 -6px 20px rgba(0,0,0,.4);user-select:none;touch-action:none;" +
    "-webkit-user-select:none;";

  // A relative container the whites flex inside and the blacks absolutely sit on.
  const bed = doc.createElement("div");
  bed.style.cssText = "position:relative;display:flex;flex:1;height:100%;";
  bar.appendChild(bed);

  // note -> key element, so glissando and note-off can toggle the highlight.
  const keyEls = new Map();
  const whiteW = 100 / whites.length; // percent width per white key

  function styleWhite(el, active) {
    el.style.background = active ? "#8fd0ff" : "#f4f4f2";
  }
  function styleBlack(el, active) {
    el.style.background = active ? "#4c8fbf" : "#1b1d21";
  }

  // Lay out white keys first (flex children), then overlay black keys.
  let whiteIndex = 0;
  for (const k of layout) {
    if (k.black) continue;
    const el = doc.createElement("div");
    el.className = "vxn-piano-white";
    el.dataset.note = String(k.note);
    el.style.cssText =
      "flex:1;height:100%;border-right:1px solid rgba(0,0,0,.28);" +
      "border-radius:0 0 3px 3px;box-sizing:border-box;";
    styleWhite(el, false);
    bed.appendChild(el);
    keyEls.set(k.note, el);
    whiteIndex++;
  }

  // Overlay black keys. A black key at MIDI note n sits over the boundary between
  // the white below it (n-1) and the white above (n+1); centre it on that seam.
  for (const k of layout) {
    if (!k.black) continue;
    const whitesBelow = whites.filter((w) => w.note < k.note).length; // seam index
    const el = doc.createElement("div");
    el.className = "vxn-piano-black";
    el.dataset.note = String(k.note);
    const centre = whitesBelow * whiteW; // seam position, in percent
    el.style.cssText =
      "position:absolute;top:0;height:62%;width:" + (whiteW * 0.62).toFixed(4) + "%;" +
      "left:" + centre.toFixed(4) + "%;transform:translateX(-50%);" +
      "border-radius:0 0 3px 3px;box-sizing:border-box;z-index:2;" +
      "box-shadow:0 2px 3px rgba(0,0,0,.5);";
    styleBlack(el, false);
    bed.appendChild(el);
    keyEls.set(k.note, el);
  }

  // ---- pointer -> note plumbing ----
  let pointerDown = false;
  let current = null; // the single note sounding from the mouse/touch drag

  function paint(note, active) {
    const el = keyEls.get(note);
    if (!el) return;
    if (isBlackKey(note)) styleBlack(el, active);
    else styleWhite(el, active);
  }

  function press(note) {
    if (note == null || note === current) return;
    if (current != null) release(); // monophonic drag: release the old note first
    current = note;
    paint(note, true);
    if (host && typeof host.noteOn === "function") host.noteOn(note, velocity, 0);
  }

  function release() {
    if (current == null) return;
    const note = current;
    current = null;
    paint(note, false);
    if (host && typeof host.noteOff === "function") host.noteOff(note, 0);
  }

  // Resolve the DOM target under a pointer to its MIDI note (data-note). Because
  // black keys sit above whites in z-order, elementFromPoint / event.target gives
  // the topmost key, which is what we want.
  function noteFromTarget(t) {
    if (!t || !t.dataset) return null;
    const n = t.dataset.note;
    return n == null ? null : parseInt(n, 10);
  }

  function onDown(e) {
    pointerDown = true;
    const note = noteFromTarget(e.target);
    if (note != null) {
      press(note);
      if (typeof e.preventDefault === "function") e.preventDefault();
    }
  }
  function onOver(e) {
    if (!pointerDown) return;
    const note = noteFromTarget(e.target);
    if (note != null) press(note);
  }
  function onUp() {
    pointerDown = false;
    release();
  }

  // Pointer events cover mouse + touch + pen uniformly. mouseover/enter on the
  // per-key elements bubbles to `bed`, so one delegated listener handles drag.
  bed.addEventListener("pointerdown", onDown);
  bed.addEventListener("pointerover", onOver);
  // Release anywhere (pointer may lift off the keyboard) — listen on the document.
  doc.addEventListener("pointerup", onUp);
  doc.addEventListener("pointercancel", onUp);

  function allNotesOff() {
    pointerDown = false;
    release();
  }

  doc.body.appendChild(bar);

  return {
    el: bar,
    allNotesOff,
    detach() {
      allNotesOff();
      bed.removeEventListener("pointerdown", onDown);
      bed.removeEventListener("pointerover", onOver);
      doc.removeEventListener("pointerup", onUp);
      doc.removeEventListener("pointercancel", onUp);
      if (bar.remove) bar.remove();
    },
    // Exposed for tests / drivers that synthesise pointer events.
    _press: press,
    _release: release,
  };
}
