import test from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";

test("snippet catalog includes key official language constructs", () => {
  const snippetsPath = path.resolve(
    "language-basics/snippets/allium.code-snippets",
  );
  const snippets = JSON.parse(fs.readFileSync(snippetsPath, "utf8")) as Record<
    string,
    { prefix: string; body: string[] }
  >;

  assert.equal(snippets.Enum?.prefix, "enum");
  assert.equal(snippets.Given?.prefix, "given");
  assert.equal(snippets.Default?.prefix, "default");
  assert.equal(snippets.Module?.prefix, "module");
  assert.ok(snippets.Enum.body.some((line) => line.includes("EnumName")));
});
