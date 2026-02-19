import test from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { buildDefinitionLookup } from "../src/language-tools/definitions";
import { planWorkspaceImportedRename } from "../src/language-tools/cross-file-rename";
import { buildWorkspaceIndex } from "../src/language-tools/workspace-index";

function writeFile(root: string, rel: string, text: string): string {
  const full = path.join(root, rel);
  fs.mkdirSync(path.dirname(full), { recursive: true });
  fs.writeFileSync(full, text, "utf8");
  return full;
}

test("plans edits for definition and imported usages across workspace", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "allium-rename-"));
  const shared = writeFile(
    root,
    "shared.allium",
    `rule Ping {\n  when: Trigger()\n  ensures: Done()\n}\n\nrule UsesPing {\n  when: Ping()\n  ensures: Done()\n}\n`,
  );
  writeFile(
    root,
    "consumer.allium",
    `use "./shared.allium" as shared\nrule A {\n  when: shared/Ping()\n  ensures: Done()\n}\n`,
  );

  const index = buildWorkspaceIndex(root);
  const definition = buildDefinitionLookup(
    fs.readFileSync(shared, "utf8"),
  ).symbols.find((item) => item.name === "Ping" && item.kind === "rule");
  assert.ok(definition);

  const result = planWorkspaceImportedRename(
    index,
    shared,
    definition!,
    "PingRenamed",
  );
  assert.equal(result.error, undefined);
  assert.equal(result.edits.length >= 2, true);
});

test("rejects rename when target file already defines new name", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "allium-rename-"));
  const shared = writeFile(
    root,
    "shared.allium",
    `rule Ping {\n  when: Trigger()\n  ensures: Done()\n}\n\nrule PingRenamed {\n  when: Trigger()\n  ensures: Done()\n}\n`,
  );

  const index = buildWorkspaceIndex(root);
  const definition = buildDefinitionLookup(
    fs.readFileSync(shared, "utf8"),
  ).symbols.find((item) => item.name === "Ping" && item.kind === "rule");
  assert.ok(definition);

  const result = planWorkspaceImportedRename(
    index,
    shared,
    definition!,
    "PingRenamed",
  );
  assert.match(result.error ?? "", /collide/);
});

test("plans edits for dotted imported usages across workspace", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "allium-rename-"));
  const shared = writeFile(
    root,
    "shared.allium",
    `rule Ping {\n  when: Trigger()\n  ensures: Done()\n}\n`,
  );
  writeFile(
    root,
    "consumer.allium",
    `use "./shared.allium" as shared\nrule A {\n  when: shared.Ping()\n  ensures: Done()\n}\n`,
  );

  const index = buildWorkspaceIndex(root);
  const definition = buildDefinitionLookup(
    fs.readFileSync(shared, "utf8"),
  ).symbols.find((item) => item.name === "Ping" && item.kind === "rule");
  assert.ok(definition);

  const result = planWorkspaceImportedRename(
    index,
    shared,
    definition!,
    "PingRenamed",
  );
  assert.equal(result.error, undefined);
  assert.equal(result.edits.length >= 2, true);
});
