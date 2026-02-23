import test from "node:test";
import assert from "node:assert/strict";
import { buildRuleTestScaffold } from "../src/language-tools/test-scaffold";
import {
  getFramework,
  frameworkIds,
} from "../src/language-tools/test-scaffold-frameworks";

const SPEC_TEXT =
  `rule Close {\n` +
  `  when: CloseTicket(ticket)\n` +
  `  requires: ticket.status = open\n` +
  `  ensures: ticket.status = closed\n` +
  `}\n`;

test("builds scaffold tests from rule declarations (node:test)", () => {
  const framework = getFramework("node:test")!;
  const output = buildRuleTestScaffold(SPEC_TEXT, "ticketing", framework);
  assert.match(output, /import test from "node:test"/);
  assert.match(output, /import assert from "node:assert\/strict"/);
  assert.match(output, /test\("ticketing \/ Close"/);
  assert.match(output, /trigger: CloseTicket\(ticket\)/);
  assert.match(output, /requires: ticket.status = open/);
  assert.match(output, /ensures: ticket.status = closed/);
});

test("pytest scaffold uses def test_ and python comments", () => {
  const framework = getFramework("pytest")!;
  const output = buildRuleTestScaffold(SPEC_TEXT, "ticketing", framework);
  assert.match(output, /def test_ticketing_close\(\):/);
  assert.match(output, /# trigger: CloseTicket\(ticket\)/);
  assert.match(output, /assert True/);
});

test("clojure.test scaffold uses deftest and ;; comments", () => {
  const framework = getFramework("clojure.test")!;
  const output = buildRuleTestScaffold(SPEC_TEXT, "ticketing", framework);
  assert.match(output, /\(deftest ticketing-close-test/);
  assert.match(output, /;; trigger: CloseTicket\(ticket\)/);
  assert.match(output, /\(is \(= 1 1\)\)/);
});

test("junit5 scaffold uses @Test annotation and snake_case method", () => {
  const framework = getFramework("junit5")!;
  const output = buildRuleTestScaffold(SPEC_TEXT, "ticketing", framework);
  assert.match(output, /import org\.junit\.jupiter\.api\.Test/);
  assert.match(output, /@Test\n/);
  assert.match(output, /void ticketing_close\(\)/);
  assert.match(output, /\/\/ trigger: CloseTicket\(ticket\)/);
});

test("all six framework IDs resolve", () => {
  const ids = frameworkIds();
  assert.deepStrictEqual(ids, [
    "node:test",
    "jest",
    "vitest",
    "pytest",
    "junit5",
    "clojure.test",
  ]);
  for (const id of ids) {
    assert.ok(getFramework(id), `framework "${id}" should resolve`);
  }
});
