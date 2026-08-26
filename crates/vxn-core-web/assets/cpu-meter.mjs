// CPU (render-load) meter badge — SHARED across the browser ports (0309).
//
// A small fixed badge: a bar + percent showing the audio thread's per-quantum
// DSP load, where 1.0 is the whole deadline. Fed by the worklet's `cpu` port
// messages via `WebHost.onCpu`.
//
// Pure presentation — it knows nothing about any synth, which is why it is
// shared rather than copied. It was vxn-1's, then vxn-2's verbatim; hoisted here
// when VXN1b needed one (0284's rule: one copy, parameterised).
//
// Self-contained inline styles, no external CSS, and idempotent: a re-boot
// reuses the existing element. No-ops without a document body, so headless tests
// are unaffected.

// ---- CPU (render-load) meter -----------------------------------------------
//
// A small fixed badge at the bottom-left: a bar + percent showing the audio
// thread's per-quantum DSP load (1.0 == the whole deadline). Fed by the worklet's
// `cpu` port messages via WebHost.onCpu. Self-contained (inline styles, no
// external CSS) and created once per boot; idempotent if the element already
// exists (a re-boot reuses it). Ported verbatim from vxn-1.
export function createCpuMeter(doc = globalThis.document) {
  if (!doc || !doc.body) return { update() {}, el: null };
  const ID = "vxn-cpu-meter";
  let el = doc.getElementById(ID);
  if (!el) {
    el = doc.createElement("div");
    el.id = ID;
    el.style.cssText =
      // bottom offset clears the 92px on-screen piano bar (which is fixed to the
      // page bottom); harmless extra lift when the piano is disabled.
      "position:fixed;left:10px;bottom:102px;z-index:9999;display:flex;" +
      "align-items:center;gap:6px;font:11px/1 system-ui,sans-serif;" +
      "color:#cfd3d8;background:rgba(20,22,26,.78);padding:4px 7px;" +
      "border-radius:5px;user-select:none;pointer-events:none;";
    const label = doc.createElement("span");
    label.textContent = "CPU";
    label.style.cssText = "opacity:.7;letter-spacing:.04em;";
    const track = doc.createElement("span");
    track.style.cssText =
      "position:relative;width:54px;height:6px;border-radius:3px;" +
      "background:rgba(255,255,255,.14);overflow:hidden;";
    const fill = doc.createElement("span");
    fill.style.cssText =
      "position:absolute;left:0;top:0;bottom:0;width:0%;background:#46c46e;" +
      "transition:width .1s linear,background .2s linear;";
    track.appendChild(fill);
    const pct = doc.createElement("span");
    pct.textContent = "—";
    pct.style.cssText = "min-width:28px;text-align:right;font-variant-numeric:tabular-nums;";
    el.append(label, track, pct);
    doc.body.appendChild(el);
    el._fill = fill;
    el._pct = pct;
  }
  const update = (load, peak) => {
    const f = el._fill, p = el._pct;
    // null load == "no measurement" (meter disabled — e.g. Safari). Show n/a so a
    // missing reading is distinct from a real 0% (and from the initial "—").
    if (load == null) {
      f.style.width = "0%";
      p.textContent = "n/a";
      return;
    }
    // Bar tracks peak (the worklet posts mean + per-window peak; peak shows
    // transient spikes the mean smooths away).
    const pk = Math.max(0, Math.min(1.5, Math.max(load, peak || 0)));
    f.style.width = `${Math.min(100, pk * 100).toFixed(0)}%`;
    // green < 0.7, amber < 0.9, red beyond — the usual xrun-headroom bands.
    f.style.background = pk < 0.7 ? "#46c46e" : pk < 0.9 ? "#e0b341" : "#e0564b";
    // One decimal under 10% so a live-but-light load stays legible despite the
    // 1% quantization floor.
    const a = Math.max(0, load) * 100;
    p.textContent = a < 10 ? `${a.toFixed(1)}%` : `${Math.round(a)}%`;
  };
  return { update, el };
}
