// VXN2 page bootstrap. Lives at the bottom of index.html — full panels JS
// arrives in 0026.
//
// Responsibilities until 0026:
//   - Expose `window.__vxn` with `applyViewEvents` + `applyPresetCorpus`
//     no-op stubs so the Rust-side flush calls don't throw.
//   - Post a `ready` opcode after DOMContentLoaded so the controller's
//     `EditorReady` handler fires and seeds the first param broadcast.

(function () {
  function dispatch(opcode, payload) {
    var msg = Object.assign({ op: opcode }, payload || {});
    try {
      window.ipc.postMessage(JSON.stringify(msg));
    } catch (e) {
      console.error("vxn2 ipc post failed", e);
    }
  }

  window.__vxn = {
    params: {},
    paramsByCladId: {},
    dispatch: dispatch,
    applyViewEvents: function (events) {
      // 0026 will dispatch these into the panel renderers.
      window.__vxn._lastBatch = events;
    },
    applyPresetCorpus: function (corpus) {
      // 0029 wires the browser modal to this.
      window.__vxn._lastCorpus = corpus;
    },
  };

  function announceReady() {
    dispatch("ready");
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", announceReady, { once: true });
  } else {
    announceReady();
  }
})();
