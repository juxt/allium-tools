import test from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { spawn, spawnSync } from "node:child_process";

const checkCliPath = path.resolve(__dirname, "../src/check.js");

function writeAllium(
  dir: string,
  relativePath: string,
  contents: string,
): string {
  const fullPath = path.join(dir, relativePath);
  fs.mkdirSync(path.dirname(fullPath), { recursive: true });
  fs.writeFileSync(fullPath, contents, "utf8");
  return fullPath;
}

function runCheck(
  args: string[],
  cwd: string,
  timeoutMs?: number,
  input?: string,
  allowTimeout = false,
): { status: number | null; stdout: string; stderr: string } {
  if (!fs.existsSync(checkCliPath)) {
    throw new Error(`check.js not found at ${checkCliPath}`);
  }
  const result = spawnSync(process.execPath, [checkCliPath, ...args], {
    cwd,
    encoding: "utf8",
    timeout: timeoutMs,
    input,
  });
  if (result.error) {
    const spawnError = result.error as NodeJS.ErrnoException;
    if (allowTimeout && spawnError.code === "ETIMEDOUT") {
      return {
        status: result.status,
        stdout: result.stdout,
        stderr: result.stderr,
      };
    }
    throw spawnError;
  }
  return {
    status: result.status,
    stdout: result.stdout,
    stderr: result.stderr,
  };
}

function runCheckWatchOnce(
  cwd: string,
  requiredOutputPatterns: RegExp[],
  maxWaitMs = 2000,
): Promise<{ stdout: string; stderr: string }> {
  if (!fs.existsSync(checkCliPath)) {
    return Promise.reject(new Error(`check.js not found at ${checkCliPath}`));
  }

  return new Promise((resolve, reject) => {
    const child = spawn(
      process.execPath,
      [checkCliPath, "--watch", "spec.allium"],
      {
        cwd,
        stdio: ["ignore", "pipe", "pipe"],
      },
    );
    let stdout = "";
    let stderr = "";
    let settled = false;

    const finish = (fn: () => void) => {
      if (settled) return;
      settled = true;
      clearTimeout(safetyTimer);
      fn();
    };

    const maybeResolve = () => {
      if (requiredOutputPatterns.every((pattern) => pattern.test(stdout))) {
        child.kill("SIGTERM");
        finish(() => resolve({ stdout, stderr }));
      }
    };

    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      stdout += chunk;
      maybeResolve();
    });

    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (chunk: string) => {
      stderr += chunk;
    });

    child.on("error", (error) => {
      finish(() => reject(error));
    });

    child.on("exit", () => {
      if (settled) return;
      finish(() =>
        reject(
          new Error(
            `watch process exited before expected output.\nstdout:\n${stdout}\nstderr:\n${stderr}`,
          ),
        ),
      );
    });

    const safetyTimer = setTimeout(() => {
      child.kill("SIGTERM");
      finish(() =>
        reject(
          new Error(
            `watch process did not produce expected output within ${maxWaitMs}ms.\nstdout:\n${stdout}\nstderr:\n${stderr}`,
          ),
        ),
      );
    }, maxWaitMs);
  });
}

function runCheckWithInput(
  args: string[],
  cwd: string,
  input: string,
  timeoutMs = 1500,
): Promise<{ status: number | null; stdout: string; stderr: string }> {
  if (!fs.existsSync(checkCliPath)) {
    return Promise.reject(new Error(`check.js not found at ${checkCliPath}`));
  }

  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, [checkCliPath, ...args], {
      cwd,
      stdio: ["pipe", "pipe", "pipe"],
    });
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
            `check CLI timed out after ${timeoutMs}ms.\nstdout:\n${stdout}\nstderr:\n${stderr}`,
          ),
        ),
      );
    }, timeoutMs);
  });
}

test("fails with exit code 1 on strict warning", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  writeAllium(
    dir,
    "spec.allium",
    `entity Invitation {\n  expires_at: Timestamp\n  status: String\n}\n\nrule Expires {\n  when: invitation: Invitation.expires_at <= now\n  ensures: invitation.status = expired\n}\n`,
  );

  const result = runCheck(["spec.allium"], dir);
  assert.equal(result.status, 1);
  assert.match(result.stdout, /allium\.temporal\.missingGuard/);
});

test("relaxed mode suppresses temporal warning and returns success", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  writeAllium(
    dir,
    "spec.allium",
    `entity Invitation {\n  expires_at: Timestamp\n  status: String\n}\n\nrule Expires {\n  when: invitation: Invitation.expires_at <= now\n  ensures: invitation.status = expired\n}\n`,
  );

  const result = runCheck(["--mode", "relaxed", "spec.allium"], dir);
  assert.equal(result.status, 0);
  assert.match(result.stdout, /No blocking findings\./);
});

