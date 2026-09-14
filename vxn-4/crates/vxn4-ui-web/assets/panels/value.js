// Value mapping — the normalised position a control holds against the plain
// value it means.
//
// Every control on this faceplate owns a `t` in 0..1 and a `spec` that turns
// it into something a player can read. That indirection is deliberate: a drag
// is a distance in pixels, so the control's own state has to be the thing
// pixels map onto linearly, and the taper belongs to the value rather than to
// the gesture.
//
// `spec` fields: min, max, unit, dp, exp (log taper), step (quantise), signed
// (print a leading +), bipolar (fill outward from centre), fmt (own formatter).
//
// This is deliberately NOT vxn-2's `ParamDesc` taper math (`panels/fader.js`
// there). That mirrors the engine's `taper_to/from_norm_exp` including its
// three-point `mid`, and vxn-4's descriptor table (0381) is not built yet.
// When it is, this module is what 0388 replaces — one file, one seam.
//
// ES module so the vitest suite can import the pure helpers; the `export`
// markers are stripped at splice time (`strip_esm_exports`), which is why
// every module here must be free of module-scope name collisions.

export function mapValue(spec, t) {
  const min = spec.min ?? 0, max = spec.max ?? 1;
  if (spec.exp) {
    // A log taper cannot start at zero, so the floor is clamped rather than
    // the spec being rejected — plenty of specs want `min: 0` and mean
    // "as near silence as the fader can get".
    const lo = Math.max(min, 1e-4);
    return lo * Math.pow(max / lo, t);
  }
  const v = min + (max - min) * t;
  return spec.step ? Math.round(v / spec.step) * spec.step : v;
}

// Inverse of `mapValue` — the EG and key-scaling graphs drag in real units and
// have to put the result back into a fader's normalised position.
export function invValue(spec, v) {
  const min = spec.min ?? 0, max = spec.max ?? 1;
  if (spec.exp) {
    const lo = Math.max(min, 1e-4);
    return Math.log(Math.max(v, lo) / lo) / Math.log(max / lo);
  }
  return (v - min) / (max - min);
}

export function formatValue(spec, t) {
  if (spec.fmt) return spec.fmt(mapValue(spec, t), t);
  const v = mapValue(spec, t);
  const dp = spec.dp ?? (Math.abs(v) < 10 ? 2 : 0);
  const s = v.toFixed(dp);
  const signed = spec.signed && v > 0 ? "+" + s : s;
  return signed + (spec.unit || "");
}

export const fmtHz = (v) => (v >= 1000 ? (v / 1000).toFixed(2) + " kHz" : v.toFixed(v < 10 ? 2 : 0) + " Hz");
export const fmtSec = (v) => (v >= 1 ? v.toFixed(2) + " s" : (v * 1000).toFixed(0) + " ms");
export const fmtPct = (v) => Math.round(v) + "%";
