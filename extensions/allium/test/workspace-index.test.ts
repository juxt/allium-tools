import test from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import {
  buildWorkspaceIndex,
  resolveImportedDefinition,
} from "../src/language-tools/workspace-index";

test("resolves imported definitions from aliased use path", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "allium-index-"));
  const sharedPath = path.join(root, "shared.allium");
  const mainPath = path.join(root, "main.allium");

  fs.writeFileSync(
    sharedPath,
    `entity SharedInvite {\n  status: String\n}\n`,
    "utf8",
  );
  fs.writeFileSync(
    mainPath,
    `use "./shared.allium" as shared\nrule A {\n  when: shared/SharedInvite.created_at <= now\n  ensures: Done()\n}\n`,
    "utf8",
  );

  const mainText = fs.readFileSync(mainPath, "utf8");
  const offset =
    mainText.indexOf("shared/SharedInvite") + "shared/Shared".length;
  const index = buildWorkspaceIndex(root);
  const matches = resolveImportedDefinition(mainPath, mainText, offset, index);

  assert.equal(matches.length, 1);
  assert.equal(matches[0].filePath, sharedPath);
  assert.equal(matches[0].definition.name, "SharedInvite");
});

test("resolves imported definitions from dotted aliased use path", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "allium-index-"));
  const sharedPath = path.join(root, "shared.allium");
  const mainPath = path.join(root, "main.allium");

  fs.writeFileSync(
    sharedPath,
    `entity SharedInvite {\n  status: String\n}\n`,
    "utf8",
  );
  fs.writeFileSync(
    mainPath,
    `use "./shared.allium" as shared\nrule A {\n  when: shared.SharedInvite.created_at <= now\n  ensures: Done()\n}\n`,
    "utf8",
  );

  const mainText = fs.readFileSync(mainPath, "utf8");
  const offset =
    mainText.indexOf("shared.SharedInvite") + "shared.Shar".length;
  const index = buildWorkspaceIndex(root);
  const matches = resolveImportedDefinition(mainPath, mainText, offset, index);

  assert.equal(matches.length, 1);
  assert.equal(matches[0].filePath, sharedPath);
  assert.equal(matches[0].definition.name, "SharedInvite");
});

test("resolves use paths that omit .allium extension", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "allium-index-"));
  const sharedPath = path.join(root, "shared.allium");
  const mainPath = path.join(root, "main.allium");

  fs.writeFileSync(
    sharedPath,
    `rule Ping {\n  when: Tick()\n  ensures: Done()\n}\n`,
    "utf8",
  );
  fs.writeFileSync(
    mainPath,
    `use "./shared" as shared\nrule A {\n  when: shared/Ping()\n  ensures: Done()\n}\n`,
    "utf8",
  );

  const mainText = fs.readFileSync(mainPath, "utf8");
  const offset = mainText.indexOf("shared/Ping") + "shared/Pi".length;
  const index = buildWorkspaceIndex(root);
  const matches = resolveImportedDefinition(mainPath, mainText, offset, index);

  assert.equal(matches.length, 1);
  assert.equal(matches[0].filePath, sharedPath);
  assert.equal(matches[0].definition.name, "Ping");
});
