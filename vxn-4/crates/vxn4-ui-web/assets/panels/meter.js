// Level meters.
//
// Stereo throughout: a mono bar cannot show a patch that has collapsed to one
// side, which on an instrument with per-operator pan is a thing you need to
// see. Two bars share one frame so the pair reads as one meter.
//
// Gain reduction is the exception and draws as a single bar reading downward
// from 0 dB. The compressor's detector is linked across the pair — both
// channels are pulled down by the same amount, by design, or a hard pan would
// wander mid-transient — so a two-bar GR meter would be two copies of one
// number.
//
// `tick(l, r)` takes normalised heights, not dB: the mapping from the engine's
// levels to bar heights belongs to whoever reads the engine, which is 0388.
// Until then nothing calls it and the bars sit at zero, which is honest — no
// audio is being reported because nothing is reporting any.

export function meter(label, kind) {
  const gr = kind === "gr";
  const chan = '<div class="meter-chan"><div class="meter-fill"></div><div class="meter-peak"></div></div>';
  const col = document.createElement("div");
  col.className = "meter-col";
  col.innerHTML =
    `<div class="ctl-label"></div>` +
    `<div class="meter-bar${gr ? " meter-gr" : ""}">${gr ? chan : chan + chan}</div>`;
  col.querySelector(".ctl-label").textContent = label;
  const fills = [...col.querySelectorAll(".meter-fill")];
  const peaks = [...col.querySelectorAll(".meter-peak")];
  const pk = [0, 0];
  col.tick = (l, r) => {
    fills.forEach((fill, i) => {
      const v = i === 0 ? l : r;
      fill.style.height = (v * 100) + "%";
      // Peak hold with a slow bleed rather than a timer: the marker falls a
      // fixed fraction per frame, so it stays legible on a transient without
      // needing to know the frame rate.
      pk[i] = Math.max(pk[i] * 0.97, v);
      peaks[i].style.bottom = (pk[i] * 100) + "%";
    });
  };
  return col;
}
