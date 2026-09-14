// Dropdown — the matrix source / destination / curve pickers, and the
// operator waveform list.
//
// A native `<select>`, styled. The lists it carries are long (a destination
// can name any parameter of any of eight operators) and are chosen rarely,
// which is exactly where the platform's own list beats anything drawn here:
// it scrolls, it type-selects, and it opens outside the window on every OS.

/** Flat option list. `idx0` is the selected index. */
export function combo(options, idx0, onChange) {
  const sel = document.createElement("select");
  sel.className = "combo";
  options.forEach((o, i) => {
    const opt = document.createElement("option");
    opt.value = i; opt.textContent = o;
    sel.append(opt);
  });
  sel.value = String(idx0 || 0);
  // "—" is the empty selection, greyed so an unassigned slot reads as blank
  // rather than as a choice someone made.
  const sync = () => sel.classList.toggle("empty", sel.value === "0" && options[0] === "—");
  sel.addEventListener("change", () => { sync(); if (onChange) onChange(+sel.value); });
  sync();
  return sel;
}

/**
 * Same control, grouped options. Indices are flat across all groups so a slot
 * stores one number, exactly as a destination id does — the grouping is a
 * presentation of the id space, not a second dimension of it.
 */
export function comboGrouped(groups, idx0, onChange) {
  const sel = document.createElement("select");
  sel.className = "combo";
  let flat = 0;
  groups.forEach((g) => {
    const parent = g.label
      ? Object.assign(document.createElement("optgroup"), { label: g.label })
      : sel;
    g.items.forEach((it) => {
      const opt = document.createElement("option");
      opt.value = flat++; opt.textContent = it;
      parent.append(opt);
    });
    if (g.label) sel.append(parent);
  });
  sel.value = String(idx0 || 0);
  const sync = () => sel.classList.toggle("empty", sel.value === "0");
  sel.addEventListener("change", () => { sync(); if (onChange) onChange(+sel.value); });
  sync();
  return sel;
}
