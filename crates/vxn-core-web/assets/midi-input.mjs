// Web MIDI input adapter (E017, ticket 0053 + 0054).
//
// The browser-input half of E017: take a `WebHost` (coordinator.mjs producer
// surface) and a MIDIAccess, subscribe every input port's `onmidimessage`,
// decode raw MIDI bytes into producer calls (noteOn/off+velocity, pitchBend,
// modWheel, sustain), and handle device hotplug. The ring stays
// source-agnostic — this module only ever calls the SAME WebHost methods the
// faceplate UI and automation call, so MIDI is just one more producer.
//
// SCOPE: channel-voice messages we map to the E015 ring —
//   0x90 NoteOn   (vel 0 == NoteOff, the running-status convention)
//   0x80 NoteOff
//   0xE0 PitchBend  -> [-1, 1]
//   0xB0 CC 1  ModWheel -> [0, 1]
//   0xB0 CC 64 Sustain  -> on iff value >= 64
// Everything else (program change, aftertouch, other CCs, SysEx, clock) is
// ignored — MIDI output / clock / MPE are explicitly out of E017's scope.
//
// CHANNELS: we are a single-timbral synth; all 16 channels fold onto the one
// engine (no per-channel routing). This matches vxn-clap, which also ignores
// the MIDI channel nibble.
//
// ===========================================================================
// TIMESTAMP -> SAMPLE-OFFSET  (the honest version — see 0054)
// ===========================================================================
//
// Web MIDI hands each message a `DOMHighResTimeStamp` (`event.timeStamp`),
// measured on the SAME clock as `performance.now()` (ms since the time origin).
// The render quantum is placed on the AudioContext clock (`ctx.currentTime`,
// seconds). To put a MIDI event at the right sub-block position we must convert
// between the two time bases and express the result as a frame offset 0..Q-1
// into the *upcoming* quantum.
//
// The bridge between the clocks is `AudioContext.getOutputTimestamp()`, which
// returns `{ contextTime, performanceTime }` — the same instant in both bases.
// Given that pair we can map a performance-time MIDI stamp to a context time:
//
//   contextTimeOfEvent = contextTime + (event.timeStamp - performanceTime) / 1000
//
// and then to a frame offset relative to the block the worklet is about to
// render. We don't know the worklet's exact block boundary from the main
// thread, so we anchor on `ctx.currentTime` (the playback position the context
// reports *now*) and compute how far past it the event falls:
//
//   deltaSec   = contextTimeOfEvent - ctx.currentTime
//   offsetFrm  = round(deltaSec * sampleRate)
//   offset     = clamp(offsetFrm, 0, Q-1)
//
// HONEST LIMITS (this is approximate — the epic flags it as a risk):
//
//   * `ctx.currentTime` advances in quantum-sized steps and lags the truly
//     "next" block by an unknown amount (output latency, the render-ahead the
//     browser keeps). So the absolute phase of our offset within the worklet's
//     block is not guaranteed — we are accurate to the *spacing* of events
//     within a quantum, not their absolute frame.
//   * Most MIDI stamps arrive in the PAST relative to `ctx.currentTime` (the
//     message was generated a few ms ago and `currentTime` has moved on). Those
//     clamp to offset 0 ("as soon as possible"), which is the correct, safe
//     degenerate: the event lands at the block start, exactly the
//     postMessage-only behaviour, never in a stale block.
//   * Events that land more than one quantum in the future clamp to Q-1 rather
//     than being deferred to a later block (we don't buffer across quanta on the
//     main thread). At Q=128 / 48 kHz a quantum is ~2.7 ms; a stamp that far
//     ahead is rare for live play.
//   * Some environments expose `event.timeStamp === 0` (timestamp unsupported)
//     or no `getOutputTimestamp`. We detect both and fall back to offset 0, so
//     the adapter still plays — just without sub-block placement. This keeps the
//     keyboard/Safari-without-Web-MIDI story honest: degrade, never throw.
//
// Net: when the clocks cooperate we get tighter-than-postMessage timing for
// events that cluster inside a quantum; when they don't we degrade to the
// block-start path. Either way the ring contract is satisfied (offset in
// 0..Q-1) and no event is lost.

