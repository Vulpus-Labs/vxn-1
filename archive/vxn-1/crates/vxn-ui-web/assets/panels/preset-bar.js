// panels/preset-bar.js — the preset bar (prev/next/browse/save/save-as +
// dirty-tracking).
//
// Prev/next post `step_preset` with a signed delta — the controller walks
// the combined Factory + User list and emits a fresh `preset_loaded` for
// us to pick up. The current name comes from the same `preset_loaded`
// dispatch branch in `init()`. Browse toggles the 0050 browser panel
// (open/close mirrored back via the panel's `onOpenChange` so the button's
// `.active` class stays in sync with click-outside / ESC dismissals).
// Save As opens the 0048 popup and on commit posts `save_preset` using
// the browser panel's currently-selected user folder as the destination
// (factory selections collapse to user root — there's no write target
// inside the factory bank).
//
// The `../bridge.js` side-effect `import` installs `window.vxn` (the `send`
// table + the `onMutation` dirty hook this panel registers). Stripped at
// splice time (bridge.js is concatenated ahead of this module); under Node
// ESM it ensures bridge.js evaluates first. `browserPanel` is a concat-global
// from browser.js (spliced before the panels), referenced only after the
// no-DOM stub guard so pure-helper test imports don't crash.
import '../bridge.js';

export const presetBar = (() => {
  const nameEl   = document.getElementById('pbar-name');
  // E015 / 0077: under Node ESM `import` (no faceplate DOM, no concatenated
  // `browserPanel` global), bail out with a stub so pure-helper test
  // imports don't crash on `browserPanel.onOpenChange(...)` below.
  if (!nameEl) return { setName() {}, setSource() {}, markDirty() {} };
  const prevEl     = document.getElementById('pbar-prev');
  const nextEl     = document.getElementById('pbar-next');
  const browseEl   = document.getElementById('pbar-browse');
  const saveAsEl   = document.getElementById('pbar-save');
  const saveEl     = document.getElementById('pbar-save-overwrite');
  let currentName  = '';
  // 0094: Save (overwrite) gates on a) loaded source is user, b) dirty.
  // `currentSource` mirrors browser.js's: { kind: 'factory', index } |
  // { kind: 'user', path }. `dirty` flips on the next user-initiated
  // mutation after a load and resets on every fresh preset_loaded.
  let currentSource = null;
  let dirty = false;

  function refreshSaveBtn() {
    if (!saveEl) return;
    const enabled = dirty && currentSource && currentSource.kind === 'user';
    saveEl.disabled = !enabled;
  }
  function setName(name) {
    currentName = name || '';
    if (nameEl) nameEl.textContent = currentName;
  }
  function setSource(src) {
    currentSource = src || null;
    dirty = false;
    refreshSaveBtn();
  }
  function markDirty() {
    if (dirty) return;
    dirty = true;
    refreshSaveBtn();
  }

  // Any engine-mutating UI write flips dirty. Registered on the bridge's
  // first-class `onMutation` hook (0141) instead of monkey-patching
  // `window.vxn.send.*`: the hook is additive, so this composes with any
  // other subscriber rather than overwriting the shared senders. The
  // mutating senders (set_param / set_param_norm / reset_layer /
  // set_key_mode / set_split_point, and `discrete` via set_param) fire it;
  // view-only sends (gestures, edit-layer, preset load) don't. `setEditLayer`
  // is pure view state — correctly not a mutation, so it never marks dirty.
  if (window.vxn && typeof window.vxn.onMutation === 'function') {
    window.vxn.onMutation(markDirty);
  }

  if (prevEl) {
    prevEl.addEventListener('click', () => window.vxn.send.stepPreset(-1));
  }
  if (nextEl) {
    nextEl.addEventListener('click', () => window.vxn.send.stepPreset(1));
  }
  if (browseEl) {
    browseEl.addEventListener('click', () => browserPanel.setOpen(!browserPanel.isOpen()));
  }
  browserPanel.onOpenChange((open) => {
    if (browseEl) browseEl.classList.toggle('active', open);
  });
  if (saveAsEl) {
    // Save As opens the combined name + folder modal. The name field
    // funnels through the existing native popup (`promptText`) for
    // spacebar-safe entry; the folder dropdown is mouse-driven (no kbd
    // capture concern). The modal anchors over the faceplate, so it
    // works whether or not the browser panel itself is open.
    saveAsEl.addEventListener('click', () => browserPanel.openSaveAs(currentName));
  }
  if (saveEl) {
    saveEl.addEventListener('click', () => {
      if (saveEl.disabled) return;
      if (!currentSource || currentSource.kind !== 'user') return;
      const folder = browserPanel.folderForUserPath(currentSource.path);
      // Path missing from corpus (race against a refresh, or a moved file
      // we haven't re-anchored on): refuse rather than fall through to
      // user root and silently fork the preset. The Save As path stays
      // available for explicit relocation.
      if (folder === undefined) return;
      window.vxn.send.savePreset(currentName, folder);
      // Optimistic: assume the controller's write succeeded. A failed
      // save will surface as a `save failed: …` status flash but won't
      // re-mark the patch dirty — the next param wiggle will.
      dirty = false;
      refreshSaveBtn();
    });
  }
  return { setName, setSource, markDirty };
})();
