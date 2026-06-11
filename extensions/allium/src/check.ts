#!/usr/bin/env node
import * as fs from "node:fs";
import * as path from "node:path";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  analyzeAllium,
  type DiagnosticsMode,
  type Finding,
} from "./language-tools/analyzer";

type CheckOutputFormat = "text" | "json" | "sarif";

interface ParsedArgs {
  mode: DiagnosticsMode;
  autofix: boolean;
  fixInteractive: boolean;
  dryRun: boolean;
  watch: boolean;
  changedOnly: boolean;
  showStats: boolean;
  minSeverity: Finding["severity"];
  failOnSeverity: Finding["severity"];
  fixCodes: Set<string>;
  ignoreCodes: Set<string>;
  format: CheckOutputFormat;
  reportPath?: string;
  cache: boolean;
  cachePath: string;
  baselinePath?: string;
  writeBaselinePath?: string;
  inputs: string[];
}

interface TextEdit {
  offset: number;
  text: string;
}

interface SafeFixPlan extends TextEdit {
  code: string;
  label: string;
}

interface FindingRecord {
  filePath: string;
  finding: Finding;
  fingerprint: string;
}

interface BaselineFile {
  version: 1;
  findings: Array<{ fingerprint: string }>;
}

interface CacheFile {
  version: 1;
  mode: DiagnosticsMode;
  files: Record<
    string,
    {
      hash: string;
      imports: string[];
      findings: Finding[];
    }
  >;
}

interface AlliumConfig {
  project?: {
    specPaths?: string[];
  };
  check?: {
    mode?: DiagnosticsMode;
    minSeverity?: Finding["severity"];
    failOn?: Finding["severity"];
    ignoreCodes?: string[];
    fixCodes?: string[];
  };
}

function main(argv: string[]): number | null {
  const parsed = parseArgs(argv);
  if (!parsed) {
    return 2;
  }

  if (parsed.watch) {
    runWatch(parsed);
    return null;
  }

  return runCheckOnce(parsed);
}

interface CheckCycleResult {
  output: string;
  hasFailing: boolean;
  signature: string;
  noFiles: boolean;
}

function runCheckOnce(parsed: ParsedArgs): number {
  const cycle = evaluateCheckRun(parsed);
  if (cycle.noFiles) {
    process.stderr.write("No .allium files found for the provided inputs.\n");
    return 2;
  }
  process.stdout.write(cycle.output);
  if (parsed.reportPath) {
    writeReport(parsed.reportPath, cycle.output);
  }
  if (parsed.format === "text" && !cycle.hasFailing) {
    process.stdout.write("No blocking findings.\n");
  }
  return cycle.hasFailing ? 1 : 0;
}

function runWatch(parsed: ParsedArgs): void {
  let previousSignature = "";
  const runCycle = (): void => {
    const cycle = evaluateCheckRun(parsed);
    if (cycle.signature === previousSignature) {
      return;
    }
    previousSignature = cycle.signature;
    process.stdout.write(
      `\n== allium-check watch (${new Date().toISOString()}) ==\n`,
    );
    if (cycle.noFiles) {
      process.stderr.write("No .allium files found for the provided inputs.\n");
      process.exitCode = 2;
      return;
    }
    process.stdout.write(cycle.output);
    if (parsed.reportPath) {
      writeReport(parsed.reportPath, cycle.output);
    }
    if (parsed.format === "text" && !cycle.hasFailing) {
      process.stdout.write("No blocking findings.\n");
    }
    process.exitCode = cycle.hasFailing ? 1 : 0;
  };

  runCycle();
  setInterval(runCycle, 1000);
}

