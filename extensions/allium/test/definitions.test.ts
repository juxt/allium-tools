import test from "node:test";
import assert from "node:assert/strict";
import {
  buildDefinitionLookup,
  findDefinitionsAtOffset,
  importedSymbolAtOffset,
  parseUseAliases,
} from "../src/language-tools/definitions";

test("collects top-level symbol definitions", () => {
  const text = `
entity Invitation {
  status: String
}

enum Recommendation {
  strong_yes | yes | no | strong_no
}

default Role viewer = {
  name: "viewer"
}

rule ExpireInvitation {
  when: invitation: Invitation.expires_at <= now
  requires: invitation.role = viewer
  requires: invitation.recommendation = Recommendation.yes
  ensures: invitation.status = "expired"
}
`;
  const lookup = buildDefinitionLookup(text);
  assert.deepEqual(lookup.symbols.map((s) => s.name).sort(), [
    "ExpireInvitation",
    "Invitation",
    "Recommendation",
    "viewer",
  ]);
});

test("finds entity definition at usage offset", () => {
  const text = `
entity Invitation {
  status: String
}

rule ExpireInvitation {
  when: invitation: Invitation.expires_at <= now
  ensures: invitation.status = "expired"
}
`;
  const usageOffset =
    text.indexOf("Invitation.expires_at") + "Invitation".length - 1;
  const matches = findDefinitionsAtOffset(text, usageOffset);
  assert.equal(matches.length, 1);
  assert.equal(matches[0].name, "Invitation");
});

test("finds config key definition from config reference", () => {
  const text = `
config {
  timeout_hours: Integer = 12
}

rule ExpireInvitation {
  when: invitation: Invitation.expires_at <= now + config.timeout_hours
  ensures: invitation.status = "expired"
}
`;
  const usageOffset =
    text.indexOf("config.timeout_hours") + "config.timeout_".length;
  const matches = findDefinitionsAtOffset(text, usageOffset);
  assert.equal(matches.length, 1);
  assert.equal(matches[0].name, "timeout_hours");
});

test("returns no definition for unknown symbol", () => {
  const text = `
rule A {
  when: Ping()
  ensures: UnknownThing()
}
`;
  const usageOffset = text.indexOf("UnknownThing") + 2;
  const matches = findDefinitionsAtOffset(text, usageOffset);
  assert.equal(matches.length, 0);
});

test("finds enum definition at usage offset", () => {
  const text = `
enum Recommendation {
  strong_yes | yes | no | strong_no
}

rule A {
  when: Ping()
  ensures: recommendation = Recommendation.yes
}
`;
  const usageOffset =
    text.indexOf("Recommendation.yes") + "Recommendation".length - 1;
  const matches = findDefinitionsAtOffset(text, usageOffset);
  assert.equal(matches.length, 1);
  assert.equal(matches[0].name, "Recommendation");
});

test("finds default instance definition at usage offset", () => {
  const text = `
default Role viewer = {
  name: "viewer"
}

rule A {
  when: Ping()
  ensures: role = viewer
}
`;
  const usageOffset = text.indexOf("role = viewer") + "role = view".length;
  const matches = findDefinitionsAtOffset(text, usageOffset);
  assert.equal(matches.length, 1);
  assert.equal(matches[0].name, "viewer");
});

test("parses use aliases from document", () => {
  const text = `use "./shared.allium" as shared\nrule A {\n  when: shared/Ping()\n  ensures: Done()\n}\n`;
  const aliases = parseUseAliases(text);
  assert.deepEqual(aliases, [
    { alias: "shared", sourcePath: "./shared.allium" },
  ]);
});

test("extracts imported symbol token at offset", () => {
  const text = `rule A {\n  when: shared/Ping()\n  ensures: Done()\n}\n`;
  const offset = text.indexOf("shared/Ping") + "shared/Pi".length;
  const imported = importedSymbolAtOffset(text, offset);
  assert.deepEqual(imported, { alias: "shared", symbol: "Ping" });
});

test("extracts dotted imported symbol token at offset", () => {
  const text = `rule A {\n  when: shared.Ping()\n  ensures: Done()\n}\n`;
  const offset = text.indexOf("shared.Ping") + "shared.Pi".length;
  const imported = importedSymbolAtOffset(text, offset);
  assert.deepEqual(imported, { alias: "shared", symbol: "Ping" });
});
