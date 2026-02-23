export interface ParsedBlock {
  kind: "rule" | "given" | "config" | "surface" | "actor" | "enum" | "use";
  name: string;
  nameStartOffset: number;
  startOffset: number;
  bodyStartOffset: number;
  endOffset: number;
  body: string;
  sourcePath?: string;
  alias?: string;
}

export function parseAlliumBlocks(text: string): ParsedBlock[] {
  const blocks: ParsedBlock[] = [];
  blocks.push(
    ...findNamedBraceBlocks(
      text,
      /^\s*rule\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{/gm,
      "rule",
    ),
  );
  blocks.push(
    ...findNamedBraceBlocks(
      text,
      /^\s*enum\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{/gm,
      "enum",
    ),
  );
  blocks.push(
    ...findNamedBraceBlocks(text, /^\s*given\s*\{/gm, "given", "given"),
  );
  blocks.push(
    ...findNamedBraceBlocks(text, /^\s*config\s*\{/gm, "config", "config"),
  );
  blocks.push(
    ...findNamedBraceBlocks(
      text,
      /^\s*surface\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{/gm,
      "surface",
    ),
  );
  blocks.push(
    ...findNamedBraceBlocks(
      text,
      /^\s*actor\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{/gm,
      "actor",
    ),
  );
  blocks.push(...findUseStatements(text));
  return blocks.sort((a, b) => a.startOffset - b.startOffset);
}

export function findMatchingBrace(text: string, openOffset: number): number {
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

function findNamedBraceBlocks(
  text: string,
  startPattern: RegExp,
  kind: ParsedBlock["kind"],
  defaultName = "",
): ParsedBlock[] {
  const blocks: ParsedBlock[] = [];
  for (
    let match = startPattern.exec(text);
    match;
    match = startPattern.exec(text)
  ) {
    const braceOffset = text.indexOf("{", match.index);
    if (braceOffset < 0) {
      continue;
    }
    const endOffset = findMatchingBrace(text, braceOffset);
    if (endOffset < 0) {
      continue;
    }
    blocks.push({
      kind,
      name: match[1] ?? defaultName,
      nameStartOffset:
        match.index + (match[1] ? match[0].indexOf(match[1]) : 0),
      startOffset: match.index,
      bodyStartOffset: braceOffset + 1,
      endOffset,
      body: text.slice(braceOffset + 1, endOffset),
    });
  }
  return blocks;
}

function findUseStatements(text: string): ParsedBlock[] {
  const uses: ParsedBlock[] = [];
  const pattern = /^\s*use\s+"([^"]+)"\s+as\s+([A-Za-z_][A-Za-z0-9_]*)\s*$/gm;
  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    uses.push({
      kind: "use",
      name: match[2],
      nameStartOffset: match.index + match[0].indexOf(match[2]),
      alias: match[2],
      sourcePath: match[1],
      startOffset: match.index,
      bodyStartOffset: match.index,
      endOffset: match.index + match[0].length,
      body: "",
    });
  }
  return uses;
}