test("checks .allium files found through directory input", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  writeAllium(dir, "nested/a.allium", `rule A {\n  when: Ping()\n}\n`);
  writeAllium(dir, "nested/readme.txt", "ignore");

  const result = runCheck(["nested"], dir);
  assert.equal(result.status, 1);
  assert.match(
    result.stdout,
    /nested\/a\.allium:3:1 error allium\.rule\.missingEnsures/,
  );
});

test("returns exit code 2 for invalid mode", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  writeAllium(
    dir,
    "spec.allium",
    `rule A {\n  when: Ping()\n  ensures: Done()\n}\n`,
  );

  const result = runCheck(["--mode", "invalid", "spec.allium"], dir);
  assert.equal(result.status, 2);
  assert.match(result.stderr, /Expected --mode strict\|relaxed/);
});

test("returns exit code 2 when no inputs are provided", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  const result = runCheck([], dir);
  assert.equal(result.status, 2);
  assert.match(result.stderr, /Provide at least one file, directory, or glob/);
});

test("uses project.specPaths from config when no inputs are passed", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  writeAllium(dir, "specs/spec.allium", `rule A {\n  when: Ping()\n}\n`);
  fs.writeFileSync(
    path.join(dir, "allium.config.json"),
    JSON.stringify({ project: { specPaths: ["specs"] } }),
    "utf8",
  );
  const result = runCheck([], dir);
  assert.equal(result.status, 1);
  assert.match(result.stdout, /specs\/spec\.allium/);
});

test("returns exit code 2 when inputs resolve to no .allium files", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  fs.writeFileSync(path.join(dir, "readme.txt"), "no spec files", "utf8");

  const result = runCheck(["readme.txt"], dir);
  assert.equal(result.status, 2);
  assert.match(result.stderr, /No \.allium files found/);
});

test("supports wildcard inputs and checks matched files", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  writeAllium(dir, "specs/a.allium", `rule A {\n  when: Ping()\n}\n`);
  writeAllium(
    dir,
    "specs/b.allium",
    `rule B {\n  when: Pong()\n  ensures: Done()\n}\n`,
  );
  fs.writeFileSync(path.join(dir, "specs/c.txt"), "ignore", "utf8");

  const result = runCheck(["specs/*.allium"], dir);
  assert.equal(result.status, 1);
  assert.match(result.stdout, /specs\/a\.allium/);
  assert.doesNotMatch(result.stdout, /specs\/c\.txt/);
});

test("autofix adds missing ensures and returns success", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  const filePath = writeAllium(
    dir,
    "spec.allium",
    `rule A {\n  when: Ping()\n}\n`,
  );

  const result = runCheck(["--autofix", "spec.allium"], dir);
  assert.equal(result.status, 0);
  assert.match(result.stdout, /autofixed/);

  const updated = fs.readFileSync(filePath, "utf8");
  assert.match(updated, /ensures: TODO\(\)/);
});

test("autofix adds temporal guard scaffold", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  const filePath = writeAllium(
    dir,
    "spec.allium",
    `entity Invitation {\n  expires_at: Timestamp\n  status: String\n}\n\nrule Expires {\n  when: invitation: Invitation.expires_at <= now\n  ensures: invitation.status = expired\n}\n`,
  );

  const result = runCheck(["--autofix", "spec.allium"], dir);
  assert.equal(result.status, 0);

  const updated = fs.readFileSync(filePath, "utf8");
  assert.match(updated, /requires: \/\* add temporal guard \*\//);
});

test("autofix adds missing when scaffold", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  const filePath = writeAllium(
    dir,
    "spec.allium",
    `rule A {\n  ensures: Done()\n}\n`,
  );
  const result = runCheck(["--autofix", "spec.allium"], dir);
  assert.equal(result.status, 0);
  const updated = fs.readFileSync(filePath, "utf8");
  assert.match(updated, /when: TODO\(\)/);
});

test("json format prints machine-readable payload", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  writeAllium(dir, "spec.allium", `rule A {\n  when: Ping()\n}\n`);

  const result = runCheck(["--format", "json", "spec.allium"], dir);
  assert.equal(result.status, 1);
  const parsed = JSON.parse(result.stdout) as {
    summary: { findings: number; errors: number };
    findings: Array<{ code: string }>;
  };
  assert.equal(parsed.summary.findings > 0, true);
  assert.equal(parsed.summary.errors > 0, true);
  assert.ok(
    parsed.findings.some(
      (entry) => entry.code === "allium.rule.missingEnsures",
    ),
  );
});

