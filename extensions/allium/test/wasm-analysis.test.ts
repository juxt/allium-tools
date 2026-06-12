import test from "node:test";
import assert from "node:assert/strict";
import { analyzeAlliumWithRust } from "../src/language-tools/wasm-ast";

test("WASM analysis exposes Rust undefined binding diagnostics", () => {
  const diagnostics = analyzeAlliumWithRust(
    `rule Notify {\n  when: Ping()\n  requires: user.status = active\n  ensures: Done()\n}\n`,
  );
  assert.ok(
    diagnostics.some(
      (diagnostic) => diagnostic.code === "allium.rule.undefinedBinding",
    ),
  );
});

test("WASM analysis accepts typed trigger parameter bindings", () => {
  const diagnostics = analyzeAlliumWithRust(
    `entity User {\n  status: String\n}\n\nrule Notify {\n  when: UserUpdated(user: User)\n  requires: user.status = active\n  ensures: Done()\n}\n`,
  );
  assert.equal(
    diagnostics.some(
      (diagnostic) => diagnostic.code === "allium.rule.undefinedBinding",
    ),
    false,
  );
});
