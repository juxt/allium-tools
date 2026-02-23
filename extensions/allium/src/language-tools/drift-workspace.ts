import * as fs from "node:fs";
import * as path from "node:path";

export interface WorkspaceAlliumConfig {
  project?: {
    specPaths?: string[];
    testPaths?: string[];
  };
  check?: { mode?: "strict" | "relaxed" };
  trace?: {
    tests?: string[];
    specs?: string[];
    testExtensions?: string[];
    testNamePatterns?: string[];
  };
  drift?: {
    sources?: string[];
    sourceExtensions?: string[];
    excludeDirs?: string[];
    diagnosticsFrom?: string;
    specs?: string[];
    commandsFrom?: string;
    skipCommands?: boolean;
  };
  scaffold?: {
    framework?: string;
  };
}

export function readWorkspaceAlliumConfig(
  workspaceRoot: string,
): WorkspaceAlliumConfig | undefined {
  const configFileName = ["allium", "config", "json"].join(".");
  const configPath = path.join(workspaceRoot, configFileName);
  if (!fs.existsSync(configPath)) {
    return undefined;
  }
  try {
    return JSON.parse(
      fs.readFileSync(configPath, "utf8"),
    ) as WorkspaceAlliumConfig;
  } catch {
    return undefined;
  }
}

export function collectWorkspaceFiles(
  workspaceRoot: string,
  inputs: string[],
  extensions: string[],
  excludeDirs: string[],
): string[] {
  const out = new Set<string>();
  const allowed = new Set(extensions.map((ext) => ext.toLowerCase()));
  const excluded = new Set(excludeDirs.filter((name) => name.length > 0));
  for (const input of inputs) {
    const resolved = path.isAbsolute(input)
      ? input
      : path.resolve(workspaceRoot, input);
    if (!fs.existsSync(resolved)) {
      continue;
    }
    const stat = fs.statSync(resolved);
    if (stat.isFile()) {
      if (allowed.has(path.extname(resolved).toLowerCase())) {
        out.add(resolved);
      }
      continue;
    }
    if (stat.isDirectory()) {
      for (const filePath of walkFiles(resolved, excluded)) {
        if (allowed.has(path.extname(filePath).toLowerCase())) {
          out.add(filePath);
        }
      }
    }
  }
  return [...out].sort();
}

function walkFiles(root: string, excludeDirs: ReadonlySet<string>): string[] {
  const out: string[] = [];
  const stack = [root];
  while (stack.length > 0) {
    const current = stack.pop();
    if (!current) {
      continue;
    }
    for (const entry of fs.readdirSync(current, { withFileTypes: true })) {
      const fullPath = path.join(current, entry.name);
      if (entry.isDirectory()) {
        if (excludeDirs.has(entry.name)) {
          continue;
        }
        stack.push(fullPath);
      } else if (entry.isFile()) {
        out.push(fullPath);
      }
    }
  }
  return out;
}

export function readDiagnosticsManifest(
  workspaceRoot: string,
  manifestPath: string,
): Set<string> {
  const resolved = path.isAbsolute(manifestPath)
    ? manifestPath
    : path.resolve(workspaceRoot, manifestPath);
  const payload = JSON.parse(fs.readFileSync(resolved, "utf8")) as
    | string[]
    | { diagnostics?: string[] };
  const diagnostics = Array.isArray(payload)
    ? payload
    : (payload.diagnostics ?? []);
  return new Set(
    diagnostics.filter((entry): entry is string => typeof entry === "string"),
  );
}

export function readCommandManifest(
  workspaceRoot: string,
  manifestPath: string,
): Set<string> {
  const resolved = path.isAbsolute(manifestPath)
    ? manifestPath
    : path.resolve(workspaceRoot, manifestPath);
  const payload = JSON.parse(fs.readFileSync(resolved, "utf8")) as
    | string[]
    | {
        contributes?: { commands?: Array<{ command?: string }> };
        commands?: string[];
        commandIds?: string[];
        command_names?: string[];
      };
  const commands = new Set<string>();
  if (Array.isArray(payload)) {
    for (const command of payload) {
      if (typeof command === "string") {
        commands.add(command);
      }
    }
    return commands;
  }
  for (const entry of payload.contributes?.commands ?? []) {
    if (typeof entry.command === "string") {
      commands.add(entry.command);
    }
  }
  for (const command of payload.commands ?? []) {
    commands.add(command);
  }
  for (const command of payload.commandIds ?? []) {
    commands.add(command);
  }
  for (const command of payload.command_names ?? []) {
    commands.add(command);
  }
  return commands;
}