function evaluateCheckRun(parsed: ParsedArgs): CheckCycleResult {
  const changedInputs = parsed.changedOnly ? resolveChangedAlliumInputs() : [];
  const files = resolveInputs([...parsed.inputs, ...changedInputs]);
  if (files.length === 0) {
    return {
      output: "",
      hasFailing: true,
      signature: "no-files",
      noFiles: true,
    };
  }

  const allFindings: FindingRecord[] = [];
  const useCache = parsed.cache && !parsed.autofix;
  const cache = useCache ? loadCache(parsed.cachePath, parsed.mode) : undefined;
  const textByFile = new Map<string, string>();
  const hashByFile = new Map<string, string>();
  const importsByFile = new Map<string, string[]>();
  for (const filePath of files) {
    const text = fs.readFileSync(filePath, "utf8");
    textByFile.set(filePath, text);
    hashByFile.set(filePath, hashText(text));
    importsByFile.set(filePath, parseImports(filePath, text));
  }
  const affectedFiles =
    useCache && cache
      ? collectAffectedFiles(files, cache, hashByFile, importsByFile)
      : new Set(files);

  for (const filePath of files) {
    let text = textByFile.get(filePath) ?? "";

    if (parsed.autofix) {
      const fixed = parsed.fixInteractive
        ? applyAutoFixesInteractive(
            text,
            parsed.mode,
            parsed.fixCodes,
            path.relative(process.cwd(), filePath) || filePath,
          )
        : applyAutoFixes(text, parsed.mode, parsed.fixCodes);
      if (fixed !== text) {
        if (!parsed.dryRun) {
          fs.writeFileSync(filePath, fixed, "utf8");
        }
        text = fixed;
        if (parsed.format === "text") {
          process.stdout.write(
            `${path.relative(process.cwd(), filePath) || filePath}: ${parsed.dryRun ? "would autofix" : "autofixed"}\n`,
          );
        }
      }
    }

    const cacheKey = relativeFilePath(filePath);
    const cacheEntry = cache?.files[cacheKey];
    const canReuse =
      useCache &&
      cacheEntry &&
      !affectedFiles.has(filePath) &&
      cacheEntry.hash === hashByFile.get(filePath);
    const findings = canReuse
      ? cacheEntry.findings
      : analyzeAllium(text, { mode: parsed.mode });
    if (useCache) {
      cache!.files[cacheKey] = {
        hash: hashByFile.get(filePath) ?? hashText(text),
        imports: importsByFile.get(filePath) ?? [],
        findings,
      };
    }
    for (const finding of findings) {
      allFindings.push({
        filePath,
        finding,
        fingerprint: findingFingerprint(filePath, finding),
      });
    }
  }
  if (useCache) {
    saveCache(parsed.cachePath, cache!);
  }

  if (parsed.writeBaselinePath) {
    writeBaseline(parsed.writeBaselinePath, allFindings);
    if (parsed.format === "text") {
      process.stdout.write(
        `Wrote baseline with ${allFindings.length} finding fingerprints to ${parsed.writeBaselinePath}\n`,
      );
    }
    const output =
      parsed.format === "text"
        ? `Wrote baseline with ${allFindings.length} finding fingerprints to ${parsed.writeBaselinePath}\n`
        : "";
    return {
      output,
      hasFailing: false,
      signature: `baseline:${allFindings.length}`,
      noFiles: false,
    };
  }

  const baselineFingerprints = parsed.baselinePath
    ? loadBaselineFingerprints(parsed.baselinePath)
    : new Set<string>();
  const filteredByBaseline = allFindings.filter(
    (record) => !baselineFingerprints.has(record.fingerprint),
  );
  const filteredByCode = filteredByBaseline.filter(
    (record) => !parsed.ignoreCodes.has(record.finding.code),
  );
  const filtered = filteredByCode.filter(
    (record) =>
      severityRank(record.finding.severity) >= severityRank(parsed.minSeverity),
  );
  const suppressedCount = allFindings.length - filteredByBaseline.length;

  let hasFailing = false;
  for (const record of filtered) {
    if (
      severityRank(record.finding.severity) >=
      severityRank(parsed.failOnSeverity)
    ) {
      hasFailing = true;
      break;
    }
  }

  const output = renderOutput(
    parsed.format,
    filtered,
    suppressedCount,
    parsed.showStats,
  );
  return {
    output,
    hasFailing,
    signature: JSON.stringify({
      filtered: filtered.map((record) => record.fingerprint),
      suppressedCount,
      hasFailing,
      noFiles: false,
    }),
    noFiles: false,
  };
}

