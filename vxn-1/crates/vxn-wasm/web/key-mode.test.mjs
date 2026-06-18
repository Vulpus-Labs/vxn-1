// Headless Node test for the key-mode / split-point control path (ticket 0056).
//
//   node web/key-mode.test.mjs
//
// Key mode / split point are non-automatable shared state on the worklet port
// (NOT the ring, NOT the store). We use a fake host that records setKeyMode /
// setSplitPoint calls (the WebHost methods that own the port hop) and assert the
// routing. Covers the 0056 acceptance:
//
//   1. setKeyMode by enum and by name; junk -> no-op.
//   2. setSplitPoint clamps to 0..127.
//   3. attachKeyMode pushes initial state and tracks current mode/split.

import { KeyMode, setKeyMode, setSplitPoint, attachKeyMode } from "./key-mode.mjs";

let failures = 0;
const check = (cond, msg) => {
  console.log(`  ${cond ? "PASS" : "FAIL"}  ${msg}`);
  if (!cond) failures++;
};

// Fake host: records the non-automatable control calls the WebHost would post
// over the worklet port.
function ctlHost() {
  return {
    modes: [],
    splits: [],
    setKeyMode(m) {
      this.modes.push(m);
    },
    setSplitPoint(n) {
      this.splits.push(n);
    },
  };
}

console.log("\n=== 1. setKeyMode by enum and name ===");
{
  const host = ctlHost();
  check(setKeyMode(host, KeyMode.WHOLE) === 0, "WHOLE enum -> 0");
  check(setKeyMode(host, KeyMode.DUAL) === 1, "DUAL enum -> 1");
  check(setKeyMode(host, KeyMode.SPLIT) === 2, "SPLIT enum -> 2");
  check(setKeyMode(host, "split") === 2, "name 'split' -> 2");
  check(setKeyMode(host, "Dual") === 1, "name 'Dual' (case-insensitive) -> 1");
  check(host.modes.join(",") === "0,1,2,2,1", `host saw the modes in order (${host.modes.join(",")})`);
  // Junk -> no-op, no port write.
  check(setKeyMode(host, 99) === null, "out-of-range mode -> null (no-op)");
  check(setKeyMode(host, "bogus") === null, "unknown name -> null (no-op)");
  check(host.modes.length === 5, "junk modes did not reach the host");
}

console.log("\n=== 2. setSplitPoint clamps ===");
{
  const host = ctlHost();
  check(setSplitPoint(host, 60) === 60, "split 60 passes through");
  check(setSplitPoint(host, -5) === 0, "negative split clamps to 0");
  check(setSplitPoint(host, 200) === 127, "split > 127 clamps to 127");
  check(host.splits.join(",") === "60,0,127", `host saw clamped splits (${host.splits.join(",")})`);
}

console.log("\n=== 3. attachKeyMode initial push + tracking ===");
{
  const host = ctlHost();
  const kc = attachKeyMode(host, { mode: KeyMode.SPLIT, splitPoint: 64 });
  // Initial state pushed on attach.
  check(host.modes[0] === KeyMode.SPLIT, "initial mode pushed on attach");
  check(host.splits[0] === 64, "initial split pushed on attach");
  check(kc.getMode() === KeyMode.SPLIT && kc.getModeLabel() === "Split", "controller reflects initial mode");
  check(kc.getSplitPoint() === 64, "controller reflects initial split");

  kc.setMode("dual");
  check(kc.getMode() === KeyMode.DUAL && host.modes[host.modes.length - 1] === 1, "setMode by name routed + tracked");
  kc.setSplitPoint(72);
  check(kc.getSplitPoint() === 72 && host.splits[host.splits.length - 1] === 72, "setSplitPoint routed + tracked");

  // Bad mode leaves the tracked mode unchanged.
  const before = kc.getMode();
  kc.setMode("nonsense");
  check(kc.getMode() === before, "bad setMode leaves tracked mode unchanged");
}

console.log(`\n${failures === 0 ? "ALL CHECKS PASSED" : `${failures} CHECK(S) FAILED`}`);
process.exit(failures === 0 ? 0 : 1);
