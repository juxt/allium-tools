import test from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { formatAlliumText } from "../src/format";

function runFormat(
  args: string[],
  cwd: string,
  input?: string,
): { status: number | null; stdout: string; stderr: string } {
  const result = spawnSync(
    process.execPath,
    [path.resolve("dist/src/format.js"), ...args],
    { cwd, encoding: "utf8", input },
  );
  return {
    status: result.status,
    stdout: result.stdout,
    stderr: result.stderr,
  };
}

function runFormatWithStdin(
  args: string[],
  cwd: string,
  input: string,
  timeoutMs = 2000,
): Promise<{ status: number | null; stdout: string; stderr: string }> {
  return new Promise((resolve, reject) => {
    const child = spawn(
      process.execPath,
      [path.resolve("dist/src/format.js"), ...args],
      {
        cwd,
        stdio: ["pipe", "pipe", "pipe"],
      },
    );
    let stdout = "";
    let stderr = "";
    let settled = false;

    const finish = (fn: () => void) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      fn();
    };

    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      stdout += chunk;
    });

    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (chunk: string) => {
      stderr += chunk;
    });

    child.on("error", (error) => finish(() => reject(error)));
    child.on("exit", (code) => {
      finish(() => resolve({ status: code, stdout, stderr }));
    });

    child.stdin.end(input);

    const timer = setTimeout(() => {
      child.kill("SIGTERM");
      finish(() =>
        reject(
          new Error(
            `format --stdin --stdout timed out after ${timeoutMs}ms.\nstdout:\n${stdout}\nstderr:\n${stderr}`,
          ),
        ),
      );
    }, timeoutMs);
  });
}

test("formatAlliumText normalizes line endings and trims trailing whitespace", () => {
  const input = "rule A {\r\n  when: Ping()  \r\n}\r\n\r\n";
  const output = formatAlliumText(input);
  assert.equal(output, "rule A {\n    when: Ping()\n}\n");
});

test("formatAlliumText applies structural indentation and top-level spacing", () => {
  const input =
    "rule A{\nwhen: Ping()\nensures: Done()\n}\nrule B {\nwhen: Pong()\nensures: Done()\n}\n";
  const output = formatAlliumText(input);
  assert.equal(
    output,
    "rule A {\n    when: Ping()\n    ensures: Done()\n}\n\nrule B {\n    when: Pong()\n    ensures: Done()\n}\n",
  );
});

test("formatAlliumText respects indent width and top-level spacing options", () => {
  const input = "rule A {\nwhen: Ping()\n}\nrule B {\nwhen: Pong()\n}\n";
  const output = formatAlliumText(input, {
    indentWidth: 2,
    topLevelSpacing: 2,
  });
  assert.equal(
    output,
    "rule A {\n  when: Ping()\n}\n\n\nrule B {\n  when: Pong()\n}\n",
  );
});

test("formatAlliumText normalizes pipe spacing in enum literals", () => {
  const input = "enum Recommendation {\nstrong_yes| yes|no |strong_no\n}\n";
  const output = formatAlliumText(input);
  assert.equal(
    output,
    "enum Recommendation {\n    strong_yes | yes | no | strong_no\n}\n",
  );
});

test("formatAlliumText normalizes declaration header brace spacing", () => {
  const input =
    'enum Recommendation{\nstrong_yes|yes\n}\nrule A{\nwhen: Ping()\n}\nopen question SpecGap{\nwhy: "x"\n}\n';
  const output = formatAlliumText(input);
  assert.equal(
    output,
    'enum Recommendation {\n    strong_yes | yes\n}\n\nrule A {\n    when: Ping()\n}\n\nopen question SpecGap {\n    why: "x"\n}\n',
  );
});

test("format CLI rewrites .allium files", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-format-"));
  const target = path.join(dir, "spec.allium");
  fs.writeFileSync(target, "rule A {\r\n  when: Ping()  \r\n}\r\n", "utf8");

  const result = runFormat(["spec.allium"], dir);
  assert.equal(result.status, 0);
  assert.match(result.stdout, /spec\.allium: formatted/);
  assert.equal(
    fs.readFileSync(target, "utf8"),
    "rule A {\n    when: Ping()\n}\n",
  );
});