function parseArgs(argv: string[]): ParsedArgs | null {
  let configPath = "allium.config.json";
  let useConfig = true;
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === "--config" && argv[i + 1]) {
      configPath = argv[i + 1];
      i += 1;
      continue;
    }
    if (argv[i] === "--no-config") {
      useConfig = false;
    }
  }
  const config = useConfig ? readAlliumConfig(configPath) : {};

  let mode: DiagnosticsMode = config.check?.mode ?? "strict";
  let autofix = false;
  let fixInteractive = false;
  let dryRun = false;
  let watch = false;
  let changedOnly = false;
  let showStats = false;
  let minSeverity: Finding["severity"] = config.check?.minSeverity ?? "info";
  let failOnSeverity: Finding["severity"] = config.check?.failOn ?? "warning";
  let format: CheckOutputFormat = "text";
  let reportPath: string | undefined;
  let cache = false;
  let cachePath = ".allium-check-cache.json";
  let baselinePath: string | undefined;
  let writeBaselinePath: string | undefined;
  const ignoreCodes = new Set(config.check?.ignoreCodes ?? []);
  const fixCodes = new Set(config.check?.fixCodes ?? []);
  const inputs: string[] = [...(config.project?.specPaths ?? [])];
  let resetInputs = false;

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--mode") {
      const modeArg = argv[i + 1];
      if (modeArg !== "strict" && modeArg !== "relaxed") {
        printUsage("Expected --mode strict|relaxed");
        return null;
      }
      mode = modeArg;
      i += 1;
      continue;
    }

    if (arg === "--autofix") {
      autofix = true;
      continue;
    }
    if (arg === "--fix-interactive") {
      fixInteractive = true;
      continue;
    }
    if (arg === "--dryrun" || arg === "--dry-run") {
      dryRun = true;
      continue;
    }
    if (arg === "--watch") {
      watch = true;
      continue;
    }
    if (arg === "--changed") {
      changedOnly = true;
      continue;
    }
    if (arg === "--stats") {
      showStats = true;
      continue;
    }
    if (arg === "--min-severity") {
      const next = argv[i + 1];
      if (next !== "info" && next !== "warning" && next !== "error") {
        printUsage("Expected --min-severity info|warning|error");
        return null;
      }
      minSeverity = next;
      i += 1;
      continue;
    }
    if (arg === "--ignore-code") {
      const next = argv[i + 1];
      if (!next) {
        printUsage("Expected a comma-separated list after --ignore-code");
        return null;
      }
      for (const value of next.split(",")) {
        const code = value.trim();
        if (code.length > 0) {
          ignoreCodes.add(code);
        }
      }
      i += 1;
      continue;
    }
    if (arg === "--fix-code") {
      const next = argv[i + 1];
      if (!next) {
        printUsage("Expected a comma-separated list after --fix-code");
        return null;
      }
      for (const value of next.split(",")) {
        const code = value.trim();
        if (code.length > 0) {
          fixCodes.add(code);
        }
      }
      i += 1;
      continue;
    }
    if (arg === "--fail-on") {
      const next = argv[i + 1];
      if (next !== "info" && next !== "warning" && next !== "error") {
        printUsage("Expected --fail-on info|warning|error");
        return null;
      }
      failOnSeverity = next;
      i += 1;
      continue;
    }
    if (arg === "--report") {
      const next = argv[i + 1];
      if (!next) {
        printUsage("Expected a path after --report");
        return null;
      }
      reportPath = next;
      i += 1;
      continue;
    }
    if (arg === "--cache") {
      cache = true;
      continue;
    }
    if (arg === "--cache-path") {
      const next = argv[i + 1];
      if (!next) {
        printUsage("Expected a path after --cache-path");
        return null;
      }
      cachePath = next;
      i += 1;
      continue;
    }

    if (arg === "--format") {
      const formatArg = argv[i + 1];
      if (
        formatArg !== "text" &&
        formatArg !== "json" &&
        formatArg !== "sarif"
      ) {
        printUsage("Expected --format text|json|sarif");
        return null;
      }
      format = formatArg;
      i += 1;
      continue;
    }

    if (arg === "--baseline") {
      const next = argv[i + 1];
      if (!next) {
        printUsage("Expected a path after --baseline");
        return null;
      }
      baselinePath = next;
      i += 1;
      continue;
    }

    if (arg === "--write-baseline") {
      const next = argv[i + 1];
      if (!next) {
        printUsage("Expected a path after --write-baseline");
        return null;
      }
      writeBaselinePath = next;
      i += 1;
      continue;
    }

    if (arg === "--help" || arg === "-h") {
      printUsage();
      return null;
    }
    if (arg === "--config") {
      i += 1;
      continue;
    }
    if (arg === "--no-config") {
      continue;
    }

    if (!resetInputs) {
      inputs.length = 0;
      resetInputs = true;
    }
    inputs.push(arg);
  }

  if (dryRun && !autofix) {
    printUsage("--dryrun requires --autofix");
    return null;
  }
  if (fixCodes.size > 0 && !autofix) {
    printUsage("--fix-code requires --autofix");
    return null;
  }
  if (fixInteractive && !autofix) {
    printUsage("--fix-interactive requires --autofix");
    return null;
  }

  if (inputs.length === 0 && !changedOnly) {
    printUsage("Provide at least one file, directory, or glob.");
    return null;
  }

  return {
    mode,
    autofix,
    fixInteractive,
    dryRun,
    watch,
    changedOnly,
    showStats,
    minSeverity,
    failOnSeverity,
    fixCodes,
    ignoreCodes,
    format,
    reportPath,
    cache,
    cachePath,
    baselinePath,
    writeBaselinePath,
    inputs,
  };
}

