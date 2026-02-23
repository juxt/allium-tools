export interface ScaffoldFramework {
  id: string;
  languageId: string;
  header: string[];
  testOpen: (name: string) => string;
  testClose: string;
  comment: (text: string) => string;
  placeholder: string;
  indent: string;
}

export function toSnakeCase(s: string): string {
  return s
    .replace(/([a-z])([A-Z])/g, "$1_$2")
    .replace(/[^a-zA-Z0-9]+/g, "_")
    .replace(/^_|_$/g, "")
    .toLowerCase();
}

export function toKebabCase(s: string): string {
  return s
    .replace(/([a-z])([A-Z])/g, "$1-$2")
    .replace(/[^a-zA-Z0-9]+/g, "-")
    .replace(/^-|-$/g, "")
    .toLowerCase();
}

const frameworks: ReadonlyMap<string, ScaffoldFramework> = new Map([
  [
    "node:test",
    {
      id: "node:test",
      languageId: "typescript",
      header: [
        `import test from "node:test";`,
        `import assert from "node:assert/strict";`,
      ],
      testOpen: (name) => `test("${name}", () => {`,
      testClose: "});",
      comment: (text) => `// ${text}`,
      placeholder: "assert.ok(true);",
      indent: "  ",
    },
  ],
  [
    "jest",
    {
      id: "jest",
      languageId: "typescript",
      header: [],
      testOpen: (name) => `test("${name}", () => {`,
      testClose: "});",
      comment: (text) => `// ${text}`,
      placeholder: "expect(true).toBe(true);",
      indent: "  ",
    },
  ],
  [
    "vitest",
    {
      id: "vitest",
      languageId: "typescript",
      header: [`import { test, expect } from "vitest";`],
      testOpen: (name) => `test("${name}", () => {`,
      testClose: "});",
      comment: (text) => `// ${text}`,
      placeholder: "expect(true).toBe(true);",
      indent: "  ",
    },
  ],
  [
    "pytest",
    {
      id: "pytest",
      languageId: "python",
      header: [],
      testOpen: (name) => `def test_${toSnakeCase(name)}():`,
      testClose: "",
      comment: (text) => `# ${text}`,
      placeholder: "assert True",
      indent: "    ",
    },
  ],
  [
    "junit5",
    {
      id: "junit5",
      languageId: "java",
      header: ["import org.junit.jupiter.api.Test;"],
      testOpen: (name) => `@Test\nvoid ${toSnakeCase(name)}() {`,
      testClose: "}",
      comment: (text) => `// ${text}`,
      placeholder: "// TODO: implement",
      indent: "    ",
    },
  ],
  [
    "clojure.test",
    {
      id: "clojure.test",
      languageId: "clojure",
      header: [`(require '[clojure.test :refer [deftest is]])`],
      testOpen: (name) => `(deftest ${toKebabCase(name)}-test`,
      testClose: ")",
      comment: (text) => `;; ${text}`,
      placeholder: "(is (= 1 1))",
      indent: "  ",
    },
  ],
]);

export function getFramework(id: string): ScaffoldFramework | undefined {
  return frameworks.get(id);
}

export function frameworkIds(): string[] {
  return [...frameworks.keys()];
}
