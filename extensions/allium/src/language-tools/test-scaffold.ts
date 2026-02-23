import { parseDeclarationAst } from "./typed-ast";
import type { ScaffoldFramework } from "./test-scaffold-frameworks";

export function buildRuleTestScaffold(
  specText: string,
  moduleName: string,
  framework: ScaffoldFramework,
): string {
  const declarations = parseDeclarationAst(specText).filter(
    (entry) => entry.kind === "rule",
  );
  if (declarations.length === 0) {
    return "";
  }
  const lines: string[] = [];
  for (const line of framework.header) {
    lines.push(line);
  }
  if (framework.header.length > 0) {
    lines.push("");
  }
  for (const declaration of declarations) {
    for (const line of framework.testOpen(`${moduleName} / ${declaration.name}`).split("\n")) {
      lines.push(line);
    }
    if (declaration.when) {
      lines.push(
        `${framework.indent}${framework.comment(`trigger: ${declaration.when.replace(/\s+/g, " ").trim()}`)}`,
      );
    }
    for (const req of declaration.requires) {
      lines.push(`${framework.indent}${framework.comment(`requires: ${req}`)}`);
    }
    for (const ens of declaration.ensures) {
      lines.push(`${framework.indent}${framework.comment(`ensures: ${ens}`)}`);
    }
    lines.push(`${framework.indent}${framework.placeholder}`);
    if (framework.testClose) {
      lines.push(framework.testClose);
    }
    lines.push("");
  }
  return lines.join("\n");
}
