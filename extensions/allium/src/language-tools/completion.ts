import { buildDefinitionLookup } from "./definitions";

export interface CompletionCandidate {
  label: string;
  kind: "keyword" | "property";
}

const KEYWORDS = [
  "entity",
  "external",
  "value",
  "variant",
  "enum",
  "given",
  "rule",
  "surface",
  "actor",
  "config",
  "when",
  "requires",
  "ensures",
  "let",
  "facing",
  "with",
  "if",
  "else",
  "in",
  "and",
  "or",
  "not",
  "exists",
  "becomes",
  "identified_by",
  "exposes",
  "provides",
  "related",
  "guidance",
  "invariant",
  "default",
  "deferred",
  "open question",
  "module",
  "use",
  "as",
  "transitions_to",
  "guarantee",
  "timeout",
  "within",
  "where",
  "trigger",
  "tags",
  "transitions",
  "terminal",
  "for",
  "implies",
  "contract",
  "contracts",
  "demands",
  "fulfils",
];

export function collectCompletionCandidates(
  text: string,
  offset: number,
): CompletionCandidate[] {
  const keywordCandidates = KEYWORDS.map((label) => ({
    label,
    kind: "keyword" as const,
  }));

  if (!isConfigReferenceContext(text, offset)) {
    return keywordCandidates;
  }

  const configKeys = buildDefinitionLookup(text).configKeys.map((entry) => ({
    label: entry.name,
    kind: "property" as const,
  }));
  return dedupeByLabel([...configKeys, ...keywordCandidates]);
}

function isConfigReferenceContext(text: string, offset: number): boolean {
  if (offset < 0 || offset > text.length) {
    return false;
  }
  const prefix = text.slice(Math.max(0, offset - "config.".length), offset);
  return prefix === "config.";
}

function dedupeByLabel<T extends { label: string }>(values: T[]): T[] {
  const seen = new Set<string>();
  const out: T[] = [];
  for (const value of values) {
    if (seen.has(value.label)) {
      continue;
    }
    seen.add(value.label);
    out.push(value);
  }
  return out;
}
