import test from "node:test";
import assert from "node:assert/strict";
import { planSafeFixesByCategory } from "../src/language-tools/fix-all";

test("plans missing ensures scaffold edits", () => {
  const text = `rule A {\n  when: Ping()\n}\n`;
  const edits = planSafeFixesByCategory(text, "strict", "missingEnsures");
  assert.equal(edits.length, 1);
  assert.match(edits[0].text, /ensures: TODO\(\)/);
});

test("plans temporal guard edits", () => {
  const text = `entity Invitation {\n  expires_at: Timestamp\n  status: String\n}\n\nrule Expire {\n  when: invitation: Invitation.expires_at <= now\n  ensures: invitation.status = expired\n}\n`;
  const edits = planSafeFixesByCategory(text, "strict", "temporalGuards");
  assert.equal(edits.length, 1);
  assert.match(edits[0].text, /requires: TODO\(\) -- add temporal guard/);
});
