"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { pathToFileURL } = require("node:url");
const { LspHarness } = require("./lsp-harness");

function offsetToPosition(text, offset) {
  const before = text.slice(0, offset);
  const lines = before.split("\n");
  return {
    line: lines.length - 1,
    character: lines[lines.length - 1].length,
  };
}

function positionAt(text, marker, withinMarker = 0) {
  const index = text.indexOf(marker);
  if (index < 0) {
    throw new Error(`Marker not found: ${marker}`);
  }
  return offsetToPosition(text, index + withinMarker);
}

function uriFor(filePath) {
  return pathToFileURL(filePath).toString();
}

function writeFile(root, relPath, text) {
  const fullPath = path.join(root, relPath);
  fs.mkdirSync(path.dirname(fullPath), { recursive: true });
  fs.writeFileSync(fullPath, text, "utf8");
  return fullPath;
}

async function createHarness(root) {
  const harness = new LspHarness(path.resolve(__dirname, "../dist/bin.js"), {
    cwd: path.resolve(__dirname, ".."),
  });
  await harness.initialize({
    processId: process.pid,
    rootUri: uriFor(root),
    capabilities: {},
    workspaceFolders: [{ uri: uriFor(root), name: path.basename(root) }],
  });
  return harness;
}

async function openAlliumDoc(harness, filePath, text) {
  harness.notify("textDocument/didOpen", {
    textDocument: {
      uri: uriFor(filePath),
      languageId: "allium",
      version: 1,
      text,
    },
  });
}

test("initialize advertises core language capabilities", async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "allium-lsp-test-"));
  const harness = new LspHarness(path.resolve(__dirname, "../dist/bin.js"), {
    cwd: path.resolve(__dirname, ".."),
  });
  t.after(() => harness.shutdown());

  const init = await harness.request("initialize", {
    processId: process.pid,
    rootUri: uriFor(root),
    capabilities: {},
    workspaceFolders: [{ uri: uriFor(root), name: path.basename(root) }],
  });

  assert.equal(init.capabilities.hoverProvider, true);
  assert.equal(init.capabilities.definitionProvider, true);
  assert.equal(init.capabilities.referencesProvider, true);
  assert.equal(init.capabilities.documentSymbolProvider, true);
  assert.equal(init.capabilities.workspaceSymbolProvider, true);
  assert.equal(init.capabilities.renameProvider.prepareProvider, true);
  assert.equal(init.capabilities.documentFormattingProvider, true);
  assert.equal(init.capabilities.foldingRangeProvider, true);
  assert.equal(init.capabilities.codeLensProvider.resolveProvider, false);
  assert.equal(init.capabilities.documentLinkProvider.resolveProvider, false);
  assert.ok(init.capabilities.semanticTokensProvider.legend.tokenTypes.length > 0);
});

test("publishes diagnostics on didOpen", async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "allium-lsp-test-"));
  const filePath = writeFile(
    root,
    "bad.allium",
    `rule A {\n  when: Ping(\n  ensures: Done()\n}\n`,
  );
  const text = fs.readFileSync(filePath, "utf8");

  const harness = await createHarness(root);
  t.after(() => harness.shutdown());

  const waitDiagnostics = harness.waitForNotification(
    "textDocument/publishDiagnostics",
    (params) => params.uri === uriFor(filePath),
  );
  await openAlliumDoc(harness, filePath, text);
  const published = await waitDiagnostics;

  assert.ok(Array.isArray(published.diagnostics));
  assert.ok(published.diagnostics.length > 0);
});

test("definition resolves local symbol", async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "allium-lsp-test-"));
  const filePath = writeFile(
    root,
    "main.allium",
    `entity Invitation {\n  status: String\n}\n\nrule A {\n  when: invite: Invitation.status = "pending"\n  ensures: Done()\n}\n`,
  );
  const text = fs.readFileSync(filePath, "utf8");
  const harness = await createHarness(root);
  t.after(() => harness.shutdown());
  await openAlliumDoc(harness, filePath, text);

  const locations = await harness.request("textDocument/definition", {
    textDocument: { uri: uriFor(filePath) },
    position: positionAt(text, "Invitation.status", 3),
  });

  assert.equal(locations.length, 1);
  assert.equal(locations[0].uri, uriFor(filePath));
});

test("definition resolves imported dotted alias symbol", async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "allium-lsp-test-"));
  const shared = writeFile(
    root,
    "shared.allium",
    `entity SharedInvite {\n  status: String\n}\n`,
  );
  const consumer = writeFile(
    root,
    "consumer.allium",
    `use "./shared.allium" as shared\nrule A {\n  when: invite: shared.SharedInvite.status = "ok"\n  ensures: Done()\n}\n`,
  );
  const consumerText = fs.readFileSync(consumer, "utf8");

  const harness = await createHarness(root);
  t.after(() => harness.shutdown());
  await openAlliumDoc(harness, shared, fs.readFileSync(shared, "utf8"));
  await openAlliumDoc(harness, consumer, consumerText);

  const locations = await harness.request("textDocument/definition", {
    textDocument: { uri: uriFor(consumer) },
    position: positionAt(consumerText, "shared.SharedInvite", "shared.Shar".length),
  });

  assert.equal(locations.length, 1);
  assert.equal(locations[0].uri, uriFor(shared));
});