function printUsage(error?: string): void {
  if (error) {
    process.stderr.write(`${error}\n`);
  }
  process.stderr.write(
    "Usage: node dist/src/check.js [--config file|--no-config] [--mode strict|relaxed] [--autofix] [--fix-interactive] [--fix-code a,b] [--dryrun] [--changed] [--watch] [--stats] [--min-severity info|warning|error] [--fail-on info|warning|error] [--ignore-code a,b] [--format text|json|sarif] [--report file] [--cache] [--cache-path file] [--baseline file] [--write-baseline file] <file|directory|glob> [...]\n",
  );
}

function applyAutoFixes(
  text: string,
  mode: DiagnosticsMode,
  fixCodes: Set<string>,
): string {
  let current = text;
  for (let i = 0; i < 5; i += 1) {
    const findings = analyzeAllium(current, { mode });
    const edits = buildSafeEdits(
      buildSafeFixPlans(current, findings, fixCodes).map((entry) => ({
        offset: entry.offset,
        text: entry.text,
      })),
    );
    if (edits.length === 0) {
      break;
    }
    current = applyEdits(current, edits);
  }
  return current;
}

function applyAutoFixesInteractive(
  text: string,
  mode: DiagnosticsMode,
  fixCodes: Set<string>,
  displayPath: string,
): string {
  let current = text;
  const prompt = createInteractivePrompt();
  let applyAllRemaining = false;

  for (let i = 0; i < 5; i += 1) {
    const findings = analyzeAllium(current, { mode });
    const plans = buildSafeFixPlans(current, findings, fixCodes);
    if (plans.length === 0) {
      break;
    }
    const selected: TextEdit[] = [];
    for (const plan of plans) {
      if (applyAllRemaining) {
        selected.push({ offset: plan.offset, text: plan.text });
        continue;
      }
      process.stdout.write(
        `Apply fix ${plan.code} (${plan.label}) in ${displayPath}? [y]es/[n]o/[a]ll/[q]uit: `,
      );
      const answer = prompt().trim().toLowerCase();
      if (answer === "q") {
        return current;
      }
      if (answer === "a") {
        applyAllRemaining = true;
        selected.push({ offset: plan.offset, text: plan.text });
        continue;
      }
      if (answer === "y" || answer === "yes") {
        selected.push({ offset: plan.offset, text: plan.text });
      }
    }
    const edits = buildSafeEdits(selected);
    if (edits.length === 0) {
      break;
    }
    current = applyEdits(current, edits);
  }

  return current;
}

