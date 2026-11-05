#!/usr/bin/env node
import * as fs from "node:fs";
import * as path from "node:path";

interface ParsedArgs {
  checkOnly: boolean;
  dryRun: boolean;
  readFromStdin: boolean;
  writeToStdout: boolean;
  indentWidth: number;
  topLevelSpacing: number;
  inputs: string[];
}

interface AlliumConfig {
  project?: {
    specPaths?: string[];
  };
  format?: {
    indentWidth?: number;
    topLevelSpacing?: number;
  };
}

function main(argv: string[]): number {
  const parsed = parseArgs(argv);
  if (!parsed) {
    return 2;
  }

  if (parsed.readFromStdin) {
    const original = fs.readFileSync(0, "utf8");
    const formatted = formatAlliumText(original, {
      indentWidth: parsed.indentWidth,
      topLevelSpacing: parsed.topLevelSpacing,
    });
    const changed = formatted !== original;
    if (parsed.writeToStdout || !parsed.checkOnly) {
      process.stdout.write(formatted);
    }
    if (parsed.checkOnly && changed) {
      process.stderr.write("stdin: would format\n");
      return 1;
    }
    return 0;
  }

  const files = resolveInputs(parsed.inputs);
  if (files.length === 0) {
    process.stderr.write("No .allium files found for the provided inputs.\n");
    return 2;
  }

  let changed = 0;
  for (const filePath of files) {
    const original = fs.readFileSync(filePath, "utf8");
    const formatted = formatAlliumText(original, {
      indentWidth: parsed.indentWidth,
      topLevelSpacing: parsed.topLevelSpacing,
    });
    if (formatted === original) {
      continue;
    }

    changed += 1;
    const relPath = path.relative(process.cwd(), filePath) || filePath;
    if (parsed.checkOnly) {
      process.stdout.write(`${relPath}: would format\n`);
    } else if (parsed.dryRun || parsed.writeToStdout) {
      process.stdout.write(`--- ${relPath} (formatted preview)\n`);
      process.stdout.write(formatted);
      if (!formatted.endsWith("\n")) {
        process.stdout.write("\n");
      }
    } else {
      fs.writeFileSync(filePath, formatted, "utf8");
      process.stdout.write(`${relPath}: formatted\n`);
    }
  }

  if (parsed.checkOnly) {
    if (changed > 0) {
      process.stderr.write(
        `${changed} file(s) need formatting. Run allium-format without --check.\n`,
      );
      return 1;
    }
    process.stdout.write("All files already formatted.\n");
    return 0;
  }

  process.stdout.write(
    changed > 0
      ? `Formatted ${changed} file(s).\n`
      : "No formatting changes needed.\n",
  );
  return 0;
}

export interface FormatOptions {
  indentWidth?: number;
  topLevelSpacing?: number;
}

export function formatAlliumText(
  text: string,
  options: FormatOptions = {},
): string {
  const indentWidth = clampInteger(options.indentWidth ?? 4, 1, 8);
  const topLevelSpacing = clampInteger(options.topLevelSpacing ?? 1, 0, 3);
  const normalized = text.replace(/\r\n?/g, "\n");
  const lines = normalized
    .split("\n")
    .map((line) => line.replace(/[ \t]+$/g, ""));

  const formattedLines: string[] = [];
  // Depth of unclosed `{` and `(` before the current line.
  let bracketDepth = 0;
  // Indented blocks that have no brackets: the body of a clause header such
  // as `provides:` or `if x:`, and continuation lines inside that body. Each
  // entry adds one level. It records the input indentation of the line that
  // opened it and the bracket depth it lives at; it closes at the first line
  // at that depth that is not indented deeper than the opener.
  const softBlocks: { openerIndent: number; bracketDepth: number }[] = [];
  let previous: {
    indent: number;
    code: string;
    bracketDepthAfter: number;
  } | null = null;

  for (const rawLine of lines) {
    const trimmed = rawLine.trim();

    if (trimmed.length === 0) {
      if (
        formattedLines.length > 0 &&
        formattedLines[formattedLines.length - 1] !== ""
      ) {
        formattedLines.push("");
      }
      continue;
    }

    const inputIndent = measureIndent(rawLine, indentWidth);
    const code = stripStringsAndComments(trimmed);
    const leadingClosers = countLeadingClosers(code);
    bracketDepth = Math.max(bracketDepth - leadingClosers, 0);

    while (
      softBlocks.length > 0 &&
      softBlocks[softBlocks.length - 1].bracketDepth > bracketDepth
    ) {
      softBlocks.pop();
    }
    while (
      softBlocks.length > 0 &&
      softBlocks[softBlocks.length - 1].bracketDepth === bracketDepth &&
      inputIndent <= softBlocks[softBlocks.length - 1].openerIndent
    ) {
      softBlocks.pop();
    }

    if (
      previous !== null &&
      leadingClosers === 0 &&
      inputIndent > previous.indent &&
      previous.bracketDepthAfter === bracketDepth
    ) {
      const opensClauseBody = previous.code.trimEnd().endsWith(":");
      const continuesSoftBlock =
        softBlocks.length > 0 &&
        softBlocks[softBlocks.length - 1].bracketDepth === bracketDepth;
      if (opensClauseBody || continuesSoftBlock) {
        softBlocks.push({ openerIndent: previous.indent, bracketDepth });
      }
    }

    const indentLevel = bracketDepth + softBlocks.length;

    const isTopLevelDeclaration =
      indentLevel === 0 && isTopLevelDeclarationLine(trimmed);
    const prevNonBlank = lastNonBlankLine(formattedLines);
    if (
      isTopLevelDeclaration &&
      formattedLines.length > 0 &&
      blankLinesAtEnd(formattedLines) < topLevelSpacing &&
      prevNonBlank !== null &&
      !prevNonBlank.trimStart().startsWith("--")
    ) {
      while (blankLinesAtEnd(formattedLines) < topLevelSpacing) {
        formattedLines.push("");
      }
    }

    const indent = " ".repeat(indentLevel * indentWidth);
    formattedLines.push(
      `${indent}${normalizePipeSpacing(normalizeDeclarationHeaderSpacing(trimmed))}`,
    );

    const openCount = countOccurrences(code, "{") + countOccurrences(code, "(");
    const closeCount =
      countOccurrences(code, "}") + countOccurrences(code, ")");
    const trailingCloseCount = Math.max(closeCount - leadingClosers, 0);
    bracketDepth = Math.max(bracketDepth + openCount - trailingCloseCount, 0);
    previous = { indent: inputIndent, code, bracketDepthAfter: bracketDepth };
  }

  const withoutTrailingBlankLines = formattedLines
    .join("\n")
    .replace(/\n+$/g, "");
  return `${withoutTrailingBlankLines}\n`;
}

