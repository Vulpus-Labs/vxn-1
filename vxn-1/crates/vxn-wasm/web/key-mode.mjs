// Key-mode / split-point control path (E017, ticket 0056).
//
// Whole/Dual/Split key mode and the split point are NON-AUTOMATABLE shared
// state, not params (ADR 0003 §3): they never occupy a param-store slot and
// never travel on the event ring. They go out-of-band on the worklet port, set
// once and latched on the audio thread (the worklet applies them at block
// start). `WebHost.setKeyMode` / `setSplitPoint` already own that port hop; this
// module is the thin, named control surface the faceplate (E018) binds its
// mode buttons / split-point control to — so the page never pokes the port
// shape directly and the mode<->int mapping lives in one place.
//
// This mirrors the vxn-clap `UiEvent::Custom` key-mode/split path: the same
// three modes, the same "set once per block" semantics, just over the web port
// instead of the CLAP host's custom-event channel.

// Key mode enum — the wire values the worklet (and the engine behind it)
// expect. MUST match the engine's key-mode ordering (Whole/Dual/Split).
export const KeyMode = Object.freeze({
  WHOLE: 0, // one engine across the whole keyboard
  DUAL: 1, // both layers stacked across the whole keyboard
  SPLIT: 2, // lower/upper layers either side of the split point
});

// Human labels, for a faceplate that wants to render the mode name from the
// enum without duplicating the mapping.
export const KEY_MODE_LABELS = Object.freeze(["Whole", "Dual", "Split"]);

const MIN_NOTE = 0;
const MAX_NOTE = 127;

function isValidMode(m) {
  return m === KeyMode.WHOLE || m === KeyMode.DUAL || m === KeyMode.SPLIT;
}

// Set the key mode on the host. Accepts the numeric enum (KeyMode.*) or a
// case-insensitive name ("whole"/"dual"/"split"). Returns the numeric mode
// applied, or null if the input was unrecognised (no-op — never poisons the
// port with a junk value).
export function setKeyMode(host, mode) {
  let m = mode;
  if (typeof mode === "string") {
    const i = KEY_MODE_LABELS.findIndex((l) => l.toLowerCase() === mode.toLowerCase());
    m = i; // -1 if not found
  }
  if (!isValidMode(m)) return null;
  host.setKeyMode(m);
  return m;
}

// Set the split point (the MIDI note at/above which the upper layer sounds in
// SPLIT mode). Clamped to 0..127. Returns the clamped note actually sent.
export function setSplitPoint(host, note) {
  let n = note | 0;
  if (n < MIN_NOTE) n = MIN_NOTE;
  else if (n > MAX_NOTE) n = MAX_NOTE;
  host.setSplitPoint(n);
  return n;
}

// Bind a small stateful controller for a faceplate that wants to track the
// current mode/split and toggle between them. Pure convenience over the two
// setters; holds no audio state (the worklet is the source of truth) — it just
// remembers what was last sent so the UI can reflect it.
//
//   const kc = attachKeyMode(host, { mode: KeyMode.WHOLE, splitPoint: 60 });
//   kc.setMode("split"); kc.setSplitPoint(64); kc.getMode();
//
// Sends the initial mode/split on attach so the worklet and UI start in sync.
export function attachKeyMode(host, opts = {}) {
  const { mode = KeyMode.WHOLE, splitPoint = 60 } = opts;
  let curMode = isValidMode(mode) ? mode : KeyMode.WHOLE;
  let curSplit = Math.min(MAX_NOTE, Math.max(MIN_NOTE, splitPoint | 0));

  // Push the initial state so the audio thread isn't left at its own default.
  host.setKeyMode(curMode);
  host.setSplitPoint(curSplit);

  return {
    setMode(m) {
      const applied = setKeyMode(host, m);
      if (applied != null) curMode = applied;
      return curMode;
    },
    setSplitPoint(n) {
      curSplit = setSplitPoint(host, n);
      return curSplit;
    },
    getMode: () => curMode,
    getModeLabel: () => KEY_MODE_LABELS[curMode],
    getSplitPoint: () => curSplit,
  };
}