function buildSafeFixPlans(
  text: string,
  findings: Finding[],
  fixCodes: Set<string>,
): SafeFixPlan[] {
  const lineStarts = buildLineStarts(text);
  const edits = new Map<string, SafeFixPlan>();

  for (const finding of findings) {
    if (
      finding.code === "allium.rule.missingEnsures" &&
      (fixCodes.size === 0 || fixCodes.has(finding.code))
    ) {
      const lineStart = lineStarts[finding.start.line] ?? text.length;
      const key = `${lineStart}:ensures`;
      edits.set(key, {
        offset: lineStart,
        text: "    ensures: TODO()\n",
        code: finding.code,
        label: "insert ensures scaffold",
      });
    }

    if (
      finding.code === "allium.rule.missingWhen" &&
      (fixCodes.size === 0 || fixCodes.has(finding.code))
    ) {
      const insertLine = finding.start.line + 1;
      const lineStart = lineStarts[insertLine] ?? text.length;
      const key = `${lineStart}:when`;
      edits.set(key, {
        offset: lineStart,
        text: "    when: TODO()\n",
        code: finding.code,
        label: "insert when scaffold",
      });
    }

    if (
      finding.code === "allium.temporal.missingGuard" &&
      (fixCodes.size === 0 || fixCodes.has(finding.code))
    ) {
      // The finding spans from the rule's body start to the end of the
      // `when:` clause, so `end.line` is the `when:` line. Insert the guard on
      // the following line — clauses must appear after `when:`, and a guard
      // placed before it is a parse error.
      const whenLine = finding.end.line;
      const currentLineStart = lineStarts[whenLine] ?? 0;
      const nextLineStart = lineStarts[whenLine + 1] ?? text.length;
      const lineText = text.slice(
        currentLineStart,
        text.indexOf("\n", currentLineStart) >= 0
          ? text.indexOf("\n", currentLineStart)
          : text.length,
      );
      const indent = lineText.match(/^\s*/)?.[0] ?? "    ";
      const key = `${nextLineStart}:guard`;
      edits.set(key, {
        offset: nextLineStart,
        text: `${indent}requires: TODO() -- add temporal guard\n`,
        code: finding.code,
        label: "insert temporal requires guard",
      });
    }
  }

  return [...edits.values()].sort((a, b) => b.offset - a.offset);
}

function buildSafeEdits(edits: TextEdit[]): TextEdit[] {
  return [...edits].sort((a, b) => b.offset - a.offset);
}

function createInteractivePrompt(): () => string {
  if (!process.stdin.isTTY) {
    const scripted = fs.readFileSync(0, "utf8").split(/\r?\n/);
    let index = 0;
    return () => scripted[index++] ?? "";
  }
  return () => readLineSync();
}

function readLineSync(): string {
  const chars: number[] = [];
  const buf = Buffer.alloc(1);
  while (true) {
    const read = fs.readSync(0, buf, 0, 1, null);
    if (read === 0) {
      break;
    }
    const value = buf[0];
    if (value === 10) {
      break;
    }
    if (value !== 13) {
      chars.push(value);
    }
  }
  return Buffer.from(chars).toString("utf8");
}

function applyEdits(text: string, edits: TextEdit[]): string {
  let out = text;
  for (const edit of edits) {
    out = `${out.slice(0, edit.offset)}${edit.text}${out.slice(edit.offset)}`;
  }
  return out;
}

function buildLineStarts(text: string): number[] {
  const starts = [0];
  for (let i = 0; i < text.length; i += 1) {
    if (text[i] === "\n") {
      starts.push(i + 1);
    }
  }
  return starts;
}

function findingFingerprint(filePath: string, finding: Finding): string {
  const relPath = path.relative(process.cwd(), filePath) || filePath;
  return `${relPath}|${finding.start.line}|${finding.start.character}|${finding.code}|${finding.message}`;
}

function writeBaseline(outputPath: string, findings: FindingRecord[]): void {
  const fullPath = path.resolve(process.cwd(), outputPath);
  fs.mkdirSync(path.dirname(fullPath), { recursive: true });
  const unique = new Set(findings.map((record) => record.fingerprint));
  const baseline: BaselineFile = {
    version: 1,
    findings: [...unique].sort().map((fingerprint) => ({ fingerprint })),
  };
  fs.writeFileSync(fullPath, `${JSON.stringify(baseline, null, 2)}\n`, "utf8");
}

