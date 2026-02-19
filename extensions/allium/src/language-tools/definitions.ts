export interface DefinitionSite {
  name: string;
  kind:
    | "entity"
    | "external_entity"
    | "value"
    | "variant"
    | "enum"
    | "default_instance"
    | "rule"
    | "surface"
    | "actor"
    | "config_key";
  startOffset: number;
  endOffset: number;
}

export interface DefinitionLookup {
  symbols: DefinitionSite[];
  configKeys: DefinitionSite[];
}

export function buildDefinitionLookup(text: string): DefinitionLookup {
  return {
    symbols: collectNamedDefinitions(text),
    configKeys: collectConfigKeys(text),
  };
}

export function findDefinitionsAtOffset(
  text: string,
  offset: number,
): DefinitionSite[] {
  const token = tokenAtOffset(text, offset);
  if (!token) {
    return [];
  }

  const lookup = buildDefinitionLookup(text);
  if (token.kind === "configKey") {
    return lookup.configKeys.filter((entry) => entry.name === token.name);
  }
  return lookup.symbols.filter((entry) => entry.name === token.name);
}

function collectNamedDefinitions(text: string): DefinitionSite[] {
  const patterns: Array<{
    pattern: RegExp;
    kind: DefinitionSite["kind"];
  }> = [
    { pattern: /^\s*entity\s+([A-Za-z_][A-Za-z0-9_]*)\b/gm, kind: "entity" },
    {
      pattern: /^\s*external\s+entity\s+([A-Za-z_][A-Za-z0-9_]*)\b/gm,
      kind: "external_entity",
    },
    { pattern: /^\s*value\s+([A-Za-z_][A-Za-z0-9_]*)\b/gm, kind: "value" },
    { pattern: /^\s*variant\s+([A-Za-z_][A-Za-z0-9_]*)\b/gm, kind: "variant" },
    { pattern: /^\s*enum\s+([A-Za-z_][A-Za-z0-9_]*)\b/gm, kind: "enum" },
    { pattern: /^\s*rule\s+([A-Za-z_][A-Za-z0-9_]*)\b/gm, kind: "rule" },
    { pattern: /^\s*surface\s+([A-Za-z_][A-Za-z0-9_]*)\b/gm, kind: "surface" },
    { pattern: /^\s*actor\s+([A-Za-z_][A-Za-z0-9_]*)\b/gm, kind: "actor" },
  ];

  const out: DefinitionSite[] = [];
  for (const { pattern, kind } of patterns) {
    for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
      const name = match[1];
      const startOffset = match.index + match[0].indexOf(name);
      out.push({
        name,
        kind,
        startOffset,
        endOffset: startOffset + name.length,
      });
    }
  }
  const defaultPattern =
    /^\s*default\s+([A-Za-z_][A-Za-z0-9_]*)(?:\s+([A-Za-z_][A-Za-z0-9_]*))?\s*=/gm;
  for (
    let match = defaultPattern.exec(text);
    match;
    match = defaultPattern.exec(text)
  ) {
    const name = match[2] ?? match[1];
    const startOffset = match.index + match[0].indexOf(name);
    out.push({
      name,
      kind: "default_instance",
      startOffset,
      endOffset: startOffset + name.length,
    });
  }
  return out;
}

function collectConfigKeys(text: string): DefinitionSite[] {
  const out: DefinitionSite[] = [];
  const blockPattern = /^\s*config\s*\{/gm;

  for (
    let block = blockPattern.exec(text);
    block;
    block = blockPattern.exec(text)
  ) {
    const openOffset = text.indexOf("{", block.index);
    if (openOffset < 0) {
      continue;
    }
    const closeOffset = findMatchingBrace(text, openOffset);
    if (closeOffset < 0) {
      continue;
    }

    const body = text.slice(openOffset + 1, closeOffset);
    const keyPattern = /^\s*([A-Za-z_][A-Za-z0-9_]*)\s*:/gm;
    for (
      let match = keyPattern.exec(body);
      match;
      match = keyPattern.exec(body)
    ) {
      const name = match[1];
      const startOffset = openOffset + 1 + match.index + match[0].indexOf(name);
      out.push({
        name,
        kind: "config_key",
        startOffset,
        endOffset: startOffset + name.length,
      });
    }
  }

  return out;
}

export function tokenAtOffset(
  text: string,
  offset: number,
): { name: string; kind: "symbol" | "configKey" } | null {
  if (offset < 0 || offset >= text.length) {
    return null;
  }

  const isIdent = (char: string | undefined): boolean =>
    !!char && /[A-Za-z0-9_]/.test(char);
  let start = offset;
  while (start > 0 && isIdent(text[start - 1])) {
    start -= 1;
  }
  let end = offset;
  while (end < text.length && isIdent(text[end])) {
    end += 1;
  }
  if (start === end) {
    return null;
  }

  const name = text.slice(start, end);
  const prefixStart = start - "config.".length;
  if (prefixStart >= 0 && text.slice(prefixStart, start) === "config.") {
    return { name, kind: "configKey" };
  }
  return { name, kind: "symbol" };
}

export interface UseAlias {
  alias: string;
  sourcePath: string;
}

export function parseUseAliases(text: string): UseAlias[] {
  const aliases: UseAlias[] = [];
  const pattern = /^\s*use\s+"([^"]+)"\s+as\s+([A-Za-z_][A-Za-z0-9_]*)\s*$/gm;
  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    aliases.push({ sourcePath: match[1], alias: match[2] });
  }
  return aliases;
}

export function importedSymbolAtOffset(
  text: string,
  offset: number,
): { alias: string; symbol: string } | null {
  if (offset < 0 || offset >= text.length) {
    return null;
  }

  const isIdent = (char: string | undefined): boolean =>
    !!char && /[A-Za-z0-9_]/.test(char);
  let start = offset;
  while (start > 0 && isIdent(text[start - 1])) {
    start -= 1;
  }
  let end = offset;
  while (end < text.length && isIdent(text[end])) {
    end += 1;
  }
  if (start === end) {
    return null;
  }
  const symbol = text.slice(start, end);
  const separatorIndex = start - 1;
  if (separatorIndex < 1) {
    return null;
  }
  const separator = text[separatorIndex];
  if (separator !== "/" && separator !== ".") {
    return null;
  }

  const aliasEnd = separatorIndex;
  let aliasStart = aliasEnd;
  while (aliasStart > 0 && isIdent(text[aliasStart - 1])) {
    aliasStart -= 1;
  }
  if (aliasStart === aliasEnd) {
    return null;
  }
  const alias = text.slice(aliasStart, aliasEnd);
  return { alias, symbol };
}

function findMatchingBrace(text: string, openOffset: number): number {
  let depth = 0;
  for (let i = openOffset; i < text.length; i += 1) {
    const char = text[i];
    if (char === "{") {
      depth += 1;
    } else if (char === "}") {
      depth -= 1;
      if (depth === 0) {
        return i;
      }
    }
  }
  return -1;
}
