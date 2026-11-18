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
  // One frame per open brace. Within a frame, a line's depth comes from the
  // author's own indentation relative to the frame's first line: clause
  // bodies (`provides:`, `ensures:`, `for ...:`) and multi-line call
  // arguments are marked only by being indented further, so that relative
  // structure is kept and re-expressed in `indentWidth` steps (#114).
  const frames: BraceFrame[] = [{ level: 0, base: null, columns: [] }];

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

    const { leadingClosers, opens, trailingClosers } = scanBraces(trimmed);
    let lineLevel: number;
    if (leadingClosers > 0) {
      // A closing line aligns with the line that opened the brace.
      let closedLevel = frames[frames.length - 1].level;
      for (let i = 0; i < leadingClosers && frames.length > 1; i += 1) {
        closedLevel = frames.pop()!.level;
      }
      lineLevel = Math.max(closedLevel - 1, 0);
    } else {
      const frame = frames[frames.length - 1];
      const column = leadingColumn(rawLine, indentWidth);
      if (frame.base === null || column <= frame.base) {
        frame.base ??= column;
        frame.columns.length = 0;
      } else {
        while (
          frame.columns.length > 0 &&
          frame.columns[frame.columns.length - 1] > column
        ) {
          frame.columns.pop();
        }
        if (
          frame.columns.length === 0 ||
          frame.columns[frame.columns.length - 1] < column
        ) {
          frame.columns.push(column);
        }
      }
      lineLevel = frame.level + frame.columns.length;
    }

    const isTopLevelDeclaration =
      lineLevel === 0 && isTopLevelDeclarationLine(trimmed);
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

    const indent = " ".repeat(lineLevel * indentWidth);
    formattedLines.push(
      `${indent}${normalizePipeSpacing(normalizeDeclarationHeaderSpacing(trimmed))}`,
    );

    for (let i = 0; i < trailingClosers && frames.length > 1; i += 1) {
      frames.pop();
    }
    for (let i = 1; i <= opens; i += 1) {
      frames.push({ level: lineLevel + i, base: null, columns: [] });
    }
  }

  const withoutTrailingBlankLines = formattedLines
    .join("\n")
    .replace(/\n+$/g, "");
  return `${withoutTrailingBlankLines}\n`;
}

interface BraceFrame {
  /** Indentation level of the brace block's own lines. */
  level: number;
  /** The original column of the block's first line, once seen. */
  base: number | null;
  /** Original columns deeper than `base`, innermost last. */
  columns: number[];
}

function leadingColumn(line: string, tabWidth: number): number {
  let column = 0;
  for (const char of line) {
    if (char === " ") {
      column += 1;
    } else if (char === "\t") {
      column += tabWidth;
    } else {
      break;
    }
  }
  return column;
}

/**
 * Counts the braces on a line that change nesting, skipping those inside
 * string or backtick literals and in a trailing `--` comment (#114).
 * `leadingClosers` are the `}` before any other code; `opens` and
 * `trailingClosers` are what remains after the line, net of each other.
 */
function scanBraces(line: string): {
  leadingClosers: number;
  opens: number;
  trailingClosers: number;
} {
  let leadingClosers = 0;
  let depth = 0;
  let minDepth = 0;
  let sawCode = false;
  let quote: string | null = null;
  for (let i = 0; i < line.length; i += 1) {
    const char = line[i];
    if (quote !== null) {
      if (char === "\\" && quote === '"') {
        i += 1;
      } else if (char === quote) {
        quote = null;
      }
      continue;
    }
    if (char === "-" && line[i + 1] === "-") {
      break;
    }
    if (char === '"' || char === "`") {
      quote = char;
      sawCode = true;
      continue;
    }
    if (char === "}") {
      if (!sawCode) {
        leadingClosers += 1;
        continue;
      }
      depth -= 1;
      minDepth = Math.min(minDepth, depth);
    } else if (char === "{") {
      depth += 1;
    } else if (char !== " " && char !== "\t") {
      sawCode = true;
    }
    if (char === "{") {
      sawCode = true;
    }
  }
  const trailingClosers = -minDepth;
  return { leadingClosers, opens: depth - minDepth, trailingClosers };
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
