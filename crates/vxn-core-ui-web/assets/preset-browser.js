// Shared preset browser — floating two-pane folders/presets panel.
//
// Synth-agnostic: it speaks only the shared preset corpus shape (from
// `corpus_snapshot_json`) and the shared opcode vocabulary, and is wired to a
// host via an injected adapter, so VXN1 and VXN2 embed the same code with
// different bridges. Each synth's faceplate crate splices this file (ESM
// markers stripped) into its inline <script>, then a tiny per-synth glue
// calls `createPresetBrowser(cfg)` and registers the result where its bridge
// expects it.
//
// Features: two-pane FACTORY categories + USER folders / presets, flat
// case-insensitive search with origin labels, right-click context menu
// (Rename / Delete / Move-to ▸ on user presets; Rename / Delete on named
// folders), "+ New" folder, HTML5 drag-and-drop preset→folder, delete-confirm
// + Save-As (name + folder dropdown) modals, and follow-path after move/rename.
//
// cfg: {
//   send: { loadFactory(index), loadUser(path), renamePreset(path,newName),
//           deletePreset(path), movePreset(path,destFolder|null),
//           renameFolder(old,new), deleteFolder(name), newFolder(suggested),
//           savePreset(name, folder|null) },
//   promptText: (title, initial, cb) => void,   // cb(value|null)
//   faceplateRoot: () => Element,               // modal mount target
// }
//
// The DOM contract is fixed element ids (both synths use them):
//   #browser-panel #browser-backdrop #browser-folders #browser-presets
//   #browser-search-input #browser-search-clear #browser-close
// `bind(root)` resolves them within `root || document` and wires listeners.

export const UNCATEGORISED = 'Uncategorised';

// Map a folder name to the <select> value used in Save As. The virtual root
// has no real name; sentinel it as `__root__`.
export function folderValue(name) {
  return name == null ? '__root__' : name;
}

// Dropdown options for the Save As folder selector: virtual root first, then
// alpha-sorted named user folders.
export function folderOptions(corpus) {
  const named = [];
  for (const g of (corpus && corpus.user) || []) {
    if (g.name == null) continue;
    named.push(g.name);
  }
  named.sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase()));
  const out = [{ value: '__root__', label: UNCATEGORISED }];
  for (const n of named) out.push({ value: n, label: n });
  return out;
}

// Move-target list for a user preset's context submenu: Uncategorised first
// (if a root group exists and we're not already in root), then alpha-sorted
// named folders excluding the current one.
export function moveTargets(currentName, corpus) {
  const out = [];
  let hasRoot = false;
  const named = [];
  for (const g of (corpus && corpus.user) || []) {
    if (g.name == null) { hasRoot = true; continue; }
    named.push(g.name);
  }
  named.sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase()));
  if (hasRoot && currentName !== null) {
    out.push({ name: null, label: UNCATEGORISED });
  }
  for (const n of named) {
    if (n === currentName) continue;
    out.push({ name: n, label: n });
  }
  return out;
}