// Status byte high-nibble message types (low nibble = channel, ignored).
const MSG_NOTE_OFF = 0x80;
const MSG_NOTE_ON = 0x90;
const MSG_CONTROL_CHANGE = 0xb0;
const MSG_PITCH_BEND = 0xe0;

const CC_MOD_WHEEL = 1;
const CC_SUSTAIN = 64;

// Default render quantum (Web Audio is fixed at 128). Overridable for tests /
// future changes via opts.quantum.
const DEFAULT_QUANTUM = 128;

// Convert a 14-bit pitch-bend value (0..16383, centre 8192) to [-1, 1]. We use
// asymmetric scaling so 0 -> -1, 8192 -> 0, 16383 -> +1 exactly (the standard
// MIDI convention — the negative side has one more code than the positive).
function bend14ToUnit(lsb, msb) {
  const raw = (msb << 7) | lsb; // 0..16383
  const centred = raw - 8192; // -8192..+8191
  return centred < 0 ? centred / 8192 : centred / 8191;
}

// MIDI 7-bit value (0..127) to unit [0, 1].
function cc7ToUnit(v) {
  return v / 127;
}

// ---------------------------------------------------------------------------
// Timestamp -> sample-offset
// ---------------------------------------------------------------------------

// Build the offset mapper for a host. Returns a function (timeStamp) -> offset.
// Closes over the host's AudioContext + sample rate; tolerant of a missing
// context / getOutputTimestamp (returns 0, i.e. "block start", the safe
// degenerate documented above).
export function makeOffsetMapper(host, quantum = DEFAULT_QUANTUM) {
  return (timeStamp) => {
    const ctx = host && host.ctx;
    if (!ctx || typeof ctx.getOutputTimestamp !== "function") return 0;
    if (!(timeStamp > 0)) return 0; // 0 / NaN / undefined -> unsupported stamp
    let ots;
    try {
      ots = ctx.getOutputTimestamp();
    } catch {
      return 0;
    }
    const { contextTime, performanceTime } = ots || {};
    if (contextTime == null || performanceTime == null) return 0;
    const sampleRate = ctx.sampleRate || 48000;
    // performanceTime and timeStamp are both ms on the performance clock.
    const contextTimeOfEvent = contextTime + (timeStamp - performanceTime) / 1000;
    const deltaSec = contextTimeOfEvent - ctx.currentTime;
    const offsetFrm = Math.round(deltaSec * sampleRate);
    if (offsetFrm <= 0) return 0;
    if (offsetFrm > quantum - 1) return quantum - 1;
    return offsetFrm;
  };
}

// ---------------------------------------------------------------------------
// Decode one MIDI message into producer calls
// ---------------------------------------------------------------------------

// Decode `data` (a Uint8Array / number[] of MIDI bytes) and drive `host`'s
// producer surface, placing the event at `offset` within the upcoming quantum.
// Exported standalone so tests can feed synthetic byte arrays without a port.
// Unknown / unhandled messages are silently ignored (forward-compatible).
export function decodeMidiMessage(host, data, offset = 0) {
  if (!data || data.length < 1) return;
  const status = data[0];
  if (status < 0x80) return; // not a status byte (running status unsupported)
  const type = status & 0xf0;

  switch (type) {
    case MSG_NOTE_ON: {
      const note = data[1];
      const vel = data[2] | 0;
      // Running-status convention: NoteOn velocity 0 IS a NoteOff.
      if (vel === 0) host.noteOff(note, offset);
      else host.noteOn(note, cc7ToUnit(vel), offset);
      break;
    }
    case MSG_NOTE_OFF: {
      host.noteOff(data[1], offset);
      break;
    }
    case MSG_PITCH_BEND: {
      host.pitchBend(bend14ToUnit(data[1] | 0, data[2] | 0), offset);
      break;
    }
    case MSG_CONTROL_CHANGE: {
      const cc = data[1];
      const val = data[2] | 0;
      if (cc === CC_MOD_WHEEL) host.modWheel(cc7ToUnit(val), offset);
      else if (cc === CC_SUSTAIN) host.sustain(val >= 64, offset);
      // Other CCs: ignored (out of scope).
      break;
    }
    default:
      break; // aftertouch / program-change / system: ignored
  }
}

