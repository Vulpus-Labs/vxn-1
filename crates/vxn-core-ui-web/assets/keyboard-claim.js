// Keyboard claim (0364).
//
// The Rust shell hands the host back the keyboard on every editor tick — see
// `vxn-core-ui-web/src/focus.rs`. Without that, clicking the faceplate parks
// native focus on the WebView and the DAW stops seeing spacebar, its computer-
// MIDI keyboard and its shortcuts (reported against Ableton Live and Logic; not
// reproducible in REAPER).
//
// The page still needs the keyboard occasionally, so this module is the other
// half of the deal: while anything here holds a claim, the shell leaves focus
// alone. Two ways to claim:
//
//   - automatically, while a text-entry element has DOM focus (the preset
//     browser's search field is the only one the plugins ship — rename /
//     save-as / new-folder go through the native popup instead);
//   - explicitly, via `window.__vxnKeyboard.claim(token)` /
//     `.release(token)`, for an overlay that reads Escape or arrow keys.
//
// Claims are a token set, not a boolean, so a picker opened over a modal can't
// release the modal's claim when it closes. Only the transitions are sent, so a
// steady state costs no IPC.
//
// Auto-installs at the bottom of this file when spliced into a faceplate (it
// looks for wry's `window.ipc`); the Node suite imports the factory and drives
// it against injected fakes instead.

/// Opcode the shell intercepts in its IPC handler — never reaches the model.
export const KEYBOARD_CLAIM_OP = 'want_keyboard';

/// Token used for the automatic text-entry claim. Exported so the suite can
/// assert on it rather than matching a bare string.
export const TEXT_ENTRY_TOKEN = '__text__';

/// `<input>` types that carry no typed text — a fader, a switch, a colour well.
/// Focusing one of these must NOT take the keyboard off the host.
const NON_TEXT_INPUT_TYPES = new Set([
  'range',
  'checkbox',
  'radio',
  'button',
  'submit',
  'reset',
  'file',
  'color',
  'image',
]);

/// Whether `el` is somewhere the user types. Anything else — a knob, the body,
/// a `<button>` — leaves the keyboard with the host.
export function isTextEntry(el) {
  if (!el) return false;
  if (el.isContentEditable) return true;
  const tag = (el.tagName || '').toUpperCase();
  if (tag === 'TEXTAREA') return true;
  if (tag !== 'INPUT') return false;
  const type = (el.getAttribute && el.getAttribute('type')) || el.type || 'text';
  return !NON_TEXT_INPUT_TYPES.has(String(type).toLowerCase());
}

/// Build a claim tracker. Everything it touches is injectable so the suite can
/// run it without a WebView:
///
///   `post`      — sends one `{op, on}` message (default: wry's `window.ipc`)
///   `document`  — focus event source (default: the global)
///   `window`    — blur event source (default: the global)
///   `defer`     — schedules the focus-out re-check (default: `queueMicrotask`)
export function createKeyboardClaim(opts) {
  const o = opts || {};
  const doc = o.document || (typeof document !== 'undefined' ? document : null);
  const win = o.window || (typeof window !== 'undefined' ? window : null);
  const defer =
    o.defer ||
    (typeof queueMicrotask === 'function' ? queueMicrotask : (fn) => setTimeout(fn, 0));
  const post =
    o.post ||
    function (msg) {
      try {
        win.ipc.postMessage(JSON.stringify(msg));
      } catch (e) {
        console.warn('vxn keyboard claim failed', e);
      }
    };

  const tokens = new Set();
  // `null` until the first sync, so the initial state is always sent once and
  // the shell can't be left guessing after a page reload.
  let sent = null;

  function sync() {
    const on = tokens.size > 0;
    if (on === sent) return;
    sent = on;
    post({ op: KEYBOARD_CLAIM_OP, on });
  }

  function claim(token) {
    tokens.add(token);
    sync();
  }

  function release(token) {
    tokens.delete(token);
    sync();
  }

  function onFocusIn(ev) {
    if (isTextEntry(ev && ev.target)) claim(TEXT_ENTRY_TOKEN);
  }

  // Deferred: `focusout` fires BEFORE the next element's `focusin`, so
  // releasing here would drop the claim for a beat while tabbing between two
  // fields — long enough for the shell's ~60 Hz tick to yank focus mid-word.
  // Re-check what actually ended up focused instead.
  function onFocusOut() {
    defer(() => {
      if (!isTextEntry(doc && doc.activeElement)) release(TEXT_ENTRY_TOKEN);
    });
  }

  // The host window took focus back (clicked the arrangement, switched app).
  // Drop the text claim — whatever was focused in the page isn't being typed
  // into any more. Explicit overlay claims survive: their overlay is still
  // open, and holding a claim while the host has focus costs nothing.
  function onWindowBlur() {
    release(TEXT_ENTRY_TOKEN);
  }

  function listen(on) {
    const fn = on ? 'addEventListener' : 'removeEventListener';
    if (doc) {
      doc[fn]('focusin', onFocusIn, true);
      doc[fn]('focusout', onFocusOut, true);
    }
    if (win) win[fn]('blur', onWindowBlur);
  }

  listen(true);

  return {
    claim,
    release,
    isTextEntry,
    /// Current claim state — for tests and for a panel that wants to know.
    get held() {
      return tokens.size > 0;
    },
    /// Snapshot of the held tokens, for diagnosing a stuck claim.
    tokens: () => Array.from(tokens),
    /// Tear the listeners down. Not used by the faceplates (the page dies with
    /// the WebView) — it exists so the suite can't leak handlers between cases.
    dispose() {
      listen(false);
      tokens.clear();
      sent = null;
    },
  };
}

/// Install the singleton the panels talk to. Idempotent: a second call returns
/// the existing tracker rather than stacking a second set of focus listeners.
export function installKeyboardClaim(opts) {
  const g = (opts && opts.window) || (typeof window !== 'undefined' ? window : null);
  if (!g) return null;
  if (g.__vxnKeyboard) return g.__vxnKeyboard;
  g.__vxnKeyboard = createKeyboardClaim(opts);
  return g.__vxnKeyboard;
}

// Auto-install when spliced into a faceplate. `window.ipc` is wry's injected
// IPC object, so this is false in the Node suite and in any non-WebView host —
// those pull in the factory above and inject their own `post`.
//
// (Phrasing matters: the faceplate crates assert the spliced page carries no
// bare `im`+`port ` token, and a comment counts.)
if (
  typeof window !== 'undefined' &&
  window.ipc &&
  typeof window.ipc.postMessage === 'function'
) {
  installKeyboardClaim();
}
