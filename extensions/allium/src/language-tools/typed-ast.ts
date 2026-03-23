import { parseAlliumBlocks } from "./parser";

export interface RuleAst {
  kind: "rule";
  name: string;
  startOffset: number;
  endOffset: number;
  when?: string;
  requires: string[];
  ensures: string[];
}

export interface EntityAst {
  kind: "entity";
  name: string;
  startOffset: number;
  endOffset: number;
}

export interface EnumAst {
  kind: "enum";
  name: string;
  startOffset: number;
  endOffset: number;
}

export type DeclarationAst = RuleAst | EntityAst | EnumAst;

export function parseDeclarationAst(text: string): DeclarationAst[] {
  const declarations: DeclarationAst[] = [];
  for (const block of parseAlliumBlocks(text)) {
    if (block.kind === "rule") {
      declarations.push({
        kind: "rule",
        name: block.name,
        startOffset: block.startOffset,
        endOffset: block.endOffset,
        when: firstClause(block.body, "when"),
        requires: collectClause(block.body, "requires"),
        ensures: collectClause(block.body, "ensures"),
      });
    } else if (block.kind === "entity") {
      declarations.push({
        kind: "entity",
        name: block.name,
        startOffset: block.startOffset,
        endOffset: block.endOffset,
      });
    } else if (block.kind === "enum") {
      declarations.push({
        kind: "enum",
        name: block.name,
        startOffset: block.startOffset,
        endOffset: block.endOffset,
      });
    }
  }
  return declarations.sort((a, b) => a.startOffset - b.startOffset);
}

function collectClause(body: string, clause: "requires" | "ensures"): string[] {
  const pattern = new RegExp(`^\\s*${clause}\\s*:\\s*(.+)$`, "gm");
  const out: string[] = [];
  for (let match = pattern.exec(body); match; match = pattern.exec(body)) {
    out.push(match[1].trim());
  }
  return out;
}

function firstClause(body: string, clause: "when"): string | undefined {
  const match = body.match(new RegExp(`^\\s*${clause}\\s*:\\s*(.+)$`, "m"));
  return match?.[1]?.trim();
}
