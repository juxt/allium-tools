import test from "node:test";
import assert from "node:assert/strict";
import { planInsertTemporalGuard } from "../src/language-tools/insert-temporal-guard-refactor";

test("returns null when selected line is not when clause", () => {
  const text = `rule A {\n  ensures: x = y\n}\n`;
  const start = text.indexOf("ensures");
  const plan = planInsertTemporalGuard(text, start);
  assert.equal(plan, null);
});

test("returns null for non-temporal when clause", () => {
  const text = `rule A {\n  when: Ping()\n  ensures: Done()\n}\n`;
  const start = text.indexOf("when:");
  const plan = planInsertTemporalGuard(text, start);
  assert.equal(plan, null);
});

test("returns null when rule already has requires", () => {
  const text = `rule A {\n  when: item: Thing.expires_at <= now\n  requires: item.active = true\n  ensures: item.active = false\n}\n`;
  const start = text.indexOf("when:");
  const plan = planInsertTemporalGuard(text, start);
  assert.equal(plan, null);
});

test("proposes guard insertion after temporal when clause", () => {
  const text = `rule A {\n  when: item: Thing.expires_at <= now\n  ensures: item.active = false\n}\n`;
  const start = text.indexOf("when:");
  const plan = planInsertTemporalGuard(text, start);
  assert.ok(plan);
  assert.equal(plan.title, "Add temporal requires guard");
  assert.equal(plan.edit.text, "  requires: TODO() -- add temporal guard\n");
});
