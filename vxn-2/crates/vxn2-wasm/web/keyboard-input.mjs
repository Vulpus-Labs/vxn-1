// Computer-keyboard note input (E017, ticket 0055).
//
// The device-less play path: map the QWERTY keyboard to note numbers so users
// without a MIDI controller can still play the web synth. Like midi-input.mjs,
// this is purely a producer for the E015 ring — it only calls WebHost.noteOn /
// noteOff, so the ring stays source-agnostic.
//
// LAYOUT: the de-facto tracker / Ableton "computer MIDI keyboard" layout —
// two rows forming a piano octave-and-a-bit, the lower (ZXCV…) row the white
// keys from the base note, the upper (SD FGH…) row the black keys, plus the
// QWERTY row as a second octave starting an octave up. We key off
// `KeyboardEvent.code` (physical key, layout-independent) NOT `.key`, so the
// mapping is identical on AZERTY/QWERTZ/Dvorak — the physical piano shape is
// what matters, not the printed legend.
//
//   Lower octave (base + 0):
//     KeyZ  C    KeyS  C#   KeyX  D    KeyD  D#   KeyC  E    KeyV  F
//     KeyG  F#   KeyB  G    KeyH  G#   KeyN  A    KeyJ  A#   KeyM  B
//     Comma C(+12)  KeyL  C#(+12)  Period D(+12)
//   Upper octave (base + 12):
//     KeyQ  C    Digit2 C#  KeyW  D    Digit3 D#  KeyE  E    KeyR  F
//     KeyT  F#   Digit5 G   KeyY  G#   KeyU  A    Digit7 A#  KeyI  B
//     KeyO  C(+12)  Digit0 C#(+12)  KeyP  D(+12)
//
// The two rows overlap by an octave so you get ~2.5 octaves of contiguous range
// from one base, shiftable by OCTAVE keys.
//
// HELD-NOTE TRACKING / AUTO-REPEAT: holding a key fires repeated `keydown`
// events (OS auto-repeat). We MUST NOT retrigger the note on each repeat — a
// held note should sound once and sustain. We track the set of physically-held
// physical-key codes; a `keydown` for an already-held code is swallowed.
// Releasing (`keyup`) sends the note-off and clears the held entry. We also
// honour `event.repeat` when present (cheap fast-path) but don't rely on it —
// the held-set is the source of truth, because focus loss can drop a keyup.
//
// OCTAVE SHIFT: 'KeyZ'-row base octave is shifted by dedicated keys (default
// 'Minus' / 'Equal', i.e. the - / = keys). Shifting only affects notes pressed
// AFTER the shift; notes already sounding keep their pitch and get the correct
// note-off (we remember the note number we sent per held code, so a shift
// mid-hold can't orphan a note-off at the wrong pitch).

// Physical-key (event.code) -> semitone offset from the base note.
// Two overlapping octaves; see the header diagram.
const KEY_TO_SEMITONE = {
  // lower row (white) + black keys
  KeyZ: 0, // C
  KeyS: 1, // C#
  KeyX: 2, // D
  KeyD: 3, // D#
  KeyC: 4, // E
  KeyV: 5, // F
  KeyG: 6, // F#
  KeyB: 7, // G
  KeyH: 8, // G#
  KeyN: 9, // A
  KeyJ: 10, // A#
  KeyM: 11, // B
  Comma: 12, // C
  KeyL: 13, // C#
  Period: 14, // D
  // upper row, one octave up
  KeyQ: 12, // C
  Digit2: 13, // C#
  KeyW: 14, // D
  Digit3: 15, // D#
  KeyE: 16, // E
  KeyR: 17, // F
  KeyT: 18, // F#
  Digit5: 19, // G
  KeyY: 20, // G#
  KeyU: 21, // A
  Digit7: 22, // A#
  KeyI: 23, // B
  KeyO: 24, // C
  Digit0: 25, // C#
  KeyP: 26, // D
};

// Default octave-shift keys (event.code). Minus/Equal == the '-' and '=' keys.
const DEFAULT_OCTAVE_DOWN = "Minus";
const DEFAULT_OCTAVE_UP = "Equal";

// Base MIDI note for octave 0 of the lower row. 48 == C3, a comfortable centre
// that keeps both overlapping octaves in audible range and leaves headroom to
// shift several octaves either way before clipping the 0..127 MIDI range.
const DEFAULT_BASE_NOTE = 48;

// MIDI velocity for computer-keyboard notes (no pressure sensing). Unit [0,1];
// a musical-but-not-max default.
const DEFAULT_VELOCITY = 0.8;

const MIN_NOTE = 0;
const MAX_NOTE = 127;

function clampNote(n) {
  return n < MIN_NOTE ? MIN_NOTE : n > MAX_NOTE ? MAX_NOTE : n;
}

