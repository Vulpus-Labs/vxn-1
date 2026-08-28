// web-boot-bpm.js — the WEB-ONLY tempo control (E045 delta 5).
//
// The plugin takes BPM from its host transport; a browser has no host, and
// `sync.rs` resolves every LFO and delay subdivision against a tempo, so
// without this the synced rates are stuck at the default forever.
//
// It lives in the bottom-left web-chrome row beside the CPU meter rather than
// on the faceplate: both are host-shaped readouts about the browser rather
// than parts of the instrument, so they belong together and away from the
// panels.
//
// Split out of `WEB_BOOT_HEAD`'s Rust string literal in 0320. Everything else
// in that literal is a template — it carries the double-underscore
// substitution tokens `assemble_faceplate` fills in, so it cannot be valid JS
// on its own. This half is, which is what lets the vitest glob reach it. It is
// spliced into a plain `<script>`, so the `export` markers are stripped on the
// way in — same treatment as every other module in `assets/`.
//
// Do not write a substitution token literally in this file, even inside a
// comment: the splice is a blind textual replace over the whole page, so the
// token would be filled in here too. `no_asset_contains_a_placeholder_token`
// enforces that.

/// Clamp a typed tempo to the range the engine will accept. `null` means "post
/// nothing".
///
/// Non-finite is rejected outright rather than clamped: it is not a slider that
/// ran past its end, it is a value that would divide through every synced rate
/// in the engine.
///
/// Blank is rejected for a subtler reason found when this got its first test in
/// 0320. `Number('')` is `0`, not `NaN`, so the original `isFinite` check let an
/// empty field through and clamped it to 20 — and a `<input type=number>` reads
/// back as `''` whenever its contents are invalid, so *clearing the box, or
/// typing a letter, slammed the tempo to 20 BPM*. Emptying a field is not a
/// tempo edit.
export function clampBpm(raw) {
  if (raw == null) return null;
  if (typeof raw === 'string' && raw.trim() === '') return null;
  const bpm = Number(raw);
  if (!Number.isFinite(bpm)) return null;
  return Math.min(300, Math.max(20, bpm));
}

/// The shared bottom-left chrome row, created if the CPU meter has not mounted
/// yet — it boots asynchronously and this runs on DOMContentLoaded, so either
/// can be first. Same id, same styles, both idempotent; `order` decides the
/// layout rather than mount order.
export function webChromeRow(doc) {
  let row = doc.getElementById('vxn-web-chrome');
  if (row) return row;
  row = doc.createElement('div');
  row.id = 'vxn-web-chrome';
  row.style.cssText =
    'position:fixed;left:10px;bottom:102px;z-index:9999;' +
    'display:flex;align-items:center;gap:8px;';
  doc.body.appendChild(row);
  return row;
}

/// Mount the BPM control and seed the engine with its starting value.
///
/// Posts through the same `window.ipc` as every other opcode, so it queues
/// before the bridge is up and routes to the ring afterwards — tempo has no
/// model presence, so there is no echo that could carry it.
export function mountBpmControl(doc, ipc, defaultBpm) {
  const row = webChromeRow(doc);
  const wrap = doc.createElement('div');
  wrap.className = 'vxn-bpm';
  const label = doc.createElement('span');
  label.textContent = 'BPM';
  const input = doc.createElement('input');
  input.type = 'number';
  input.min = '20';
  input.max = '300';
  input.step = '1';
  input.value = String(defaultBpm);

  const send = () => {
    const bpm = clampBpm(input.value);
    if (bpm == null) return;
    // Write the clamped value back, so the field shows what was actually sent.
    input.value = String(bpm);
    ipc.postMessage(JSON.stringify({ op: 'set_tempo', bpm }));
  };
  input.addEventListener('change', send);
  // Typing a tempo must not trigger the faceplate's single-key shortcuts.
  input.addEventListener('keydown', (ev) => ev.stopPropagation());

  wrap.appendChild(label);
  wrap.appendChild(input);
  row.appendChild(wrap);
  // Seed the engine so a synced LFO is right before anyone touches this.
  send();
  return { input, send };
}