// ---------------------------------------------------------------------------
// Attach: subscribe inputs, decode, handle hotplug
// ---------------------------------------------------------------------------

// Attach Web MIDI to a WebHost. Returns a controller:
//   { access, inputs(), detach(), state }
//
// opts:
//   requestMIDIAccess : navigator.requestMIDIAccess seam (default the global).
//                       Injectable for headless tests (Node has no Web MIDI).
//   sysex             : forwarded to requestMIDIAccess (default false).
//   quantum           : render quantum for the offset mapper (default 128).
//   onStateChange(p)  : called on every device connect/disconnect with the raw
//                       MIDIConnectionEvent.port (id, name, state, type).
//   onError(err)      : called if access is denied / unavailable. The adapter
//                       resolves (does not reject) so the keyboard fallback can
//                       still come up; inspect controller.state.granted.
//
// GRACEFUL DENIAL: if requestMIDIAccess is absent (Safari w/o Web MIDI) or the
// permission prompt is denied, we DON'T throw — we resolve a controller whose
// `state.granted === false`, so the page can fall back to keyboard input. This
// is the safety-net contract the epic's risk section calls for.
export async function attachMidi(host, opts = {}) {
  const {
    requestMIDIAccess = globalThis.navigator && globalThis.navigator.requestMIDIAccess
      ? globalThis.navigator.requestMIDIAccess.bind(globalThis.navigator)
      : null,
    sysex = false,
    quantum = DEFAULT_QUANTUM,
    onStateChange = () => {},
    onError = () => {},
  } = opts;

  const toOffset = makeOffsetMapper(host, quantum);

  // The subscribed-input bookkeeping: id -> { port, handler } so detach() can
  // unsubscribe and hotplug-remove can drop a gone device cleanly.
  const subscribed = new Map();

  const state = { granted: false, error: null };

  function subscribe(port) {
    if (!port || port.type !== "input") return;
    if (subscribed.has(port.id)) return; // already wired (re-statechange)
    const handler = (e) => {
      const offset = toOffset(e.timeStamp);
      decodeMidiMessage(host, e.data, offset);
    };
    port.onmidimessage = handler;
    subscribed.set(port.id, { port, handler });
  }

  function unsubscribe(id) {
    const rec = subscribed.get(id);
    if (!rec) return;
    try {
      rec.port.onmidimessage = null;
    } catch {}
    subscribed.delete(id);
  }

  if (!requestMIDIAccess) {
    state.error = new Error("Web MIDI unavailable (no requestMIDIAccess)");
    try {
      onError(state.error);
    } catch {}
    return {
      access: null,
      state,
      inputs: () => [],
      detach: () => {},
    };
  }

  let access;
  try {
    access = await requestMIDIAccess({ sysex });
  } catch (err) {
    state.error = err;
    try {
      onError(err);
    } catch {}
    return {
      access: null,
      state,
      inputs: () => [],
      detach: () => {},
    };
  }

  state.granted = true;

  // Wire every currently-present input.
  for (const port of access.inputs.values()) subscribe(port);

  // Hotplug. `statechange` fires on every connect/disconnect for inputs AND
  // outputs; we filter to inputs and (un)subscribe accordingly. A reconnected
  // device with the same id re-subscribes idempotently.
  const onStateChangeWrapped = (e) => {
    const port = e.port;
    if (port && port.type === "input") {
      if (port.state === "connected") subscribe(port);
      else unsubscribe(port.id); // "disconnected"
    }
    try {
      onStateChange(port);
    } catch {}
  };
  access.onstatechange = onStateChangeWrapped;

  return {
    access,
    state,
    inputs: () => Array.from(subscribed.values()).map((r) => r.port),
    detach() {
      try {
        access.onstatechange = null;
      } catch {}
      for (const id of Array.from(subscribed.keys())) unsubscribe(id);
    },
  };
}
