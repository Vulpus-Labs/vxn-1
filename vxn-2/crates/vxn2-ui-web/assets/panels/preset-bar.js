// Preset bar (ticket 0029). Owns:
//   - name display, fed by `preset_loaded`
//   - status toast, fed by `status`
//   - browse <dialog> open/close (no IPC — empty-state stub)
//   - save-as text-input round trip via `vxn.dispatchTextInput` (0030);
//     the Promise resolves with the committed name (or null) and the
//     bar dispatches `save_preset` directly — no correlation token,
//     no router callback registered with main.js.

(function () {
  var TOAST_DURATION_MS = 2500;

  function presetBar() {
    var nameEl = null;
    var toastEl = null;
    var dialogEl = null;
    var dialogCloseEl = null;
    var currentName = "Init";
    var toastTimer = 0;

    function bind(root, ctx) {
      nameEl = root.querySelector('[data-vxn-section="preset-name"]');
      toastEl = root.querySelector('[data-vxn-section="toast"]');
      dialogEl = root.querySelector('[data-vxn-section="browse-dialog"]');
      dialogCloseEl = root.querySelector('.vxn-browse-close');
      setName(currentName);

      // Buttons are bound by `main.js::bindCustoms`; the panel just owns
      // the side effects that don't have a 1:1 opcode mapping.
      var saveBtn = root.querySelector('[data-vxn-custom="preset_save"]');
      if (saveBtn) {
        saveBtn.addEventListener("click", function () {
          ctx.dispatch("save_preset", { name: currentName });
        });
      }
      var saveAsBtn = root.querySelector('[data-vxn-custom="preset_save_as"]');
      if (saveAsBtn) {
        saveAsBtn.addEventListener("click", function () {
          window.__vxn.dispatchTextInput("Save Preset As", currentName)
            .then(function (value) {
              if (value == null) return;
              var name = String(value).trim();
              if (name.length === 0) return;
              ctx.dispatch("save_preset", { name: name, folder: null });
            });
        });
      }
      var browseBtn = root.querySelector('[data-vxn-custom="preset_browse"]');
      if (browseBtn) {
        browseBtn.addEventListener("click", openBrowse);
      }
      if (dialogCloseEl) {
        dialogCloseEl.addEventListener("click", closeBrowse);
      }
      if (dialogEl) {
        // Outside-click dismissal: the dialog's backdrop is part of the
        // <dialog> element so clicks on the host land on dialogEl itself.
        dialogEl.addEventListener("click", function (ev) {
          if (ev.target === dialogEl) closeBrowse();
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

    function openBrowse() {
      if (!dialogEl) return;
      if (typeof dialogEl.showModal === "function") {
        if (!dialogEl.open) dialogEl.showModal();
      } else {
        // Legacy fallback for WebViews without <dialog> showModal.
        dialogEl.setAttribute("open", "");
      }
    }

    function closeBrowse() {
      if (!dialogEl) return;
      if (typeof dialogEl.close === "function") {
        if (dialogEl.open) dialogEl.close();
      } else {
        dialogEl.removeAttribute("open");
      }
    }

    function onView(ev) {
      if (!ev) return;
      if (ev.kind === "preset_loaded") {
        setName(ev.name);
      } else if (ev.kind === "status") {
        showToast(ev.line);
      }
    }

    return {
      bind: bind,
      onView: onView,
      setName: setName,
      showToast: showToast,
      openBrowse: openBrowse,
      closeBrowse: closeBrowse,
      // Read-only views for tests.
      _currentName: function () { return currentName; },
    };
  }

  window.__vxn = window.__vxn || {};
  window.__vxn.panels = window.__vxn.panels || {};
  window.__vxn.panels.presetBar = presetBar();
})();
