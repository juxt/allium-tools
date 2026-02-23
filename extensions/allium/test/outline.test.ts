import test from "node:test";
import assert from "node:assert/strict";
import { collectAlliumSymbols } from "../src/language-tools/outline";

test("collects symbols for core top-level blocks", () => {
  const text = `
config {
  timeout: Integer = 5
}

entity Child {
  name: String
}

rule Notify {
  when: Ping()
  ensures: Sent()
}
`;

  const symbols = collectAlliumSymbols(text);
  assert.equal(symbols.length, 3);
  assert.deepEqual(
    symbols.map((s) => `${s.type}:${s.name}`),
    ["config:config", "entity:Child", "rule:Notify"],
  );
});

test("collects external entity, value, variant, enum, default, surface, and actor", () => {
  const text = `
external entity RegistryRecord {
  id: String
}

value DurationWindow {
  hours: Integer
}

variant PremiumRecord : RegistryRecord {
  priority: Integer
}

enum Recommendation {
  strong_yes | yes | no | strong_no
}

default Role viewer = {
  name: "viewer"
}

surface ChildView {
  facing parent: Parent
}

actor Parent {
  identified_by: User.email
}
`;

  const symbols = collectAlliumSymbols(text);
  assert.deepEqual(
    symbols.map((s) => `${s.type}:${s.name}`),
    [
      "external entity:RegistryRecord",
      "value:DurationWindow",
      "variant:PremiumRecord",
      "enum:Recommendation",
      "default:viewer",
      "surface:ChildView",
      "actor:Parent",
    ],
  );
});
