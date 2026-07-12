// Input-adapter check runner (ticket 0160).
//
// `keyboard-input` / `midi-input` were ported verbatim from vxn-1, where their
// tests are self-running scripts (a `check()` harness that `process.exit`s with
// the failure count) rather than node:test cases — so they can't be globbed into
// `node --test web/*.test.mjs` directly (the exit would abort the runner). This
// wrapper runs each as a subprocess and asserts a clean exit, folding them into
// the one suite command.

import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

function runCheck(name) {
  const path = fileURLToPath(new URL(name, import.meta.url));
  // Throws (non-zero exit) if any check failed; stdout carries the PASS/FAIL log.
  execFileSync(process.execPath, [path], { stdio: "pipe" });
}

test("keyboard-input.check.mjs passes", () => {
  assert.doesNotThrow(() => runCheck("./keyboard-input.check.mjs"));
});

test("midi-input.check.mjs passes", () => {
  assert.doesNotThrow(() => runCheck("./midi-input.check.mjs"));
});