test("format CLI --check fails when formatting is needed", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-format-"));
  const target = path.join(dir, "spec.allium");
  fs.writeFileSync(target, "rule A {\n  when: Ping()  \n}\n", "utf8");

  const result = runFormat(["--check", "spec.allium"], dir);
  assert.equal(result.status, 1);
  assert.match(result.stdout, /spec\.allium: would format/);
});

test("format CLI --check succeeds when file is already formatted", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-format-"));
  const target = path.join(dir, "spec.allium");
  fs.writeFileSync(target, "rule A {\n    when: Ping()\n}\n", "utf8");

  const result = runFormat(["--check", "spec.allium"], dir);
  assert.equal(result.status, 0);
  assert.match(result.stdout, /All files already formatted\./);
});

test("format CLI fails with exit code 2 when no inputs are provided", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-format-"));
  const result = runFormat([], dir);
  assert.equal(result.status, 2);
  assert.match(result.stderr, /Provide at least one file, directory, or glob/);
});

test("format CLI uses project.specPaths from config when no inputs are passed", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-format-"));
  fs.mkdirSync(path.join(dir, "specs"), { recursive: true });
  fs.writeFileSync(
    path.join(dir, "allium.config.json"),
    JSON.stringify({ project: { specPaths: ["specs"] } }),
    "utf8",
  );
  const target = path.join(dir, "specs", "spec.allium");
  fs.writeFileSync(target, "rule A {\nwhen: Ping()\n}\n", "utf8");
  const result = runFormat([], dir);
  assert.equal(result.status, 0);
  assert.equal(
    fs.readFileSync(target, "utf8"),
    "rule A {\n    when: Ping()\n}\n",
  );
});

test("format CLI fails with exit code 2 when input has no .allium files", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-format-"));
  fs.writeFileSync(path.join(dir, "notes.txt"), "no allium", "utf8");

  const result = runFormat(["notes.txt"], dir);
  assert.equal(result.status, 2);
  assert.match(result.stderr, /No \.allium files found/);
});

test("format CLI accepts indent and spacing options", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-format-"));
  const target = path.join(dir, "spec.allium");
  fs.writeFileSync(target, "rule A {\nwhen: Ping()\n}\n", "utf8");

  const result = runFormat(
    ["--indent-width", "2", "--top-level-spacing", "0", "spec.allium"],
    dir,
  );
  assert.equal(result.status, 0);
  assert.equal(
    fs.readFileSync(target, "utf8"),
    "rule A {\n  when: Ping()\n}\n",
  );
});

test("format CLI dryrun previews changes without writing file", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-format-"));
  const target = path.join(dir, "spec.allium");
  const original = "rule A {\nwhen: Ping()\n}\n";
  fs.writeFileSync(target, original, "utf8");

  const result = runFormat(["--dryrun", "spec.allium"], dir);
  assert.equal(result.status, 0);
  assert.match(result.stdout, /formatted preview/);
  assert.equal(fs.readFileSync(target, "utf8"), original);
});

test("format CLI supports stdin stdout mode", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-format-"));
  const input = "rule A {\nwhen: Ping()\n}\n";
  const result = await runFormatWithStdin(["--stdin", "--stdout"], dir, input);
  assert.equal(result.status, 0);
  assert.match(result.stdout, / {4}when: Ping\(\)/);
});

test("format CLI reads defaults from allium.config.json", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-format-"));
  fs.writeFileSync(
    path.join(dir, "allium.config.json"),
    JSON.stringify({ format: { indentWidth: 2, topLevelSpacing: 0 } }),
    "utf8",
  );
  const target = path.join(dir, "spec.allium");
  fs.writeFileSync(target, "rule A {\nwhen: Ping()\n}\n", "utf8");
  const result = runFormat(["spec.allium"], dir);
  assert.equal(result.status, 0);
  assert.equal(
    fs.readFileSync(target, "utf8"),
    "rule A {\n  when: Ping()\n}\n",
  );
});
