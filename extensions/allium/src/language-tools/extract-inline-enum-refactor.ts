export interface RefactorEdit {
  startOffset: number;
  endOffset: number;
  text: string;
}

export interface ExtractInlineEnumPlan {
  title: string;
  edits: RefactorEdit[];
}

export function planExtractInlineEnumToNamedEnum(
  text: string,
  selectionStart: number,
): ExtractInlineEnumPlan | null {
  const line = lineForOffset(text, selectionStart);
  if (!line) {
    return null;
  }

  const match = line.text.match(
    /^(\s*)([A-Za-z_][A-Za-z0-9_]*)\s*:\s*([a-z_][a-z0-9_]*(?:\s*\|\s*[a-z_][a-z0-9_]*)+)\s*$/,
  );
  if (!match) {
    return null;
  }

  const indent = match[1];
  const fieldName = match[2];
  const variants = match[3]
    .split("|")
    .map((value) => value.trim())
    .filter((value) => value.length > 0);
  if (variants.length < 2) {
    return null;
  }

  const enumName = nextEnumName(text, toPascalCase(fieldName));
  const replacementLine = `${indent}${fieldName}: ${enumName}`;

  const insertionOffset = findEnumInsertionOffset(text);
  const enumBlock = `enum ${enumName} {\n    ${variants.join(" | ")}\n}\n\n`;

  return {
    title: "Extract inline enum to named enum",
    edits: [
      {
        startOffset: line.startOffset,
        endOffset: line.endOffset,
        text: replacementLine,
      },
      {
        startOffset: insertionOffset,
        endOffset: insertionOffset,
        text: enumBlock,
      },
    ],
  };
}

function lineForOffset(
  text: string,
  offset: number,
): { startOffset: number; endOffset: number; text: string } | null {
  if (offset < 0 || offset > text.length) {
    return null;
  }
  const startOffset = text.lastIndexOf("\n", Math.max(0, offset - 1)) + 1;
  const endIndex = text.indexOf("\n", offset);
  const endOffset = endIndex >= 0 ? endIndex : text.length;
  return { startOffset, endOffset, text: text.slice(startOffset, endOffset) };
}

function toPascalCase(name: string): string {
  return name
    .split(/[^A-Za-z0-9]+/)
    .filter((part) => part.length > 0)
    .map((part) => part[0].toUpperCase() + part.slice(1))
    .join("");
}

function nextEnumName(text: string, preferred: string): string {
  const existing = new Set<string>();
  const pattern = /^\s*enum\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{/gm;
  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    existing.add(match[1]);
  }
  if (!existing.has(preferred)) {
    return preferred;
  }
  let i = 2;
  while (existing.has(`${preferred}${i}`)) {
    i += 1;
  }
  return `${preferred}${i}`;
}

function findEnumInsertionOffset(text: string): number {
  const firstDeclaration =
    /^\s*(module|use|given|external\s+entity|entity|value|variant|enum|config|default|rule|surface|actor|deferred|open\s+question)\b/m.exec(
      text,
    );
  if (firstDeclaration) {
    return firstDeclaration.index;
  }
  return 0;
}
