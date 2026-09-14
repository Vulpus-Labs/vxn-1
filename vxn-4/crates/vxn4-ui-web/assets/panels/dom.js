// The one DOM helper worth naming.
//
// It exists in its own module rather than in whichever panel needed it first
// because the splice concatenates every module into ONE script scope: two
// modules both declaring `cellDiv` would be a `SyntaxError` on the whole page,
// not a shadowed local. Anything more than one panel wants goes here.

/** A `<div>` with a class and text — a grid cell, a header, a label. */
export function cellDiv(cls, text) {
  const d = document.createElement("div");
  d.className = cls;
  d.textContent = text;
  return d;
}
