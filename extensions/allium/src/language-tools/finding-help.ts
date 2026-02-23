export interface FindingHelp {
  title: string;
  summary: string;
  why: string;
  howToFix: string;
  url: string;
}

const BASE_URL = "https://juxt.github.io/allium/language";

const HELP_BY_CODE: Record<string, Omit<FindingHelp, "url">> = {
  "allium.rule.missingEnsures": {
    title: "Missing ensures clause",
    summary: "Rules should declare at least one post-condition using ensures.",
    why: "Without ensures, rule effects are unspecified and hard to validate.",
    howToFix:
      "Add one or more ensures lines describing the state change this rule guarantees.",
  },
  "allium.temporal.missingGuard": {
    title: "Temporal trigger without guard",
    summary: "Temporal triggers should include a requires guard.",
    why: "Ungarded temporal triggers can repeatedly fire for the same entity.",
    howToFix:
      "Add a requires line that constrains when the temporal rule may execute.",
  },
  "allium.import.undefinedSymbol": {
    title: "Undefined imported symbol",
    summary: "An aliased imported symbol was not found in the target spec.",
    why: "Cross-spec references must resolve to declared definitions.",
    howToFix:
      "Correct the symbol name or declare the missing symbol in the imported specification.",
  },
  "allium.openQuestion.present": {
    title: "Open question present",
    summary: "The spec contains an open question marker.",
    why: "Open questions indicate incomplete or unresolved behavior.",
    howToFix:
      "Resolve the question and replace it with concrete specification language.",
  },
};

export function explainFinding(code: string, message: string): FindingHelp {
  const known = HELP_BY_CODE[code];
  if (known) {
    return {
      ...known,
      url: `${BASE_URL}#${code.replaceAll(".", "-")}`,
    };
  }
  return {
    title: code,
    summary: message,
    why: "This diagnostic reports a specification issue detected by allium-check.",
    howToFix:
      "Inspect the reported location, adjust the spec text, and rerun checks.",
    url: BASE_URL,
  };
}

export function buildFindingExplanationMarkdown(
  code: string,
  message: string,
): string {
  const help = explainFinding(code, message);
  return [
    `# ${help.title}`,
    "",
    `- Code: \`${code}\``,
    "",
    `## Summary`,
    help.summary,
    "",
    `## Why`,
    help.why,
    "",
    `## How To Fix`,
    help.howToFix,
    "",
    `## Reference`,
    help.url,
    "",
  ].join("\n");
}