function countOccurrences(text: string, token: string): number {
  return text.split(token).length - 1;
}

function countLeadingClosers(code: string): number {
  let count = 0;
  for (const char of code) {
    if (char === "}" || char === ")") {
      count += 1;
      continue;
    }
    if (char === " " || char === "\t") {
      continue;
    }
    break;
  }
  return count;
}

function measureIndent(line: string, tabWidth: number): number {
  let width = 0;
  for (const char of line) {
    if (char === " ") {
      width += 1;
    } else if (char === "\t") {
      width += tabWidth;
    } else {
      break;
    }
  }
  return width;
}

// Returns the line with string literal contents and any `--` comment removed,
// so brackets and colons in them do not affect indentation.
function stripStringsAndComments(line: string): string {
  let out = "";
  let inString = false;
  for (let i = 0; i < line.length; i += 1) {
    const char = line[i];
    if (inString) {
      if (char === "\\") {
        i += 1;
      } else if (char === '"') {
        inString = false;
        out += char;
      }
      continue;
    }
    if (char === '"') {
      inString = true;
      out += char;
      continue;
    }
    if (char === "-" && line[i + 1] === "-") {
      break;
    }
    out += char;
  }
  return out;
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
  const inputs: string[] = [...(config.project?.specPaths ?? [])];
  let resetInputs = false;
  let checkOnly = false;
  let dryRun = false;
  let readFromStdin = false;
  let writeToStdout = false;
  let indentWidth = config.format?.indentWidth ?? 4;
  let topLevelSpacing = config.format?.topLevelSpacing ?? 1;

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--check") {
      checkOnly = true;
      continue;
    }
    if (arg === "--dryrun" || arg === "--dry-run") {
      dryRun = true;
      continue;
    }
    if (arg === "--stdin") {
      readFromStdin = true;
      continue;
    }
    if (arg === "--stdout") {
      writeToStdout = true;
      continue;
    }
    if (arg === "--indent-width") {
      const value = Number(argv[i + 1]);
      if (!Number.isInteger(value) || value < 1 || value > 8) {
        printUsage("Expected --indent-width <1-8>");
        return null;
      }
      indentWidth = value;
      i += 1;
      continue;
    }
    if (arg === "--top-level-spacing") {
      const value = Number(argv[i + 1]);
      if (!Number.isInteger(value) || value < 0 || value > 3) {
        printUsage("Expected --top-level-spacing <0-3>");
        return null;
      }
      topLevelSpacing = value;
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

  if (inputs.length === 0) {
    if (readFromStdin) {
      return {
        checkOnly,
        dryRun,
        readFromStdin,
        writeToStdout,
        indentWidth,
        topLevelSpacing,
        inputs: [],
      };
    }
    printUsage("Provide at least one file, directory, or glob.");
    return null;
  }

  return {
    checkOnly,
    dryRun,
    readFromStdin,
    writeToStdout,
    indentWidth,
    topLevelSpacing,
    inputs,
  };
}

function printUsage(error?: string): void {
  if (error) {
    process.stderr.write(`${error}\n`);
  }
  process.stderr.write(
    "Usage: node dist/src/format.js [--config file|--no-config] [--check] [--dryrun] [--stdin --stdout] [--stdout] [--indent-width N] [--top-level-spacing N] <file|directory|glob> [...]\n",
  );
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

function lastNonBlankLine(lines: string[]): string | null {
  for (let i = lines.length - 1; i >= 0; i -= 1) {
    if (lines[i] !== "") {
      return lines[i];
    }
  }
  return null;
}

function blankLinesAtEnd(lines: string[]): number {
  let count = 0;
  for (let i = lines.length - 1; i >= 0; i -= 1) {
    if (lines[i] !== "") {
      break;
    }
    count += 1;
  }
  return count;
}

function clampInteger(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, Math.floor(value)));
}

function isTopLevelDeclarationLine(line: string): boolean {
  return /^(entity|external\s+entity|value|variant|rule|surface|actor|config|enum|default|module|given|deferred|open\s+question)\b/.test(
    line,
  );
}

function normalizeDeclarationHeaderSpacing(line: string): string {
  if (line.startsWith("--")) {
    return line;
  }
  return line.replace(
    /^((?:entity|external\s+entity|value|variant|rule|surface|actor|config|enum|default|module|given|deferred|open\s+question)\b[^{]*?)\s*\{$/,
    "$1 {",
  );
}

function normalizePipeSpacing(line: string): string {
  const trimmed = line.trim();
  if (trimmed.startsWith("--") || !line.includes("|")) {
    return line;
  }
  return line.replace(/\s*\|\s*/g, " | ");
}

if (require.main === module) {
  const exitCode = main(process.argv.slice(2));
  process.exitCode = exitCode;
}