function loadBaselineFingerprints(filePath: string): Set<string> {
  const fullPath = path.resolve(process.cwd(), filePath);
  if (!fs.existsSync(fullPath)) {
    return new Set<string>();
  }
  try {
    const parsed = JSON.parse(
      fs.readFileSync(fullPath, "utf8"),
    ) as Partial<BaselineFile>;
    if (!parsed || parsed.version !== 1 || !Array.isArray(parsed.findings)) {
      return new Set<string>();
    }
    return new Set(
      parsed.findings
        .map((item) => item?.fingerprint)
        .filter((item): item is string => typeof item === "string"),
    );
  } catch {
    return new Set<string>();
  }
}

function renderOutput(
  format: CheckOutputFormat,
  findings: FindingRecord[],
  suppressedCount: number,
  showStats: boolean,
): string {
  if (format === "json") {
    return `${renderJson(findings, suppressedCount)}\n`;
  }
  if (format === "sarif") {
    return `${renderSarif(findings)}\n`;
  }
  let output = "";
  for (const record of findings) {
    output += `${formatFinding(record.filePath, record.finding)}\n`;
  }
  if (suppressedCount > 0) {
    output += `Suppressed ${suppressedCount} finding(s) from baseline.\n`;
  }
  if (showStats) {
    output += renderCodeStats(findings);
  }
  return output;
}

function renderJson(
  findings: FindingRecord[],
  suppressedCount: number,
): string {
  const errors = findings.filter(
    (record) => record.finding.severity === "error",
  ).length;
  const warnings = findings.filter(
    (record) => record.finding.severity === "warning",
  ).length;
  const infos = findings.filter(
    (record) => record.finding.severity === "info",
  ).length;
  return JSON.stringify(
    {
      summary: {
        findings: findings.length,
        errors,
        warnings,
        infos,
        suppressed: suppressedCount,
      },
      findings: findings.map((record) => ({
        file: path.relative(process.cwd(), record.filePath) || record.filePath,
        line: record.finding.start.line + 1,
        character: record.finding.start.character + 1,
        severity: record.finding.severity,
        code: record.finding.code,
        message: record.finding.message,
        fingerprint: record.fingerprint,
      })),
    },
    null,
    2,
  );
}

function renderSarif(findings: FindingRecord[]): string {
  const toLevel = (
    severity: Finding["severity"],
  ): "error" | "warning" | "note" => {
    if (severity === "error") {
      return "error";
    }
    if (severity === "warning") {
      return "warning";
    }
    return "note";
  };

  const sarif = {
    version: "2.1.0",
    $schema: "https://json.schemastore.org/sarif-2.1.0.json",
    runs: [
      {
        tool: {
          driver: {
            name: "allium-check",
            rules: uniqueRuleDescriptors(findings),
          },
        },
        results: findings.map((record) => ({
          ruleId: record.finding.code,
          level: toLevel(record.finding.severity),
          message: { text: record.finding.message },
          locations: [
            {
              physicalLocation: {
                artifactLocation: {
                  uri:
                    path.relative(process.cwd(), record.filePath) ||
                    record.filePath,
                },
                region: {
                  startLine: record.finding.start.line + 1,
                  startColumn: record.finding.start.character + 1,
                  endLine: record.finding.end.line + 1,
                  endColumn: record.finding.end.character + 1,
                },
              },
            },
          ],
        })),
      },
    ],
  };
  return JSON.stringify(sarif, null, 2);
}

function uniqueRuleDescriptors(findings: FindingRecord[]): Array<{
  id: string;
  shortDescription: { text: string };
  fullDescription: { text: string };
  helpUri: string;
  properties: { tags: string[]; precision: "high" };
}> {
  const descriptors = new Map<
    string,
    {
      id: string;
      shortDescription: { text: string };
      fullDescription: { text: string };
      helpUri: string;
      properties: { tags: string[]; precision: "high" };
    }
  >();
  for (const record of findings) {
    if (!descriptors.has(record.finding.code)) {
      const help = findingHelp(record.finding.code, record.finding.message);
      descriptors.set(record.finding.code, {
        id: record.finding.code,
        shortDescription: { text: help.summary },
        fullDescription: { text: help.remediation },
        helpUri: help.url,
        properties: { tags: ["allium", "spec"], precision: "high" },
      });
    }
  }
  return [...descriptors.values()];
}

