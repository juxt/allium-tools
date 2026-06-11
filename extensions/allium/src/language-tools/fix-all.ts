import { analyzeAllium, type DiagnosticsMode } from "./analyzer";

export type FixCategory = "all" | "missingEnsures" | "temporalGuards";

export interface FixEdit {
  startOffset: number;
  endOffset: number;
  text: string;
}

export function planSafeFixesByCategory(
  text: string,
  mode: DiagnosticsMode,
  category: FixCategory,
): FixEdit[] {
  const findings = analyzeAllium(text, { mode });
  const lineStarts = buildLineStarts(text);
  const edits = new Map<string, FixEdit>();

  for (const finding of findings) {
    if (
      (category === "all" || category === "missingEnsures") &&
      finding.code === "allium.rule.missingEnsures"
    ) {
      const offset = lineStarts[finding.start.line] ?? text.length;
      edits.set(`ensure:${offset}`, {
        startOffset: offset,
        endOffset: offset,
        text: "    ensures: TODO()\n",
      });
    }

    if (
      (category === "all" || category === "temporalGuards") &&
      finding.code === "allium.temporal.missingGuard"
    ) {
      // `end.line` is the `when:` clause line; insert the guard after it.
      // Clauses must follow `when:`, so a guard placed before is invalid.
      const whenStart = lineStarts[finding.end.line] ?? 0;
      const insertOffset = lineStarts[finding.end.line + 1] ?? text.length;
      const lineText = text.slice(
        whenStart,
        text.indexOf("\n", whenStart) >= 0
          ? text.indexOf("\n", whenStart)
          : text.length,
      );
      const indent = lineText.match(/^\s*/)?.[0] ?? "    ";
      edits.set(`guard:${insertOffset}`, {
        startOffset: insertOffset,
        endOffset: insertOffset,
        text: `${indent}requires: TODO() -- add temporal guard\n`,
      });
    }
  }

  return [...edits.values()].sort((a, b) => b.startOffset - a.startOffset);
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
