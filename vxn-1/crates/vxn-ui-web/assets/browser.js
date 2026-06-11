// VXN1 glue for the shared preset browser. The browser logic itself now
// lives in `vxn_core_ui_web::PRESET_BROWSER_JS` (assets/preset-browser.js in
// the vxn-core-ui-web crate), spliced into this page's inline <script>
// immediately before this file — so `createPresetBrowser` is in scope here.
//
// This glue instantiates it against VXN1's bridge (`window.vxn.send`,
// `window.vxn.promptText`, `#faceplate`) and exposes the `browserPanel`
// const that panels.js (Browse / Save / Save As) and dispatch.js
// (preset_loaded → setCurrentSource, preset_corpus_changed → followPath)
// reference. Finally it replaces the bootstrap `applyPresetCorpus` stub and
// drains any corpus snapshot that arrived during bootstrap.
//
// The pure helpers (`folderOptions`, `moveTargets`, `folderValue`,
// `UNCATEGORISED`) and `createPresetBrowser` are unit-tested directly off the
// shared module by the vitest suite — see __tests__/_helpers.js.

const browserPanel = createPresetBrowser({
  // Late-bind through `window.vxn` so this matches the original's live
  // global references (and tolerates a bridge that swaps these out).
  send: window.vxn.send,
  promptText: (title, initial, cb) => window.vxn.promptText(title, initial, cb),
  faceplateRoot: () => document.getElementById('faceplate'),
});
browserPanel.bind();

if (typeof window !== 'undefined' && window.__vxn) {
  window.__vxn.applyPresetCorpus = (snap) => browserPanel.setCorpus(snap);
  if (typeof _earlyPresetCorpus !== 'undefined' && _earlyPresetCorpus) {
    browserPanel.setCorpus(_earlyPresetCorpus);
    _earlyPresetCorpus = null;
  }
}
