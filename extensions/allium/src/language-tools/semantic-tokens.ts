export const ALLIUM_SEMANTIC_TOKEN_TYPES = [
  "keyword",
  "class",
  "function",
  "namespace",
  "property",
  "comment",
  "string",
  "number",
] as const;

export interface SemanticTokenEntry {
  line: number;
  character: number;
  length: number;
  tokenType: (typeof ALLIUM_SEMANTIC_TOKEN_TYPES)[number];
}

export function collectSemanticTokenEntries(
  text: string,
): SemanticTokenEntry[] {
  const entries: SemanticTokenEntry[] = [];

  addMatches(entries, text, /^\s*--.*$/gm, "comment");
  addMatches(entries, text, /"([^"\\]|\\.)*"/g, "string");
  addMatches(entries, text, /\b\d+\b/g, "number");
  addDeclarationNameMatches(
    entries,
    text,
    /^\s*entity\s+([A-Za-z_][A-Za-z0-9_]*)\b/gm,
    "class",
  );
  addDeclarationNameMatches(
    entries,
    text,
    /^\s*external\s+entity\s+([A-Za-z_][A-Za-z0-9_]*)\b/gm,
    "class",
  );
  addDeclarationNameMatches(
    entries,
    text,
    /^\s*(rule|value|variant)\s+([A-Za-z_][A-Za-z0-9_]*)\b/gm,
    "function",
    2,
  );
  addDeclarationNameMatches(
    entries,
    text,
    /^\s*enum\s+([A-Za-z_][A-Za-z0-9_]*)\b/gm,
    "namespace",
  );
  addDeclarationNameMatches(
    entries,
    text,
    /^\s*(surface|actor|config)\s+([A-Za-z_][A-Za-z0-9_]*)\b/gm,
    "namespace",
    2,
  );
  addMatches(
    entries,
    text,
    /^\s*([A-Za-z_][A-Za-z0-9_]*)\s*:/gm,
    "property",
    1,
  );
  addMatches(
    entries,
    text,
    /\b(entity|external|value|variant|enum|given|rule|surface|actor|config|when|requires|ensures|let|facing|with|if|else|in|as|identified_by|exposes|provides|related|guidance|invariant|becomes|not|exists|and|or|default|deferred|open|module|use|transitions_to|guarantee|timeout|within|where|trigger|tags)\b/g,
    "keyword",
  );

  return sortAndDedupe(entries);
}

function addMatches(
  entries: SemanticTokenEntry[],
  text: string,
  pattern: RegExp,
  tokenType: SemanticTokenEntry["tokenType"],
  captureIndex = 0,
): void {
  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    const value = match[captureIndex];
    if (!value) {
      continue;
    }
    const within = match[0].indexOf(value);
    const absoluteOffset = match.index + Math.max(0, within);
    const position = offsetToLineCharacter(text, absoluteOffset);
    entries.push({
      line: position.line,
      character: position.character,
      length: value.length,
      tokenType,
    });
  }
}

function addDeclarationNameMatches(
  entries: SemanticTokenEntry[],
  text: string,
  pattern: RegExp,
  tokenType: SemanticTokenEntry["tokenType"],
  captureIndex = 1,
): void {
  addMatches(entries, text, pattern, tokenType, captureIndex);
}

function sortAndDedupe(entries: SemanticTokenEntry[]): SemanticTokenEntry[] {
  const sorted = [...entries].sort((a, b) => {
    if (a.line !== b.line) {
      return a.line - b.line;
    }
    if (a.character !== b.character) {
      return a.character - b.character;
    }
    return a.length - b.length;
  });

  const seen = new Set<string>();
  const out: SemanticTokenEntry[] = [];
  for (const entry of sorted) {
    const key = `${entry.line}:${entry.character}:${entry.length}:${entry.tokenType}`;
    if (seen.has(key)) {
      continue;
    }
    seen.add(key);
    out.push(entry);
  }
  return out;
}

function offsetToLineCharacter(
  text: string,
  offset: number,
): { line: number; character: number } {
  let line = 0;
  let character = 0;
  for (let i = 0; i < offset && i < text.length; i += 1) {
    if (text[i] === "\n") {
      line += 1;
      character = 0;
    } else {
      character += 1;
    }
  }
  return { line, character };
}
