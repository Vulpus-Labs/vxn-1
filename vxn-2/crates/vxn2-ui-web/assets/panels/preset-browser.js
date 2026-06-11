// VXN2 glue for the shared preset browser. `createPresetBrowser` is defined
// by the shared module (vxn_core_ui_web::PRESET_BROWSER_JS) spliced into the
// bundle immediately before this file. Here we instantiate it with a VXN2
// bridge adapter and register it as a panel; main.js binds it in boot and
// routes corpus / preset_loaded / follow events to it.

(function () {
  var vxn = (window.__vxn = window.__vxn || {});
  vxn.panels = vxn.panels || {};

  function d(op, payload) {
    if (vxn.dispatch) vxn.dispatch(op, payload || {});
  }
  var send = {
    loadFactory: function (index) { d("load_factory", { index: index }); },
    loadUser: function (path) { d("load_user", { path: path }); },
    renamePreset: function (path, new_name) { d("rename_preset", { path: path, new_name: new_name }); },
    deletePreset: function (path) { d("delete_preset", { path: path }); },
    movePreset: function (path, dest_folder) { d("move_preset", { path: path, dest_folder: dest_folder }); },
    renameFolder: function (old_name, new_name) { d("rename_folder", { old_name: old_name, new_name: new_name }); },
    deleteFolder: function (name) { d("delete_folder", { name: name }); },
    newFolder: function (suggested) { d("new_folder", { suggested: suggested }); },
    savePreset: function (name, folder) { d("save_preset", { name: name, folder: folder }); },
  };

  vxn.panels.presetBrowser = createPresetBrowser({
    send: send,
    // VXN2's text input is Promise-based; adapt to the shared callback shape.
    promptText: function (title, initial, cb) {
      var p =
        typeof vxn.dispatchTextInput === "function"
          ? vxn.dispatchTextInput(title, initial)
          : Promise.resolve(window.prompt(title, initial == null ? "" : initial));
      p.then(function (v) { cb(v); });
    },
    faceplateRoot: function () {
      return document.querySelector('[data-vxn-section="faceplate"]') || document.body;
    },
  });
})();