function findingHelp(
  code: string,
  message: string,
): { summary: string; remediation: string; url: string } {
  const base = "https://juxt.github.io/allium/language";
  const known: Record<string, { summary: string; remediation: string }> = {
    "allium.rule.missingEnsures": {
      summary: "Rule is missing at least one ensures clause.",
      remediation:
        "Add one or more ensures statements describing post-conditions for the rule.",
    },
    "allium.temporal.missingGuard": {
      summary: "Temporal trigger should have a guard.",
      remediation:
        "Add a requires clause that prevents repeated firing for the same entity instance.",
    },
    "allium.import.undefinedSymbol": {
      summary: "Imported symbol does not exist in target spec.",
      remediation:
        "Declare the missing symbol in the imported spec or correct the aliased reference.",
    },
  };
  const entry = known[code];
  if (entry) {
    return { ...entry, url: `${base}#${code.replaceAll(".", "-")}` };
  }
  return {
    summary: message,
    remediation:
      "Review the referenced declaration and align it with Allium language constraints.",
    url: base,
  };
}

function resolveInputs(inputs: string[]): string[] {
  const files = new Set<string>();
  const cwd = process.cwd();
  let recursiveCache: string[] | null = null;

  for (const input of inputs) {
    const resolved = path.resolve(cwd, input);
    if (fs.existsSync(resolved)) {
      const stat = fs.statSync(resolved);
      if (stat.isDirectory()) {
        for (const filePath of walkAlliumFiles(resolved)) {
          files.add(filePath);
        }
      } else if (stat.isFile() && resolved.endsWith(".allium")) {
        files.add(resolved);
      }
      continue;
    }

    if (recursiveCache === null) {
      recursiveCache = walkAllFiles(cwd);
    }

    const matcher = wildcardToRegex(input);
    for (const candidate of recursiveCache) {
      const relative = path.relative(cwd, candidate).split(path.sep).join("/");
      if (matcher.test(relative) && candidate.endsWith(".allium")) {
        files.add(candidate);
      }
    }
  }

  return [...files].sort();
}

function walkAlliumFiles(root: string): string[] {
  return walkAllFiles(root).filter((entry) => entry.endsWith(".allium"));
}

function walkAllFiles(root: string): string[] {
  const out: string[] = [];
  const stack = [root];

  while (stack.length > 0) {
    const current = stack.pop();
    if (!current) {
      continue;
    }

    const entries = fs.readdirSync(current, { withFileTypes: true });
    for (const entry of entries) {
      const fullPath = path.join(current, entry.name);
      if (entry.isDirectory()) {
        stack.push(fullPath);
      } else if (entry.isFile()) {
        out.push(fullPath);
      }
    }
  }

  return out;
}

function wildcardToRegex(pattern: string): RegExp {
  const escaped = pattern
    .split(path.sep)
    .join("/")
    .replace(/[.+^${}()|[\]\\]/g, "\\$&")
    .replace(/\*/g, ".*")
    .replace(/\?/g, ".");

  return new RegExp(`^${escaped}$`);
}

function formatFinding(filePath: string, finding: Finding): string {
  const line = finding.start.line + 1;
  const character = finding.start.character + 1;
  const relPath = path.relative(process.cwd(), filePath) || filePath;
  return `${relPath}:${line}:${character} ${finding.severity} ${finding.code} ${finding.message}`;
}

function severityRank(severity: Finding["severity"]): number {
  if (severity === "info") {
    return 0;
  }
  if (severity === "warning") {
    return 1;
  }
  return 2;
}

function resolveChangedAlliumInputs(): string[] {
  const result = spawnSync(
    "git",
    ["status", "--porcelain", "--untracked-files=all"],
    {
      cwd: process.cwd(),
      encoding: "utf8",
    },
  );
  if (result.status !== 0) {
    return [];
  }
  const out: string[] = [];
  for (const rawLine of result.stdout.split("\n")) {
    const line = rawLine.trimEnd();
    if (line.length < 4) {
      continue;
    }
    const candidate = line.slice(3).trim();
    if (!candidate.endsWith(".allium")) {
      continue;
    }
    out.push(candidate);
  }
  return out;
}

function renderCodeStats(findings: FindingRecord[]): string {
  const counts = new Map<string, number>();
  for (const record of findings) {
    const current = counts.get(record.finding.code) ?? 0;
    counts.set(record.finding.code, current + 1);
  }
  if (counts.size === 0) {
    return "Stats: no findings.\n";
  }
  const ordered = [...counts.entries()].sort((a, b) => b[1] - a[1]);
  let output = "Stats by code:\n";
  for (const [code, count] of ordered) {
    output += `- ${code}: ${count}\n`;
  }
  return output;
}

