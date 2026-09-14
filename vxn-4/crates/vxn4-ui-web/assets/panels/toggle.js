// Toggle row and button group — the discrete controls.
//
// Both are the same box-and-label row; a button group is a strip of them with
// exactly one lit. vxn-1b's `panels/discrete.js` is the same idiom bound to
// markup instead of building it, so this is the shape rather than the code.

/**
 * One on/off row. `api.set` is deliberately SILENT — it paints without firing
 * `onChange`, for the case where something else already owns the state change
 * (the PM grid toggling a route the route table is also showing). A setter
 * that re-announced would put the two views in a loop.
 */
export function toggle(label, on, onChange) {
  const row = document.createElement("div");
  row.className = "ctl-tg-row" + (on ? " on" : "");
  row.innerHTML = '<span class="ctl-tg-box"></span><span class="ctl-tg-lbl"></span>';
  row.querySelector(".ctl-tg-lbl").textContent = label;
  row.addEventListener("click", () => {
    row.classList.toggle("on");
    if (onChange) onChange(row.classList.contains("on"));
  });
  row.api = {
    get: () => row.classList.contains("on"),
    set: (next) => row.classList.toggle("on", next),
  };
  return row;
}

/**
 * A labelled strip of rows, one lit — an enum small enough that every option
 * is worth showing at once (filter mode, EG curve, oversampling quality).
 * `flow: "row"` lays the options across instead of down.
 */
export function buttonGroup(label, options, idx0, flow, onChange) {
  const cell = document.createElement("div");
  cell.className = "ctl ctl-buttongroup";
  if (flow) cell.dataset.flow = flow;
  const lbl = document.createElement("div");
  lbl.className = "ctl-label";
  lbl.textContent = label || "";
  const rows = document.createElement("div");
  rows.className = "tg-rows";
  cell.append(lbl, rows);

  let idx = idx0 || 0;
  options.forEach((opt, i) => {
    const row = document.createElement("div");
    row.className = "ctl-tg-row";
    row.innerHTML = '<span class="ctl-tg-box"></span><span class="ctl-tg-lbl"></span>';
    row.querySelector(".ctl-tg-lbl").textContent = opt;
    row.addEventListener("click", () => { idx = i; draw(); if (onChange) onChange(i); });
    rows.append(row);
  });
  function draw() {
    [...rows.children].forEach((r, i) => r.classList.toggle("on", i === idx));
  }
  draw();
  cell.api = { get: () => idx, set: (i) => { idx = i; draw(); } };
  return cell;
}