// Attach computer-keyboard note input to a WebHost. Returns a controller:
//   { detach(), octaveUp(), octaveDown(), setOctave(n), getOctave(),
//     held(), allNotesOff() }
//
// opts:
//   target           : EventTarget to listen on (default globalThis /
//                      document). Injectable for headless tests.
//   baseNote         : MIDI note for the lower row's C at octave 0 (default 48).
//   velocity         : note-on velocity, unit [0,1] (default 0.8).
//   octaveDownKey /
//   octaveUpKey      : event.code values for octave shift (default Minus/Equal).
//   minOctave/maxOctave : clamp on the octave shift (default -4..+4).
//   ignoreWhenTyping : if true (default), ignore key events whose target is an
//                      <input>/<textarea>/contenteditable, so typing in a field
//                      (e.g. the preset name box) doesn't play notes.
//
// The handlers swallow auto-repeat and route only fresh presses/releases to the
// host. `keyMap` may be supplied to override the default QWERTY layout.
export function attachKeyboard(host, opts = {}) {
  const {
    target = globalThis.document || globalThis,
    baseNote = DEFAULT_BASE_NOTE,
    velocity = DEFAULT_VELOCITY,
    octaveDownKey = DEFAULT_OCTAVE_DOWN,
    octaveUpKey = DEFAULT_OCTAVE_UP,
    minOctave = -4,
    maxOctave = 4,
    ignoreWhenTyping = true,
    keyMap = KEY_TO_SEMITONE,
  } = opts;

  let octave = 0;

  // code -> the MIDI note we actually SENT for this physical key, so the
  // matching note-off is always at the same pitch even if the octave shifted
  // while the key was held. Also doubles as the held-set for auto-repeat
  // suppression (presence == held).
  const held = new Map();

  function isTypingTarget(t) {
    if (!t || typeof t !== "object") return false;
    const tag = (t.tagName || "").toUpperCase();
    if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
    if (t.isContentEditable) return true;
    return false;
  }

  function noteForCode(code) {
    const semi = keyMap[code];
    if (semi == null) return null;
    return clampNote(baseNote + octave * 12 + semi);
  }

  function onKeyDown(e) {
    if (ignoreWhenTyping && isTypingTarget(e.target)) return;
    const code = e.code;

    // Octave shift keys.
    if (code === octaveDownKey) {
      setOctave(octave - 1);
      return;
    }
    if (code === octaveUpKey) {
      setOctave(octave + 1);
      return;
    }

    if (!(code in keyMap)) return;
    // Auto-repeat / already-held: swallow. event.repeat is a fast-path; the
    // held-set is authoritative (a repeat without the flag still won't retrig).
    if (e.repeat || held.has(code)) {
      if (typeof e.preventDefault === "function") e.preventDefault();
      return;
    }
    const note = noteForCode(code);
    if (note == null) return;
    held.set(code, note);
    host.noteOn(note, velocity, 0);
    if (typeof e.preventDefault === "function") e.preventDefault();
  }

  function onKeyUp(e) {
    const code = e.code;
    if (!held.has(code)) return;
    const note = held.get(code);
    held.delete(code);
    host.noteOff(note, 0);
    if (typeof e.preventDefault === "function") e.preventDefault();
  }

  // Window blur / focus loss: the OS may swallow the keyup, leaving a stuck
  // note. Flush everything held on blur so we never hang a voice.
  function onBlur() {
    allNotesOff();
  }

  function setOctave(n) {
    octave = n < minOctave ? minOctave : n > maxOctave ? maxOctave : n;
    return octave;
  }

  function allNotesOff() {
    for (const [code, note] of held) host.noteOff(note, 0);
    held.clear();
  }

  // Wire listeners. addEventListener on the chosen target (document in the
  // browser; an injected fake in tests). blur is a window-level event; we add it
  // to globalThis when available.
  if (typeof target.addEventListener === "function") {
    target.addEventListener("keydown", onKeyDown);
    target.addEventListener("keyup", onKeyUp);
  }
  const blurTarget = globalThis.addEventListener ? globalThis : null;
  if (blurTarget) blurTarget.addEventListener("blur", onBlur);

  return {
    detach() {
      if (typeof target.removeEventListener === "function") {
        target.removeEventListener("keydown", onKeyDown);
        target.removeEventListener("keyup", onKeyUp);
      }
      if (blurTarget) blurTarget.removeEventListener("blur", onBlur);
      allNotesOff();
    },
    octaveUp: () => setOctave(octave + 1),
    octaveDown: () => setOctave(octave - 1),
    setOctave,
    getOctave: () => octave,
    held: () => Array.from(held.values()),
    allNotesOff,
    // Exposed for tests / fake-target drivers that synthesise events.
    _onKeyDown: onKeyDown,
    _onKeyUp: onKeyUp,
  };
}

export { KEY_TO_SEMITONE, DEFAULT_BASE_NOTE };