export function createPresetBrowser(cfg) {
  const send = cfg.send;
  const promptText = cfg.promptText;
  const faceplateRoot = cfg.faceplateRoot || (() => document.body);

  let panelEl = null;
  let backdropEl = null;
  let foldersEl = null;
  let presetsEl = null;
  let inputEl = null;
  let clearEl = null;
  let closeEl = null;
  let bound = false;

  let corpus = { factory: [], user: [] };
  let selectedFolder = { kind: 'user', name: null };
  let query = '';
  let currentSource = null;
  let isOpen = false;
  let onOpenChange = null;

  let menuEl = null;
  let modalEl = null;

  // HTML5 DnD state. `dragSourcePath` is non-null while dragging a user
  // preset row; `dragSourceFolder` is the folder it came from. dataTransfer
  // can't be read during `dragover`, so in-page logic reads these.
  let dragSourcePath = null;
  let dragSourceFolder = null;

  const FACTORY_HEADER = 'FACTORY';
  const USER_HEADER = 'USER';

  function bind(root) {
    const scope = root || document;
    panelEl = scope.querySelector('#browser-panel');
    if (!panelEl) return;
    backdropEl = scope.querySelector('#browser-backdrop');
    foldersEl = scope.querySelector('#browser-folders');
    presetsEl = scope.querySelector('#browser-presets');
    inputEl = scope.querySelector('#browser-search-input');
    clearEl = scope.querySelector('#browser-search-clear');
    closeEl = scope.querySelector('#browser-close');
    bound = true;

    if (inputEl) {
      inputEl.addEventListener('input', () => {
        query = inputEl.value || '';
        renderPresets();
      });
    }
    if (clearEl) {
      clearEl.addEventListener('click', () => {
        if (inputEl) inputEl.value = '';
        query = '';
        renderPresets();
        if (inputEl) inputEl.focus();
      });
    }
    if (backdropEl) backdropEl.addEventListener('click', () => setOpen(false));
    if (closeEl) {
      closeEl.addEventListener('click', (e) => {
        e.stopPropagation();
        setOpen(false);
      });
    }
    // Click inside the panel that isn't the menu closes the menu.
    panelEl.addEventListener('click', (e) => {
      if (menuEl && !menuEl.contains(e.target)) closeMenu();
    });
    document.addEventListener('keydown', (e) => {
      if (e.key !== 'Escape') return;
      if (!isOpen) return;
      e.preventDefault();
      // ESC closes one level: modal → menu → panel.
      if (modalEl) { closeModal(); return; }
      if (menuEl) { closeMenu(); return; }
      setOpen(false);
    });
    renderFolders();
    renderPresets();
  }

  function setCorpus(snap) {
    corpus = snap || { factory: [], user: [] };
    if (!Array.isArray(corpus.factory)) corpus.factory = [];
    if (!Array.isArray(corpus.user)) corpus.user = [];
    if (!folderExists(selectedFolder)) {
      selectedFolder = { kind: 'user', name: null };
    }
    // Any in-flight menu / modal is stale after a corpus change.
    closeMenu();
    closeModal();
    if (bound) {
      renderFolders();
      renderPresets();
    }
  }
  function folderExists(key) {
    if (!key) return false;
    const list = key.kind === 'factory' ? corpus.factory : corpus.user;
    if (!Array.isArray(list)) return false;
    for (const g of list) {
      const gn = key.kind === 'factory' ? g.category : g.name;
      if (gn === key.name || (gn == null && key.name == null)) return true;
    }
    return false;
  }
  function folderLabel(key, group) {
    if (key.kind === 'factory') return group.category || UNCATEGORISED;
    return group.name || UNCATEGORISED;
  }
  function renderFolders() {
    if (!foldersEl) return;
    foldersEl.innerHTML = '';
    appendSection(FACTORY_HEADER, null);
    for (const g of corpus.factory) {
      appendFolderRow({ kind: 'factory', name: g.category }, folderLabel({ kind: 'factory' }, g));
    }
    appendSection(USER_HEADER, () => {
      promptText('New folder', 'New Folder', (value) => {
        if (value == null) return;
        const trimmed = value.trim();
        if (!trimmed) return;
        send.newFolder(trimmed);
      });
    });
    for (const g of corpus.user) {
      appendFolderRow({ kind: 'user', name: g.name }, folderLabel({ kind: 'user' }, g));
    }
  }
  function appendSection(text, onNewFolder) {
    if (onNewFolder) {
      const row = document.createElement('div');
      row.className = 'browser-section-row';
      const h = document.createElement('div');
      h.className = 'browser-section';
      h.textContent = text;
      row.appendChild(h);
      const btn = document.createElement('button');
      btn.type = 'button';
      btn.className = 'browser-new-folder';
      btn.textContent = '+ New';
      btn.addEventListener('click', (e) => {
        e.stopPropagation();
        closeMenu();
        onNewFolder();
      });
      row.appendChild(btn);
      foldersEl.appendChild(row);
    } else {
      const h = document.createElement('div');
      h.className = 'browser-section';
      h.textContent = text;
      foldersEl.appendChild(h);
    }
  }
  function appendFolderRow(key, label) {
    const r = document.createElement('div');
    r.className = 'browser-row';
    r.textContent = label;
    if (selectedFolder.kind === key.kind && selectedFolder.name === key.name) {
      r.classList.add('selected');
    }
    r.addEventListener('click', () => {
      selectedFolder = key;
      closeMenu();
      renderFolders();
      renderPresets();
    });
    // Only named user folders carry a context menu. The virtual root
    // represents the user dir itself.
    if (key.kind === 'user' && key.name != null) {
      r.addEventListener('contextmenu', (e) => {
        e.preventDefault();
        openMenu(e, { kind: 'folder', name: key.name });
      });
    }
    // Every user folder (incl. root) is a drop target; factory folders are
    // not (default behaviour rejects the drop).
    if (key.kind === 'user') {
      r.addEventListener('dragover', (e) => {
        if (dragSourcePath == null) return;
        if (key.name === dragSourceFolder) {
          r.classList.add('drag-blocked');
          return;
        }
        e.preventDefault();
        e.dataTransfer.dropEffect = 'move';
        r.classList.add('drag-over');
      });
      r.addEventListener('dragleave', () => {
        r.classList.remove('drag-over', 'drag-blocked');
      });
      r.addEventListener('drop', (e) => {
        e.preventDefault();
        r.classList.remove('drag-over', 'drag-blocked');
        if (dragSourcePath == null) return;
        if (key.name === dragSourceFolder) return;
        send.movePreset(dragSourcePath, key.name);
      });
    }
    foldersEl.appendChild(r);
  }
  function findGroup() {
    const list = selectedFolder.kind === 'factory' ? corpus.factory : corpus.user;
    for (const g of list) {
      const gn = selectedFolder.kind === 'factory' ? g.category : g.name;
      if (gn === selectedFolder.name) return g;
    }
    return null;
  }
  function renderPresets() {
    if (!presetsEl) return;
    presetsEl.innerHTML = '';
    const q = query.trim().toLowerCase();
    if (q) {
      const hits = collectSearchHits(q);
      if (hits.length === 0) { appendEmpty('No matches'); return; }
      for (const h of hits) appendSearchRow(h);
      return;
    }
    const group = findGroup();
    if (!group) { appendEmpty('No presets'); return; }
    for (const p of group.presets) {
      const r = document.createElement('div');
      r.className = 'browser-row';
      r.textContent = p.name;
      if (isCurrent(p)) r.classList.add('current');
      r.addEventListener('click', () => {
        closeMenu();
        loadEntry(p);
      });
      if (selectedFolder.kind === 'user') {
        r.dataset.path = p.path;
        r.addEventListener('contextmenu', (e) => {
          e.preventDefault();
          openMenu(e, { kind: 'preset', path: p.path, name: p.name, folder: selectedFolder.name });
        });
        wirePresetDragSource(r, p.path, selectedFolder.name);
      }
      presetsEl.appendChild(r);
    }
    if (!group.presets.length) appendEmpty('No presets');
  }
  function collectSearchHits(q) {
    const out = [];
    for (const g of corpus.factory) {
      const cat = g.category || UNCATEGORISED;
      for (const p of g.presets) {
        if (!p.name.toLowerCase().includes(q)) continue;
        out.push({ name: p.name, source: { kind: 'factory', index: p.index }, origin: 'Factory · ' + cat });
      }
    }
    for (const g of corpus.user) {
      const folder = g.name || UNCATEGORISED;
      for (const p of g.presets) {
        if (!p.name.toLowerCase().includes(q)) continue;
        out.push({ name: p.name, source: { kind: 'user', path: p.path, folder: g.name }, origin: 'User · ' + folder });
      }
    }
    return out;
  }
  function appendSearchRow(h) {
    const r = document.createElement('div');
    r.className = 'browser-row search-row';
    const name = document.createElement('span');
    name.className = 'browser-row-name';
    name.textContent = h.name;
    const origin = document.createElement('span');
    origin.className = 'browser-row-origin';
    origin.textContent = h.origin;
    r.appendChild(name);
    r.appendChild(origin);
    if (isCurrentSource(h.source)) r.classList.add('current');
    r.addEventListener('click', () => {
      closeMenu();
      if (h.source.kind === 'factory') {
        send.loadFactory(h.source.index);
      } else {
        send.loadUser(h.source.path);
      }
    });
    if (h.source.kind === 'user') {
      r.dataset.path = h.source.path;
      r.addEventListener('contextmenu', (e) => {
        e.preventDefault();
        openMenu(e, { kind: 'preset', path: h.source.path, name: h.name, folder: h.source.folder });
      });
      wirePresetDragSource(r, h.source.path, h.source.folder);
    }
    presetsEl.appendChild(r);
  }
  function wirePresetDragSource(row, path, folder) {
    row.draggable = true;
    row.addEventListener('dragstart', (e) => {
      dragSourcePath = path;
      dragSourceFolder = folder == null ? null : folder;
      try { e.dataTransfer.setData('vxn/preset', path); } catch (_) {}
      e.dataTransfer.effectAllowed = 'move';
      row.classList.add('dragging');
    });
    row.addEventListener('dragend', () => {
      dragSourcePath = null;
      dragSourceFolder = null;
      row.classList.remove('dragging');
      for (const el of foldersEl.querySelectorAll('.drag-over, .drag-blocked')) {
        el.classList.remove('drag-over', 'drag-blocked');
      }
    });
  }
  function followPath(pathStr) {
    if (!pathStr || !bound) return;
    for (const g of corpus.user) {
      for (const p of g.presets) {
        if (p.path !== pathStr) continue;
        selectedFolder = { kind: 'user', name: g.name };
        if (query) {
          query = '';
          if (inputEl) inputEl.value = '';
        }
        renderFolders();
        renderPresets();
        const row = presetsEl.querySelector(`.browser-row[data-path="${cssEscapePath(pathStr)}"]`);
        if (row) {
          try { row.scrollIntoView({ block: 'nearest' }); } catch (_) {}
        }
        return;
      }
    }
  }
  function cssEscapePath(s) {
    if (typeof CSS !== 'undefined' && typeof CSS.escape === 'function') {
      return CSS.escape(s);
    }
    return s.replace(/(["\\])/g, '\\$1');
  }
  function isCurrentSource(src) {
    if (!currentSource || !src) return false;
    if (src.kind === 'factory') {
      return currentSource.kind === 'factory' && currentSource.index === src.index;
    }
    return currentSource.kind === 'user' && currentSource.path === src.path;
  }
  function appendEmpty(text) {
    const e = document.createElement('div');
    e.className = 'browser-empty';
    e.textContent = text;
    presetsEl.appendChild(e);
  }
  function isCurrent(p) {
    if (!currentSource) return false;
    if (selectedFolder.kind === 'factory') {
      return currentSource.kind === 'factory' && currentSource.index === p.index;
    }
    return currentSource.kind === 'user' && currentSource.path === p.path;
  }
  function loadEntry(p) {
    if (selectedFolder.kind === 'factory') {
      send.loadFactory(p.index);
    } else {
      send.loadUser(p.path);
    }
  }
  function setOpen(open) {
    if (!panelEl) return;
    isOpen = !!open;
    panelEl.hidden = !isOpen;
    if (backdropEl) backdropEl.hidden = !isOpen;
    if (isOpen) {
      renderFolders();
      renderPresets();
      try { inputEl.focus(); } catch (_) {}
    } else {
      closeMenu();
      closeModal();
    }
    if (onOpenChange) onOpenChange(isOpen);
  }
  function setCurrentSource(src) {
    currentSource = src || null;
    if (isOpen) renderPresets();
  }
  function getSaveFolder() {
    if (selectedFolder.kind !== 'user') return null;
    return selectedFolder.name;
  }
  // Save (overwrite) needs the folder of the currently-loaded user preset.
  // Returns the folder name (null for root) or `undefined` if the path isn't
  // in the user corpus.
  function folderForUserPath(pathStr) {
    if (!pathStr || !corpus.user) return undefined;
    for (const g of corpus.user) {
      for (const p of g.presets) {
        if (p.path === pathStr) return g.name;
      }
    }
    return undefined;
  }

  // ── Context menu ─────────────────────────────────────────────────────
  function closeMenu() {
    if (menuEl) {
      menuEl.remove();
      menuEl = null;
    }
  }
  function openMenu(ev, target) {
    closeMenu();
    const m = document.createElement('div');
    m.className = 'browser-menu';
    const rect = panelEl.getBoundingClientRect();
    m.style.left = (ev.clientX - rect.left) + 'px';
    m.style.top = (ev.clientY - rect.top) + 'px';

    const renameLabel = target.name;
    appendMenuItem(m, 'Rename', () => {
      closeMenu();
      promptText('Rename', renameLabel, (value) => {
        if (value == null) return;
        const trimmed = value.trim();
        if (!trimmed || trimmed === renameLabel) return;
        if (target.kind === 'preset') {
          send.renamePreset(target.path, trimmed);
        } else {
          send.renameFolder(target.name, trimmed);
        }
      });
    });
    appendMenuItem(m, 'Delete', () => {
      closeMenu();
      openDeleteConfirm(target);
    });
    if (target.kind === 'preset') {
      appendMoveSubmenu(m, target);
    }
    panelEl.appendChild(m);
    menuEl = m;
  }
  function appendMenuItem(parent, label, onClick) {
    const item = document.createElement('div');
    item.className = 'browser-menu-item';
    item.textContent = label;
    item.addEventListener('click', (e) => {
      e.stopPropagation();
      onClick();
    });
    parent.appendChild(item);
    return item;
  }
  function appendMoveSubmenu(parent, target) {
    const item = document.createElement('div');
    item.className = 'browser-menu-item has-submenu';
    item.textContent = 'Move to';
    const sub = document.createElement('div');
    sub.className = 'browser-submenu';
    const targets = moveTargets(target.folder, corpus);
    if (targets.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'browser-submenu-empty';
      empty.textContent = 'no other folders';
      sub.appendChild(empty);
    } else {
      for (const t of targets) {
        const subItem = document.createElement('div');
        subItem.className = 'browser-submenu-item';
        subItem.textContent = t.label;
        subItem.addEventListener('click', (e) => {
          e.stopPropagation();
          closeMenu();
          send.movePreset(target.path, t.name);
        });
        sub.appendChild(subItem);
      }
    }
    item.appendChild(sub);
    parent.appendChild(item);
    return item;
  }

  // ── Modals: delete-confirm + Save As ─────────────────────────────────
  function openDeleteConfirm(target) {
    const isPreset = target.kind === 'preset';
    const kindLabel = isPreset ? 'preset' : 'folder';
    const name = target.name;
    const message = isPreset
      ? `Delete preset “${name}”? This cannot be undone.`
      : `Delete folder “${name}” and every preset inside it? This cannot be undone.`;
    openConfirmModal({
      title: `Delete ${kindLabel}`,
      message,
      confirmLabel: 'Delete',
      danger: true,
      onConfirm: () => {
        if (isPreset) {
          send.deletePreset(target.path);
        } else {
          send.deleteFolder(target.name);
        }
      },
    });
  }
  function closeModal() {
    if (modalEl) {
      modalEl.remove();
      modalEl = null;
    }
  }
  function mountModal({ title, danger, okLabel, onOk }) {
    closeModal();
    closeMenu();
    const wrap = document.createElement('div');
    wrap.className = 'browser-modal-wrap';

    const back = document.createElement('div');
    back.className = 'browser-modal-backdrop';
    back.addEventListener('click', closeModal);
    wrap.appendChild(back);

    const dialog = document.createElement('div');
    dialog.className = 'browser-modal';

    const t = document.createElement('div');
    t.className = 'browser-modal-title';
    t.textContent = title;
    dialog.appendChild(t);

    const body = document.createElement('div');
    body.className = 'browser-modal-body';
    dialog.appendChild(body);

    const actions = document.createElement('div');
    actions.className = 'browser-modal-actions';
    const cancel = document.createElement('button');
    cancel.type = 'button';
    cancel.className = 'browser-modal-btn';
    cancel.textContent = 'Cancel';
    cancel.addEventListener('click', closeModal);
    actions.appendChild(cancel);

    const ok = document.createElement('button');
    ok.type = 'button';
    ok.className = 'browser-modal-btn' + (danger ? ' danger' : '');
    ok.textContent = okLabel || 'OK';
    ok.addEventListener('click', () => {
      if (ok.disabled) return;
      try { onOk(); } catch (e) { console.warn('modal onConfirm threw', e); }
      closeModal();
    });
    actions.appendChild(ok);

    dialog.appendChild(actions);
    wrap.appendChild(dialog);

    faceplateRoot().appendChild(wrap);
    modalEl = wrap;
    return { body, ok };
  }
  function openConfirmModal({ title, message, confirmLabel, danger, onConfirm }) {
    const { body, ok } = mountModal({ title, danger, okLabel: confirmLabel, onOk: onConfirm });
    body.textContent = message;
    try { ok.focus(); } catch (_) {}
  }
  function openSaveAsModal(initialName) {
    let name = (initialName || '').trim();
    const initialFolder = selectedFolder.kind === 'user' ? selectedFolder.name : null;
    let folder = initialFolder;

    const valid = () => name.length > 0;

    const { body, ok } = mountModal({
      title: 'Save preset as',
      okLabel: 'Save',
      onOk: () => {
        if (!valid()) return;
        send.savePreset(name, folder);
      },
    });
    body.classList.add('save-as-body');

    function gateOk() {
      const on = valid();
      ok.disabled = !on;
      ok.classList.toggle('disabled', !on);
    }

    const nameRow = document.createElement('div');
    nameRow.className = 'save-as-row';
    const nameLab = document.createElement('div');
    nameLab.className = 'save-as-label';
    nameLab.textContent = 'Name';
    nameRow.appendChild(nameLab);
    const nameLabel = document.createElement('div');
    nameLabel.className = 'save-as-name';
    nameLabel.textContent = name || '(untitled)';
    if (!name) nameLabel.classList.add('placeholder');
    nameRow.appendChild(nameLabel);
    const editBtn = document.createElement('button');
    editBtn.type = 'button';
    editBtn.className = 'browser-modal-btn';
    editBtn.textContent = name ? 'Edit' : 'Name…';
    editBtn.addEventListener('click', () => {
      promptText('Preset name', name, (value) => {
        if (value == null) return;
        const trimmed = value.trim();
        if (!trimmed) return;
        name = trimmed;
        nameLabel.textContent = name;
        nameLabel.classList.remove('placeholder');
        editBtn.textContent = 'Edit';
        gateOk();
      });
    });
    nameRow.appendChild(editBtn);
    body.appendChild(nameRow);

    const folderRow = document.createElement('div');
    folderRow.className = 'save-as-row';
    const folderLab = document.createElement('div');
    folderLab.className = 'save-as-label';
    folderLab.textContent = 'Folder';
    folderRow.appendChild(folderLab);
    const select = document.createElement('select');
    select.className = 'save-as-select';
    for (const opt of folderOptions(corpus)) {
      const o = document.createElement('option');
      o.value = opt.value;
      o.textContent = opt.label;
      if (opt.value === folderValue(folder)) o.selected = true;
      select.appendChild(o);
    }
    select.addEventListener('change', () => {
      folder = select.value === '__root__' ? null : select.value;
    });
    folderRow.appendChild(select);
    body.appendChild(folderRow);

    gateOk();
    try { ok.focus(); } catch (_) {}
  }

  return {
    bind,
    setCorpus,
    setCurrentSource,
    setOpen,
    isOpen: () => isOpen,
    getSaveFolder,
    folderForUserPath,
    openSaveAs: openSaveAsModal,
    // 0020: single-slot callback — last caller wins, a second subscriber
    // silently replaces the first. The preset bar (its Browse-button .active
    // mirror) is the sole subscriber in both synths; promote to a listener list
    // if a second consumer ever appears.
    onOpenChange: (cb) => { onOpenChange = cb; },
    followPath,
  };
}
