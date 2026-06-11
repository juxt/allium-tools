export interface TemporalGuardEdit {
  startOffset: number;
  endOffset: number;
  text: string;
}

export interface InsertTemporalGuardPlan {
  title: string;
  edit: TemporalGuardEdit;
}

interface RuleRange {
  bodyStart: number;
  bodyEnd: number;
}

export function planInsertTemporalGuard(
  text: string,
  selectionStart: number,
): InsertTemporalGuardPlan | null {
  const line = lineForOffset(text, selectionStart);
  if (
    !line ||
    !/^\s*when\s*:/.test(line.text) ||
    !isTemporalWhenClause(line.text)
  ) {
    return null;
  }

  const rule = findContainingRule(text, line.startOffset);
  if (!rule) {
    return null;
  }

  const ruleBody = text.slice(rule.bodyStart, rule.bodyEnd);
  if (/^\s*requires\s*:/m.test(ruleBody)) {
    return null;
  }

  const indent = (line.text.match(/^\s*/) ?? [""])[0];
  const insertOffset = line.endOffset + 1;

  return {
    title: "Add temporal requires guard",
    edit: {
      startOffset: insertOffset,
      endOffset: insertOffset,
      text: `${indent}requires: TODO() -- add temporal guard\n`,
    },
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
  return {
    startOffset,
    endOffset,
    text: text.slice(startOffset, endOffset),
  };
}

function findContainingRule(text: string, offset: number): RuleRange | null {
  const rulePattern = /\brule\s+[A-Za-z_][A-Za-z0-9_]*\s*\{/g;

  for (
    let match = rulePattern.exec(text);
    match;
    match = rulePattern.exec(text)
  ) {
    const openOffset = text.indexOf("{", match.index);
    if (openOffset < 0) {
      continue;
    }

    const closeOffset = findMatchingBrace(text, openOffset);
    if (closeOffset < 0) {
      continue;
    }

    if (offset > openOffset && offset < closeOffset) {
      return { bodyStart: openOffset + 1, bodyEnd: closeOffset };
    }
  }

  return null;
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

function isTemporalWhenClause(line: string): boolean {
  if (/:[^\n]*(<=|>=|<|>)\s*now\b/.test(line)) {
    return true;
  }
  if (/\bnow\s*[+-]\s*\d/.test(line)) {
    return true;
  }
  return false;
}
