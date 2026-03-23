const HOVER_DOCS: Record<string, string> = {
  entity: "Defines a persisted domain concept with fields and derived values.",
  rule: "Defines a behavior: trigger (`when`), preconditions (`requires`), and outcomes (`ensures`).",
  when: "Trigger clause that starts a rule.",
  requires: "Precondition clause that must hold before a rule can apply.",
  ensures: "Outcome clause that must hold after a rule applies.",
  config:
    "Declares reusable configuration values referenced as `config.<key>`.",
  surface:
    "Defines an actor-facing projection with context, exposed fields, and capabilities.",
  actor: "Defines a principal interacting with one or more surfaces.",
  open:
    "Marks unresolved product or domain questions inside the specification.",
  deferred: "Declares behavior that is intentionally deferred to another spec.",
  given: "Declares module-level entity bindings available to all rules.",
  facing: "Binds an actor to a surface.",
  transitions_to:
    "Trigger clause: fires when a field transitions to a specific value.",
  guarantee: "Declares a guarantee constraint on a surface.",
  timeout: "Declares a timeout constraint on a surface.",
  within: "Declares a containment relationship for an actor.",
  transitions:
    "Declares valid lifecycle transitions for a status field on an entity.",
  terminal:
    "Declares terminal states within a transitions block. Terminal states have no outbound transitions.",
  implies:
    "Boolean operator for logical implication. `a implies b` is equivalent to `not a or b`.",
  contract:
    "Declares a named contract with signatures and invariants that surfaces can demand or fulfil.",
  demands:
    "Declares that a surface requires a contract to be fulfilled by its environment.",
  fulfils:
    "Declares that a surface provides an implementation of a contract.",
};

export function hoverTextAtOffset(text: string, offset: number): string | null {
  const token = tokenAtOffset(text, offset);
  if (!token) {
    return null;
  }

  return HOVER_DOCS[token] ?? null;
}

export function findLeadingDocComment(
  text: string,
  declarationStartOffset: number,
): string | null {
  const lineStart = text.lastIndexOf("\n", declarationStartOffset - 1) + 1;
  let cursor = lineStart - 1;
  const commentLines: string[] = [];

  while (cursor >= 0) {
    const previousLineEnd = cursor;
    const previousLineStart = text.lastIndexOf("\n", previousLineEnd - 1) + 1;
    const line = text.slice(previousLineStart, previousLineEnd).trimEnd();
    if (line.trim().length === 0) {
      if (commentLines.length === 0) {
        cursor = previousLineStart - 1;
        continue;
      }
      break;
    }
    if (!/^\s*--/.test(line)) {
      break;
    }
    commentLines.push(line.replace(/^\s*--\s?/, ""));
    cursor = previousLineStart - 1;
  }

  if (commentLines.length === 0) {
    return null;
  }
  return commentLines.reverse().join("\n").trim();
}

function tokenAtOffset(text: string, offset: number): string | null {
  if (offset < 0 || offset >= text.length) {
    return null;
  }

  const isIdent = (char: string | undefined): boolean =>
    !!char && /[A-Za-z_]/.test(char);
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
  return text.slice(start, end);
}