test("write-baseline creates baseline file and exits successfully", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  writeAllium(dir, "spec.allium", `rule A {\n  when: Ping()\n}\n`);

  const result = runCheck(
    ["--write-baseline", ".allium-baseline.json", "spec.allium"],
    dir,
  );
  assert.equal(result.status, 0);
  assert.match(result.stdout, /Wrote baseline/);

  const baseline = JSON.parse(
    fs.readFileSync(path.join(dir, ".allium-baseline.json"), "utf8"),
  ) as { version: number; findings: unknown[] };
  assert.equal(baseline.version, 1);
  assert.equal(baseline.findings.length > 0, true);
});

test("baseline suppresses known findings", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  writeAllium(dir, "spec.allium", `rule A {\n  when: Ping()\n}\n`);
  runCheck(["--write-baseline", ".allium-baseline.json", "spec.allium"], dir);

  const result = runCheck(
    ["--baseline", ".allium-baseline.json", "spec.allium"],
    dir,
  );
  assert.equal(result.status, 0);
  assert.match(result.stdout, /Suppressed/);
  assert.match(result.stdout, /No blocking findings\./);
});

test("min-severity hides informational findings", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  writeAllium(
    dir,
    "spec.allium",
    `rule A {\n  when: Ping()\n  ensures: Done()\n}\n`,
  );

  const result = runCheck(
    ["--format", "json", "--min-severity", "warning", "spec.allium"],
    dir,
  );
  assert.equal(result.status, 0);
  const parsed = JSON.parse(result.stdout) as {
    summary: { findings: number; infos: number };
  };
  assert.equal(parsed.summary.findings, 0);
  assert.equal(parsed.summary.infos, 0);
});

test("ignore-code suppresses matching findings", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  writeAllium(
    dir,
    "spec.allium",
    `rule A {\n  when: Ping()\n  ensures: Done()\n}\n`,
  );

  const result = runCheck(
    [
      "--format",
      "json",
      "--ignore-code",
      "allium.rule.unreachableTrigger",
      "spec.allium",
    ],
    dir,
  );
  assert.equal(result.status, 0);
  const parsed = JSON.parse(result.stdout) as {
    findings: Array<{ code: string }>;
  };
  assert.equal(parsed.findings.length, 0);
});

test("stats prints finding counts by code", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  writeAllium(
    dir,
    "spec.allium",
    `rule A {\n  when: Ping()\n}\nrule B {\n  when: Pong()\n}\n`,
  );
  const result = runCheck(["--stats", "spec.allium"], dir);
  assert.equal(result.status, 1);
  assert.match(result.stdout, /Stats by code:/);
  assert.match(result.stdout, /allium\.rule\.missingEnsures/);
});

test("autofix dryrun does not persist file edits", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  const filePath = writeAllium(
    dir,
    "spec.allium",
    `rule A {\n  when: Ping()\n}\n`,
  );
  const before = fs.readFileSync(filePath, "utf8");

  const result = runCheck(["--autofix", "--dryrun", "spec.allium"], dir);
  assert.equal(result.status, 0);
  assert.match(result.stdout, /would autofix/);
  const after = fs.readFileSync(filePath, "utf8");
  assert.equal(after, before);
});

test("changed checks modified allium files", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  writeAllium(dir, "spec.allium", `rule A {\n  when: Ping()\n}\n`);
  spawnSync("git", ["init"], { cwd: dir, encoding: "utf8" });
  spawnSync("git", ["config", "user.email", "test@example.com"], {
    cwd: dir,
    encoding: "utf8",
  });
  spawnSync("git", ["config", "user.name", "Test"], {
    cwd: dir,
    encoding: "utf8",
  });
  spawnSync("git", ["add", "."], { cwd: dir, encoding: "utf8" });
  spawnSync("git", ["commit", "-m", "init"], { cwd: dir, encoding: "utf8" });
  fs.writeFileSync(
    path.join(dir, "spec.allium"),
    `rule A {\n  when: Ping()\n}\n\n`,
  );

  const result = runCheck(["--changed"], dir);
  assert.equal(result.status, 1);
  assert.match(result.stdout, /allium\.rule\.missingEnsures/);
});

test("fail-on info returns failure when informational findings exist", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  writeAllium(
    dir,
    "spec.allium",
    `rule A {\n  when: Ping()\n  ensures: Done()\n}\n`,
  );

  const result = runCheck(
    ["--fail-on", "info", "--format", "json", "spec.allium"],
    dir,
  );
  assert.equal(result.status, 1);
});