test("references for imported dotted alias include both source and usage", async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "allium-lsp-test-"));
  const shared = writeFile(
    root,
    "shared.allium",
    `entity SharedInvite {\n  status: String\n}\n`,
  );
  const consumer = writeFile(
    root,
    "consumer.allium",
    `use "./shared.allium" as shared\nrule A {\n  when: invite: shared.SharedInvite.status = "ok"\n  ensures: Done()\n}\n`,
  );
  const consumerText = fs.readFileSync(consumer, "utf8");

  const harness = await createHarness(root);
  t.after(() => harness.shutdown());
  await openAlliumDoc(harness, shared, fs.readFileSync(shared, "utf8"));
  await openAlliumDoc(harness, consumer, consumerText);

  const references = await harness.request("textDocument/references", {
    textDocument: { uri: uriFor(consumer) },
    position: positionAt(consumerText, "shared.SharedInvite", "shared.Shared".length),
    context: { includeDeclaration: true },
  });

  assert.ok(references.length >= 2);
  assert.ok(references.some((entry) => entry.uri === uriFor(shared)));
  assert.ok(references.some((entry) => entry.uri === uriFor(consumer)));
});

test("rename updates imported dotted alias usage across files", async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "allium-lsp-test-"));
  const shared = writeFile(
    root,
    "shared.allium",
    `entity SharedInvite {\n  status: String\n}\n`,
  );
  const consumer = writeFile(
    root,
    "consumer.allium",
    `use "./shared.allium" as shared\nrule A {\n  when: invite: shared.SharedInvite.status = "ok"\n  ensures: Done()\n}\n`,
  );
  const consumerText = fs.readFileSync(consumer, "utf8");

  const harness = await createHarness(root);
  t.after(() => harness.shutdown());
  await openAlliumDoc(harness, shared, fs.readFileSync(shared, "utf8"));
  await openAlliumDoc(harness, consumer, consumerText);

  const prepare = await harness.request("textDocument/prepareRename", {
    textDocument: { uri: uriFor(consumer) },
    position: positionAt(consumerText, "shared.SharedInvite", "shared.Shared".length),
  });
  assert.ok(prepare.start);
  assert.ok(prepare.end);

  const edit = await harness.request("textDocument/rename", {
    textDocument: { uri: uriFor(consumer) },
    position: positionAt(consumerText, "shared.SharedInvite", "shared.Shared".length),
    newName: "SharedInviteRenamed",
  });

  assert.ok(edit.changes[uriFor(shared)]);
  assert.ok(edit.changes[uriFor(consumer)]);
  assert.ok(
    edit.changes[uriFor(shared)].some(
      (change) => change.newText === "SharedInviteRenamed",
    ),
  );
});

test("document/workspace symbols, completion, formatting, and links are served", async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "allium-lsp-test-"));
  const shared = writeFile(
    root,
    "shared.allium",
    `entity SharedInvite {\nstatus: String\n}\n\nrule A {\nwhen: Ping()\nensures: Done()\n}\n`,
  );
  const consumer = writeFile(
    root,
    "consumer.allium",
    `use "./shared" as shared\nrule B {\n  when: shared.SharedInvite.status = "ok"\n  ensures: Done()\n}\n`,
  );
  const sharedText = fs.readFileSync(shared, "utf8");
  const consumerText = fs.readFileSync(consumer, "utf8");

  const harness = await createHarness(root);
  t.after(() => harness.shutdown());
  await openAlliumDoc(harness, shared, sharedText);
  await openAlliumDoc(harness, consumer, consumerText);

  const docSymbols = await harness.request("textDocument/documentSymbol", {
    textDocument: { uri: uriFor(shared) },
  });
  assert.ok(docSymbols.some((symbol) => symbol.name === "SharedInvite"));

  const workspaceSymbols = await harness.request("workspace/symbol", {
    query: "SharedInvite",
  });
  assert.ok(workspaceSymbols.some((symbol) => symbol.name === "SharedInvite"));

  const completions = await harness.request("textDocument/completion", {
    textDocument: { uri: uriFor(shared) },
    position: positionAt(sharedText, "when:", 0),
  });
  assert.ok(Array.isArray(completions.items));
  assert.ok(completions.items.length > 0);

  const formatting = await harness.request("textDocument/formatting", {
    textDocument: { uri: uriFor(shared) },
    options: { tabSize: 4, insertSpaces: true },
  });
  assert.ok(formatting.length > 0);
  assert.ok(formatting.some((edit) => typeof edit.newText === "string"));

  const links = await harness.request("textDocument/documentLink", {
    textDocument: { uri: uriFor(consumer) },
  });
  assert.equal(links.length, 1);
  assert.equal(links[0].target, uriFor(shared));
});

test("folding ranges, semantic tokens, and code lens are provided", async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "allium-lsp-test-"));
  const filePath = writeFile(
    root,
    "main.allium",
    `entity Invitation {\n  status: String\n}\n\nrule A {\n  when: Invitation.created()\n  ensures: Invitation.sent()\n}\n`,
  );
  const text = fs.readFileSync(filePath, "utf8");

  const harness = await createHarness(root);
  t.after(() => harness.shutdown());
  await openAlliumDoc(harness, filePath, text);

  const folds = await harness.request("textDocument/foldingRange", {
    textDocument: { uri: uriFor(filePath) },
  });
  assert.ok(folds.length > 0);

  const semantic = await harness.request("textDocument/semanticTokens/full", {
    textDocument: { uri: uriFor(filePath) },
  });
  assert.ok(Array.isArray(semantic.data));
  assert.ok(semantic.data.length > 0);

  const lenses = await harness.request("textDocument/codeLens", {
    textDocument: { uri: uriFor(filePath) },
  });
  assert.ok(lenses.length > 0);
  assert.ok(lenses.every((lens) => lens.command.command === "allium.findReferences"));
});
