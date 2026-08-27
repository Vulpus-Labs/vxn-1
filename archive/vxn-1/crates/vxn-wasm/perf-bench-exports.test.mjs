// Headless export/smoke test for the 0087 perf bench (epic E020).
//
//   cargo build -p vxn-wasm --target wasm32-unknown-unknown --release
//   node perf-bench-exports.test.mjs
//
// Proves the bench C ABI is present and behaves, WITHOUT timing (wasm32 has no
// std::time — the real per-quantum measurement runs in the browser worklet via
// perf-harness.mjs / perf.html). Same node-harness discipline as harness-0038.mjs.
//
// Asserts:
//   1. All five bench exports exist (vxn_bench_new/_destroy/_render/_out_l/_simd128).
//   2. vxn_bench_simd128() is 0 or 1 (the build label the browser harness reads).
//   3. The worst-case patch renders audible output after a few quanta.
//   4. render(n) advances the synth (output changes between batches).

import { readFileSync } from "node:fs";

const WASM = new URL(
  "../../../target/wasm32-unknown-unknown/release/vxn_wasm.wasm",
  import.meta.url,
);
const SR = 48000;

const { instance } = await WebAssembly.instantiate(readFileSync(WASM), {});
const x = instance.exports;

let failures = 0;
const check = (cond, msg) => {
  console.log(`  ${cond ? "PASS" : "FAIL"}  ${msg}`);
  if (!cond) failures++;
};

const peak = (buf) => buf.reduce((m, s) => Math.max(m, Math.abs(s)), 0);
const readOut = (b) => new Float32Array(x.memory.buffer, x.vxn_bench_out_l(b), 128);

console.log("\n=== 1. bench C-ABI exports present ===");
for (const name of [
  "vxn_bench_new",
  "vxn_bench_destroy",
  "vxn_bench_render",
  "vxn_bench_out_l",
  "vxn_bench_simd128",
]) {
  check(typeof x[name] === "function", `export ${name}`);
}

console.log("\n=== 2. simd128 build label is 0 or 1 ===");
const simd = x.vxn_bench_simd128();
check(simd === 0 || simd === 1, `vxn_bench_simd128() = ${simd} (${simd ? "SIMD128" : "scalar"} build)`);

console.log("\n=== 3. worst-case patch is audible ===");
{
  const b = x.vxn_bench_new(SR);
  x.vxn_bench_render(b, 8); // let the attack open
  const p = peak(readOut(b));
  check(p > 0, `peak after 8 quanta = ${p.toFixed(5)} (> 0)`);
  x.vxn_bench_destroy(b);
}

console.log("\n=== 4. render(n) advances per quantum ===");
{
  const b = x.vxn_bench_new(SR);
  x.vxn_bench_render(b, 4);
  const first = Array.from(readOut(b));
  x.vxn_bench_render(b, 4);
  const second = Array.from(readOut(b));
  const changed = first.some((v, i) => v !== second[i]);
  check(changed, "output differs between two render batches (loop advanced)");
  x.vxn_bench_destroy(b);
}

console.log(`\n${failures === 0 ? "ALL CHECKS PASSED ✓" : `${failures} CHECK(S) FAILED ✗`}`);
process.exit(failures === 0 ? 0 : 1);