test("fail-on error ignores warnings for exit code", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  writeAllium(
    dir,
    "spec.allium",
    `entity Invitation {\n  expires_at: Timestamp\n  status: String\n}\n\nrule Expires {\n  when: invitation: Invitation.expires_at <= now\n  ensures: invitation.status = expired\n}\n`,
  );

  const result = runCheck(["--fail-on", "error", "spec.allium"], dir);
  assert.equal(result.status, 0);
});

test("report writes emitted output to file", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  writeAllium(dir, "spec.allium", `rule A {\n  when: Ping()\n}\n`);

  const result = runCheck(
    ["--format", "json", "--report", "out/report.json", "spec.allium"],
    dir,
  );
  assert.equal(result.status, 1);
  const report = fs.readFileSync(path.join(dir, "out", "report.json"), "utf8");
  assert.equal(report.trim().startsWith("{"), true);
});

test("fix-code limits autofix to selected finding codes", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  const filePath = writeAllium(
    dir,
    "spec.allium",
    `entity Invitation {\n  expires_at: Timestamp\n  status: String\n}\n\nrule Expires {\n  when: invitation: Invitation.expires_at <= now\n}\n`,
  );

  const result = runCheck(
    ["--autofix", "--fix-code", "allium.rule.missingEnsures", "spec.allium"],
    dir,
  );
  assert.equal(result.status, 1);
  const updated = fs.readFileSync(filePath, "utf8");
  assert.match(updated, /ensures: TODO\(\)/);
  assert.doesNotMatch(updated, /requires: \/\* add temporal guard \*\//);
});

test("watch mode runs an initial cycle", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  writeAllium(dir, "spec.allium", `rule A {\n  when: Ping()\n}\n`);
  const result = await runCheckWatchOnce(
    dir,
    [/allium-check watch/, /allium\.rule\.missingEnsures/],
    2000,
  );
  assert.match(result.stdout, /allium-check watch/);
  assert.match(result.stdout, /allium\.rule\.missingEnsures/);
});

test("fix-code requires autofix", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  writeAllium(dir, "spec.allium", `rule A {\n  when: Ping()\n}\n`);
  const result = runCheck(
    ["--fix-code", "allium.rule.missingEnsures", "spec.allium"],
    dir,
  );
  assert.equal(result.status, 2);
  assert.match(result.stderr, /--fix-code requires --autofix/);
});

test("fix-interactive applies selected fixes only", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  const filePath = writeAllium(
    dir,
    "spec.allium",
    `entity Invitation {\n  expires_at: Timestamp\n  status: String\n}\n\nrule Expires {\n  when: invitation: Invitation.expires_at <= now\n}\n`,
  );
  const result = await runCheckWithInput(
    ["--autofix", "--fix-interactive", "spec.allium"],
    dir,
    "y\nn\nq\n",
  );
  assert.equal(result.status, 1);
  assert.match(result.stdout, /Apply fix allium\.rule\.missingEnsures/);
  const updated = fs.readFileSync(filePath, "utf8");
  assert.match(updated, /ensures: TODO\(\)/);
  assert.doesNotMatch(updated, /requires: \/\* add temporal guard \*\//);
});

test("sarif includes rule metadata and help uri", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  writeAllium(dir, "spec.allium", `rule A {\n  when: Ping()\n}\n`);
  const result = runCheck(["--format", "sarif", "spec.allium"], dir);
  assert.equal(result.status, 1);
  const payload = JSON.parse(result.stdout) as {
    runs: Array<{ tool: { driver: { rules: Array<{ helpUri: string }> } } }>;
  };
  assert.equal(payload.runs[0].tool.driver.rules.length > 0, true);
  assert.match(
    payload.runs[0].tool.driver.rules[0].helpUri,
    /allium\/language/,
  );
});

test("check loads defaults from allium.config.json", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  writeAllium(
    dir,
    "spec.allium",
    `entity Invitation {\n  expires_at: Timestamp\n  status: String\n}\n\nrule Expires {\n  when: invitation: Invitation.expires_at <= now\n  ensures: invitation.status = expired\n}\n`,
  );
  fs.writeFileSync(
    path.join(dir, "allium.config.json"),
    JSON.stringify({ check: { mode: "relaxed" } }),
    "utf8",
  );
  const result = runCheck(["spec.allium"], dir);
  assert.equal(result.status, 0);
});

test("cache mode writes cache file", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "allium-check-"));
  writeAllium(dir, "spec.allium", `rule A {\n  when: Ping()\n}\n`);
  const result = runCheck(["--cache", "spec.allium"], dir);
  assert.equal(result.status, 1);
  assert.equal(fs.existsSync(path.join(dir, ".allium-check-cache.json")), true);
});
