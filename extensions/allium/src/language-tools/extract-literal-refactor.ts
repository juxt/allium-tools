export interface RefactorEdit {
  startOffset: number;
  endOffset: number;
  text: string;
}

export interface ExtractLiteralPlan {
  title: string;
  edits: RefactorEdit[];
}

export function planExtractLiteralToConfig(
  text: string,
  selectionStart: number,
  selectionEnd: number,
): ExtractLiteralPlan | null {
  if (selectionEnd <= selectionStart) {
    return null;
  }

  const selected = text.slice(selectionStart, selectionEnd).trim();
  const literalKind = classifyLiteral(selected);
  if (!literalKind) {
    return null;
  }

  const occurrences = findLiteralOccurrences(text, selected, literalKind);
  if (occurrences.length < 2) {
    return null;
  }

  const existingKeys = collectConfigKeys(text);
  const key = buildUniqueKey(selected, existingKeys);
  const reference = `config.${key}`;
  const typeName = literalKind === "string" ? "String" : "Integer";

  const edits: RefactorEdit[] = occurrences.map((startOffset) => ({
    startOffset,
    endOffset: startOffset + selected.length,
    text: reference,
  }));

  const configInsert = findConfigInsertion(text);
  if (configInsert) {
    edits.push({
      startOffset: configInsert.insertOffset,
      endOffset: configInsert.insertOffset,
      text: `${configInsert.indent}${key}: ${typeName} = ${selected}\n`,
    });
  } else {
    const insertOffset = findInsertAfterVersionMarker(text);
    edits.push({
      startOffset: insertOffset,
      endOffset: insertOffset,
      text: `config {\n    ${key}: ${typeName} = ${selected}\n}\n\n`,
    });
  }

  return {
    title: "Extract repeated literal to config",
    edits,
  };
}

function classifyLiteral(value: string): "string" | "integer" | null {
  if (/^"(?:[^"\\]|\\.)*"$/.test(value)) {
    return "string";
  }
  if (/^-?\d+$/.test(value)) {
    return "integer";
  }
  return null;
}

function findLiteralOccurrences(
  text: string,
  literal: string,
  kind: "string" | "integer",
): number[] {
  if (kind === "string") {
    const occurrences: number[] = [];
    let offset = text.indexOf(literal);
    while (offset >= 0) {
      occurrences.push(offset);
      offset = text.indexOf(literal, offset + literal.length);
    }
    return occurrences;
  }

  const escaped = escapeRegex(literal);
  const pattern = new RegExp(
    `(?<![A-Za-z0-9_\\.])${escaped}(?![A-Za-z0-9_\\.])`,
    "g",
  );
  const occurrences: number[] = [];
  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    occurrences.push(match.index);
  }
  return occurrences;
}

function collectConfigKeys(text: string): Set<string> {
  const keys = new Set<string>();
  const configPattern = /^\s*config\s*\{/gm;

  for (
    let match = configPattern.exec(text);
    match;
    match = configPattern.exec(text)
  ) {
    const openOffset = text.indexOf("{", match.index);
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
      let keyMatch = keyPattern.exec(body);
      keyMatch;
      keyMatch = keyPattern.exec(body)
    ) {
      keys.add(keyMatch[1]);
    }
  }

  return keys;
}

function buildUniqueKey(literal: string, existing: Set<string>): string {
  const normalized = literal
    .replace(/^"|"$/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^_+|_+$/g, "");

  const base = normalized
    ? `extracted_${normalized.slice(0, 20)}`
    : "extracted_value";

  if (!existing.has(base)) {
    return base;
  }

  let suffix = 2;
  while (existing.has(`${base}_${suffix}`)) {
    suffix += 1;
  }
  return `${base}_${suffix}`;
}

function findConfigInsertion(
  text: string,
): { insertOffset: number; indent: string } | null {
  const configPattern = /^\s*config\s*\{/gm;
  const match = configPattern.exec(text);
  if (!match) {
    return null;
  }

  const openOffset = text.indexOf("{", match.index);
  if (openOffset < 0) {
    return null;
  }

  const closeOffset = findMatchingBrace(text, openOffset);
  if (closeOffset < 0) {
    return null;
  }

  const lineStart = text.lastIndexOf("\n", closeOffset - 1) + 1;
  const closingLine = text.slice(lineStart, closeOffset);
  const indent = (closingLine.match(/^\s*/) ?? [""])[0];
  const keyIndent = `${indent}    `;

  return {
    insertOffset: closeOffset,
    indent: `\n${keyIndent}`,
  };
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

function findInsertAfterVersionMarker(text: string): number {
  const match = text.match(/^--\s*allium:\s*\d+[^\n]*\n/);
  return match ? match[0].length : 0;
}

function escapeRegex(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
