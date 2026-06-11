// Preset bar. Owns:
//   - name display, fed by `preset_loaded`
//   - status toast, fed by `status`
//   - prev/next are plain CUSTOM_OPS in main.js (step_preset)
//   - Browse / Save / Save As delegate to the preset-browser panel:
//       Browse   -> presetBrowser.setOpen(toggle)
//       Save As  -> presetBrowser.openSaveAs(currentName)  (name + folder modal)
//       Save     -> overwrite the loaded user preset in its own folder
//                   (presetBrowser.folderForUserPath); falls back to Save As
//                   when the current preset is factory / unsaved.
//
// `currentSource` mirrors the browser's: { kind: "factory", index } |
// { kind: "user", path } | null, taken from each `preset_loaded` event.

(function () {
  var TOAST_DURATION_MS = 2500;

  function presetBar() {
    var nameEl = null;
    var toastEl = null;
    var currentName = "Init";
    var currentSource = null;
    var toastTimer = 0;

    function browser() {
      return (window.__vxn && window.__vxn.panels && window.__vxn.panels.presetBrowser) || null;
    }

    function bind(root, ctx) {
      nameEl = root.querySelector('[data-vxn-section="preset-name"]');
      toastEl = root.querySelector('[data-vxn-section="toast"]');
      setName(currentName);

      var browseBtn = root.querySelector('[data-vxn-custom="preset_browse"]');
      if (browseBtn) {
        browseBtn.addEventListener("click", function () {
          var b = browser();
          if (b) b.setOpen(!b.isOpen());
        });
        var b0 = browser();
        if (b0 && b0.onOpenChange) {
          b0.onOpenChange(function (open) {
            browseBtn.classList.toggle("active", open);
          });
        }
      }

      var saveAsBtn = root.querySelector('[data-vxn-custom="preset_save_as"]');
      if (saveAsBtn) {
        saveAsBtn.addEventListener("click", function () {
          var b = browser();
          if (b && b.openSaveAs) {
            b.openSaveAs(currentName);
          } else {
            // Defensive fallback: bare name prompt -> save to root.
            window.__vxn.dispatchTextInput("Save Preset As", currentName).then(function (value) {
              if (value == null) return;
              var name = String(value).trim();
              if (name) ctx.dispatch("save_preset", { name: name, folder: null });
            });
          }
        });
      }

      var saveBtn = root.querySelector('[data-vxn-custom="preset_save"]');
      if (saveBtn) {
        saveBtn.addEventListener("click", function () {
          var b = browser();
          // Overwrite only makes sense for a loaded user preset; resolve its
          // folder from the corpus so Save writes back in place. Anything
          // else (factory / unsaved Init) routes to Save As.
          if (b && currentSource && currentSource.kind === "user") {
            var folder = b.folderForUserPath(currentSource.path);
            // Path missing from corpus (racing refresh / moved file): refuse
            // rather than silently fork the preset to the user root.
            if (folder === undefined) return;
            ctx.dispatch("save_preset", { name: currentName, folder: folder });
            return;
          }
          if (b && b.openSaveAs) {
            b.openSaveAs(currentName);
          } else {
            ctx.dispatch("save_preset", { name: currentName, folder: null });
          }
        });
      }
    }

    function setName(name) {
      currentName = name || "";
      if (nameEl) nameEl.textContent = currentName;
    }

    function showToast(line) {
      if (!toastEl) return;
      toastEl.textContent = line || "";
      toastEl.classList.add("vxn-toast-visible");
      if (toastTimer) clearTimeout(toastTimer);
      toastTimer = setTimeout(function () {
        toastEl.classList.remove("vxn-toast-visible");
        toastTimer = 0;
      }, TOAST_DURATION_MS);
    }

    function onView(ev) {
      if (!ev) return;
      if (ev.kind === "preset_loaded") {
        setName(ev.name);
        currentSource = ev.source || null;
      } else if (ev.kind === "status") {
        showToast(ev.line);
      }
    }

    return {
      bind: bind,
      onView: onView,
      setName: setName,
      showToast: showToast,
      // Read-only views for tests.
      _currentName: function () { return currentName; },
      _currentSource: function () { return currentSource; },
    };
  }

  window.__vxn = window.__vxn || {};
  window.__vxn.panels = window.__vxn.panels || {};
  window.__vxn.panels.presetBar = presetBar();
})();