function writeReport(reportPath: string, content: string): void {
  const fullPath = path.resolve(process.cwd(), reportPath);
  fs.mkdirSync(path.dirname(fullPath), { recursive: true });
  fs.writeFileSync(fullPath, content, "utf8");
}

function readAlliumConfig(configPath: string): AlliumConfig {
  const fullPath = path.resolve(process.cwd(), configPath);
  if (!fs.existsSync(fullPath)) {
    return {};
  }
  try {
    return JSON.parse(fs.readFileSync(fullPath, "utf8")) as AlliumConfig;
  } catch {
    return {};
  }
}

function loadCache(cachePath: string, mode: DiagnosticsMode): CacheFile {
  const fullPath = path.resolve(process.cwd(), cachePath);
  if (!fs.existsSync(fullPath)) {
    return { version: 1, mode, files: {} };
  }
  try {
    const parsed = JSON.parse(fs.readFileSync(fullPath, "utf8")) as CacheFile;
    if (parsed.version !== 1 || parsed.mode !== mode) {
      return { version: 1, mode, files: {} };
    }
    return parsed;
  } catch {
    return { version: 1, mode, files: {} };
  }
}

function saveCache(cachePath: string, cache: CacheFile): void {
  const fullPath = path.resolve(process.cwd(), cachePath);
  fs.mkdirSync(path.dirname(fullPath), { recursive: true });
  fs.writeFileSync(fullPath, `${JSON.stringify(cache, null, 2)}\n`, "utf8");
}

function hashText(text: string): string {
  return createHash("sha1").update(text).digest("hex");
}

function parseImports(filePath: string, text: string): string[] {
  const imports: string[] = [];
  const pattern = /^\s*use\s+"([^"]+)"\s+as\s+[A-Za-z_][A-Za-z0-9_]*\s*$/gm;
  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    imports.push(resolveImportPath(filePath, match[1]));
  }
  return imports.sort();
}

function resolveImportPath(
  currentFilePath: string,
  sourcePath: string,
): string {
  if (path.extname(sourcePath) !== ".allium") {
    return path.resolve(path.dirname(currentFilePath), `${sourcePath}.allium`);
  }
  return path.resolve(path.dirname(currentFilePath), sourcePath);
}

function collectAffectedFiles(
  files: string[],
  cache: CacheFile,
  hashByFile: Map<string, string>,
  importsByFile: Map<string, string[]>,
): Set<string> {
  const affected = new Set<string>();
  const normalizedFiles = new Set(files.map((file) => path.resolve(file)));
  const importersByTarget = new Map<string, Set<string>>();

  for (const filePath of files) {
    const resolvedFile = path.resolve(filePath);
    const imports = importsByFile.get(filePath) ?? [];
    for (const target of imports) {
      const resolvedTarget = path.resolve(target);
      if (!normalizedFiles.has(resolvedTarget)) {
        continue;
      }
      const importers =
        importersByTarget.get(resolvedTarget) ?? new Set<string>();
      importers.add(resolvedFile);
      importersByTarget.set(resolvedTarget, importers);
    }

    const cacheEntry = cache.files[relativeFilePath(filePath)];
    if (!cacheEntry || cacheEntry.hash !== hashByFile.get(filePath)) {
      affected.add(resolvedFile);
    }
  }

  const queue = [...affected];
  while (queue.length > 0) {
    const current = queue.pop();
    if (!current) {
      continue;
    }
    for (const importer of importersByTarget.get(current) ?? []) {
      if (affected.has(importer)) {
        continue;
      }
      affected.add(importer);
      queue.push(importer);
    }
  }

  return new Set(
    [...affected].filter((filePath) =>
      normalizedFiles.has(path.resolve(filePath)),
    ),
  );
}

function relativeFilePath(filePath: string): string {
  return path.relative(process.cwd(), filePath) || filePath;
}

const exitCode = main(process.argv.slice(2));
if (typeof exitCode === "number") {
  process.exitCode = exitCode;
}
