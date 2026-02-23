import { parseAlliumBlocks } from "./parser";

export type DiagramFormat = "d2" | "mermaid";

export type DiagramNodeKind =
  | "entity"
  | "value"
  | "variant"
  | "rule"
  | "surface"
  | "actor"
  | "enum"
  | "trigger";

export interface DiagramNode {
  id: string;
  key: string;
  label: string;
  kind: DiagramNodeKind;
  sourceOffset?: number;
}

export interface DiagramEdge {
  from: string;
  to: string;
  label: string;
  sourceOffset?: number;
}

export interface DiagramModel {
  nodes: DiagramNode[];
  edges: DiagramEdge[];
}

export interface DiagramIssue {
  code: "allium.diagram.skippedDeclaration";
  message: string;
  line: number;
}

export interface DiagramBuildResult {
  model: DiagramModel;
  issues: DiagramIssue[];
  modules: string[];
}

export interface DiagramFilterOptions {
  focusNames?: string[];
  kinds?: DiagramNodeKind[];
}

export function buildDiagramModel(text: string): DiagramModel {
  return buildDiagramResult(text).model;
}

export function buildDiagramResult(text: string): DiagramBuildResult {
  const nodes: DiagramNode[] = [];
  const edges: DiagramEdge[] = [];
  const issues: DiagramIssue[] = [];
  const nodeByKey = new Map<string, DiagramNode>();
  const lineStarts = buildLineStarts(text);

  const ensureNode = (
    kind: DiagramNodeKind,
    name: string,
    labelPrefix?: string,
    sourceOffset?: number,
  ): DiagramNode => {
    const key = `${kind}:${name}`;
    const existing = nodeByKey.get(key);
    if (existing) {
      if (existing.sourceOffset === undefined && sourceOffset !== undefined) {
        existing.sourceOffset = sourceOffset;
      }
      return existing;
    }
    const id = `${kind}_${name}`.replace(/[^A-Za-z0-9_]/g, "_");
    const label = labelPrefix ? `[${labelPrefix}] ${name}` : name;
    const node: DiagramNode = { id, key, label, kind, sourceOffset };
    nodeByKey.set(key, node);
    nodes.push(node);
    return node;
  };

  const addEdge = (
    from: DiagramNode,
    to: DiagramNode,
    label: string,
    sourceOffset?: number,
  ): void => {
    edges.push({ from: from.id, to: to.id, label, sourceOffset });
  };

  const blocks = parseAlliumBlocks(text);
  for (const block of blocks) {
    if (block.kind === "rule") {
      ensureNode("rule", block.name, "rule", block.startOffset);
    } else if (block.kind === "surface") {
      ensureNode("surface", block.name, "surface", block.startOffset);
    } else if (block.kind === "actor") {
      ensureNode("actor", block.name, "actor", block.startOffset);
    } else if (block.kind === "enum") {
      ensureNode("enum", block.name, "enum", block.startOffset);
    }
  }

  const topLevelPattern =
    /^\s*(external\s+entity|entity|value|variant)\s+([A-Za-z_][A-Za-z0-9_]*)(?:\s*:\s*([A-Za-z_][A-Za-z0-9_]*))?\s*\{/gm;
  for (
    let match = topLevelPattern.exec(text);
    match;
    match = topLevelPattern.exec(text)
  ) {
    const declKind = match[1];
    const name = match[2];
    const base = match[3];
    if (declKind === "value") {
      ensureNode("value", name, "value", match.index + match[0].indexOf(name));
      continue;
    }
    if (declKind === "variant") {
      const variant = ensureNode(
        "variant",
        name,
        "variant",
        match.index + match[0].indexOf(name),
      );
      if (base) {
        const baseEntity = ensureNode("entity", base, "entity");
        addEdge(variant, baseEntity, "extends");
      }
      continue;
    }
    ensureNode(
      "entity",
      name,
      declKind.startsWith("external") ? "external" : "entity",
      match.index + match[0].indexOf(name),
    );
  }

  const entityBlockPattern =
    /^\s*(?:external\s+)?entity\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{([\s\S]*?)^\s*\}/gm;
  for (
    let entity = entityBlockPattern.exec(text);
    entity;
    entity = entityBlockPattern.exec(text)
  ) {
    const source = ensureNode("entity", entity[1], "entity");
    const body = entity[2];
    const relPattern =
      /^\s*[A-Za-z_][A-Za-z0-9_]*\s*:\s*([A-Za-z_][A-Za-z0-9_]*(?:\/[A-Za-z_][A-Za-z0-9_]*)?)\s+for\s+this\s+[A-Za-z_][A-Za-z0-9_]*\s*$/gm;
    for (let rel = relPattern.exec(body); rel; rel = relPattern.exec(body)) {
      const targetType = rel[1].includes("/") ? rel[1].split("/")[1] : rel[1];
      const target = ensureNode("entity", targetType, "entity");
      addEdge(source, target, "rel");
    }
  }

  const rulePattern =
    /^\s*rule\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{([\s\S]*?)^\s*\}/gm;
  for (let rule = rulePattern.exec(text); rule; rule = rulePattern.exec(text)) {
    const ruleNode = ensureNode(
      "rule",
      rule[1],
      "rule",
      rule.index + rule[0].indexOf(rule[1]),
    );
    const body = rule[2];

    const when = body.match(/^\s*when\s*:\s*(.+)$/m);
    if (when) {
      const trigger = when[1].trim();
      const typed = trigger.match(
        /^[a-z_][a-z0-9_]*\s*:\s*([A-Za-z_][A-Za-z0-9_]*(?:\/[A-Za-z_][A-Za-z0-9_]*)?)\./,
      );
      if (typed) {
        const typeName = typed[1].includes("/")
          ? typed[1].split("/")[1]
          : typed[1];
        const entity = ensureNode("entity", typeName, "entity");
        addEdge(
          entity,
          ruleNode,
          "when",
          rule.index + rule[0].indexOf(when[0]),
        );
      }

      const callPattern = /([A-Za-z_][A-Za-z0-9_]*)\s*\(/g;
      for (
        let call = callPattern.exec(trigger);
        call;
        call = callPattern.exec(trigger)
      ) {
        const triggerNode = ensureNode("trigger", call[1], "trigger");
        addEdge(
          triggerNode,
          ruleNode,
          "when",
          rule.index + rule[0].indexOf(when[0]),
        );
      }
    }

    const createPattern =
      /\b([A-Za-z_][A-Za-z0-9_]*(?:\/[A-Za-z_][A-Za-z0-9_]*)?)\.created\s*\(/g;
    for (
      let create = createPattern.exec(body);
      create;
      create = createPattern.exec(body)
    ) {
      const raw = create[1];
      const typeName = raw.includes("/") ? raw.split("/")[1] : raw;
      const target = ensureNode("entity", typeName, "entity");
      addEdge(ruleNode, target, "ensures", rule.index + 1 + create.index);
    }
  }

  const surfacePattern =
    /^\s*surface\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{([\s\S]*?)^\s*\}/gm;
  for (
    let surface = surfacePattern.exec(text);
    surface;
    surface = surfacePattern.exec(text)
  ) {
    const surfaceNode = ensureNode(
      "surface",
      surface[1],
      "surface",
      surface.index + surface[0].indexOf(surface[1]),
    );
    const body = surface[2];

    const facingMatch = body.match(
      /^\s*facing\s+[A-Za-z_][A-Za-z0-9_]*\s*:\s*([A-Za-z_][A-Za-z0-9_]*)\s*$/m,
    );
    if (facingMatch) {
      const actor = ensureNode("actor", facingMatch[1], "actor");
      addEdge(
        actor,
        surfaceNode,
        "facing",
        surface.index + surface[0].indexOf(facingMatch[0]),
      );
    }

    const contextMatch = body.match(
      /^\s*context\s+[A-Za-z_][A-Za-z0-9_]*\s*:\s*([A-Za-z_][A-Za-z0-9_]*)\s*$/m,
    );
    if (contextMatch) {
      const contextEntity = ensureNode("entity", contextMatch[1], "entity");
      addEdge(
        contextEntity,
        surfaceNode,
        "context",
        surface.index + surface[0].indexOf(contextMatch[0]),
      );
    }

    for (const callName of parseSurfaceProvidesCalls(body)) {
      const triggerNode = ensureNode("trigger", callName, "trigger");
      addEdge(surfaceNode, triggerNode, "provides");
    }
  }

  for (const issue of collectSkippedDeclarationIssues(text, lineStarts)) {
    issues.push(issue);
  }

  const moduleNames = collectModuleNames(text);

  return {
    model: normalizeModel({ nodes, edges }),
    issues,
    modules: moduleNames,
  };
}

export function applyDiagramFilters(
  model: DiagramModel,
  options: DiagramFilterOptions,
): DiagramModel {
  const requestedKinds = new Set(options.kinds ?? []);
  const kindFilteredNodes =
    requestedKinds.size > 0
      ? model.nodes.filter((node) => requestedKinds.has(node.kind))
      : [...model.nodes];

  const nodeById = new Map(kindFilteredNodes.map((node) => [node.id, node]));
  const kindFilteredEdges = model.edges.filter(
    (edge) => nodeById.has(edge.from) && nodeById.has(edge.to),
  );

  const focusNames = (options.focusNames ?? [])
    .map((value) => value.trim().toLowerCase())
    .filter((value) => value.length > 0);
  if (focusNames.length === 0) {
    return normalizeModel({
      nodes: kindFilteredNodes,
      edges: kindFilteredEdges,
    });
  }

  const focusMatches = new Set<string>();
  for (const node of kindFilteredNodes) {
    const target = `${node.key} ${node.label}`.toLowerCase();
    if (focusNames.some((focus) => target.includes(focus))) {
      focusMatches.add(node.id);
    }
  }

  if (focusMatches.size === 0) {
    return { nodes: [], edges: [] };
  }

  const visible = new Set<string>(focusMatches);
  for (const edge of kindFilteredEdges) {
    if (focusMatches.has(edge.from) || focusMatches.has(edge.to)) {
      visible.add(edge.from);
      visible.add(edge.to);
    }
  }

  return normalizeModel({
    nodes: kindFilteredNodes.filter((node) => visible.has(node.id)),
    edges: kindFilteredEdges.filter(
      (edge) => visible.has(edge.from) && visible.has(edge.to),
    ),
  });
}

export function renderDiagram(
  model: DiagramModel,
  format: DiagramFormat,
): string {
  if (format === "mermaid") {
    return renderMermaid(model);
  }
  return renderD2(model);
}

function renderD2(model: DiagramModel): string {
  const lines: string[] = ["direction: right", ""];
  const nodesByKind = groupNodesByKind(model.nodes);
  for (const [kind, nodes] of nodesByKind) {
    lines.push(`${kind}_group: {`);
    lines.push(`  label: "${escapeD2(kindLabel(kind))}"`);
    lines.push("  style: {");
    lines.push('    stroke: "#7b8794"');
    lines.push('    fill: "#f8fafc"');
    lines.push("  }");
    for (const node of nodes) {
      lines.push(`  ${node.id}: "${escapeD2(node.label)}"`);
    }
    lines.push("}");
    lines.push("");
  }

  for (const edge of model.edges) {
    lines.push(`${edge.from} -> ${edge.to}: "${escapeD2(edge.label)}"`);
  }
  return `${lines.join("\n").replace(/\n+$/g, "")}\n`;
}

function renderMermaid(model: DiagramModel): string {
  const lines: string[] = ["flowchart LR"];
  const nodesByKind = groupNodesByKind(model.nodes);
  for (const [kind, nodes] of nodesByKind) {
    lines.push(`  subgraph ${kind}_group["${escapeMermaid(kindLabel(kind))}"]`);
    for (const node of nodes) {
      lines.push(`    ${node.id}["${escapeMermaid(node.label)}"]`);
    }
    lines.push("  end");
  }
  for (const edge of model.edges) {
    lines.push(`  ${edge.from} -->|${escapeMermaid(edge.label)}| ${edge.to}`);
  }
  return `${lines.join("\n")}\n`;
}

function groupNodesByKind(
  nodes: DiagramNode[],
): Map<DiagramNodeKind, DiagramNode[]> {
  const grouped = new Map<DiagramNodeKind, DiagramNode[]>();
  const order: DiagramNodeKind[] = [
    "entity",
    "variant",
    "value",
    "enum",
    "rule",
    "surface",
    "actor",
    "trigger",
  ];
  for (const kind of order) {
    grouped.set(kind, []);
  }
  for (const node of nodes) {
    grouped.get(node.kind)?.push(node);
  }

  const sortedEntries: Array<[DiagramNodeKind, DiagramNode[]]> = [];
  for (const [kind, items] of grouped.entries()) {
    const sorted = [...items].sort((a, b) => a.id.localeCompare(b.id));
    if (sorted.length > 0) {
      sortedEntries.push([kind, sorted]);
    }
  }
  return new Map(sortedEntries);
}

function kindLabel(kind: DiagramNodeKind): string {
  if (kind === "entity") {
    return "Entities";
  }
  if (kind === "variant") {
    return "Variants";
  }
  if (kind === "value") {
    return "Values";
  }
  if (kind === "enum") {
    return "Enums";
  }
  if (kind === "rule") {
    return "Rules";
  }
  if (kind === "surface") {
    return "Surfaces";
  }
  if (kind === "actor") {
    return "Actors";
  }
  return "Triggers";
}

function normalizeModel(model: DiagramModel): DiagramModel {
  const uniqueNodes = new Map<string, DiagramNode>();
  for (const node of model.nodes) {
    uniqueNodes.set(node.id, node);
  }

  const uniqueEdges = new Map<string, DiagramEdge>();
  for (const edge of model.edges) {
    uniqueEdges.set(`${edge.from}|${edge.to}|${edge.label}`, edge);
  }

  return {
    nodes: [...uniqueNodes.values()].sort((a, b) => a.id.localeCompare(b.id)),
    edges: [...uniqueEdges.values()].sort((a, b) => {
      const aKey = `${a.from}|${a.to}|${a.label}`;
      const bKey = `${b.from}|${b.to}|${b.label}`;
      return aKey.localeCompare(bKey);
    }),
  };
}

function collectModuleNames(text: string): string[] {
  const modules = new Set<string>();
  const pattern = /^\s*module\s+([a-z_][a-z0-9_]*)\b/gm;
  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    modules.add(match[1]);
  }
  return [...modules].sort();
}

function collectSkippedDeclarationIssues(
  text: string,
  lineStarts: number[],
): DiagramIssue[] {
  const issues: DiagramIssue[] = [];
  const pattern =
    /^\s*(default|deferred|open\s+question|given|config|use)\b[^\n]*$/gm;
  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    if (isCommentLineAtIndex(text, match.index)) {
      continue;
    }
    issues.push({
      code: "allium.diagram.skippedDeclaration",
      line: offsetToLine(lineStarts, match.index),
      message: `Diagram extraction skipped '${match[1]}' declaration at line ${offsetToLine(lineStarts, match.index) + 1}.`,
    });
  }
  return issues;
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

function offsetToLine(lineStarts: number[], offset: number): number {
  let line = 0;
  for (let i = 0; i < lineStarts.length; i += 1) {
    if (lineStarts[i] > offset) {
      break;
    }
    line = i;
  }
  return line;
}

function isCommentLineAtIndex(text: string, index: number): boolean {
  const lineStart = text.lastIndexOf("\n", index) + 1;
  const lineEnd = text.indexOf("\n", index);
  const line = text.slice(lineStart, lineEnd >= 0 ? lineEnd : text.length);
  return /^\s*--/.test(line);
}

function escapeD2(value: string): string {
  return value.replace(/\\/g, "\\\\").replace(/"/g, '\\"');
}

function escapeMermaid(value: string): string {
  return value.replace(/"/g, "'");
}

function parseSurfaceProvidesCalls(body: string): string[] {
  const calls: string[] = [];
  const sectionPattern = /^(\s*)provides\s*:\s*$/gm;
  for (
    let section = sectionPattern.exec(body);
    section;
    section = sectionPattern.exec(body)
  ) {
    const baseIndent = (section[1] ?? "").length;
    let cursor = section.index + section[0].length + 1;
    while (cursor < body.length) {
      const lineEnd = body.indexOf("\n", cursor);
      const end = lineEnd >= 0 ? lineEnd : body.length;
      const line = body.slice(cursor, end);
      const trimmed = line.trim();
      const indent = (line.match(/^\s*/) ?? [""])[0].length;
      if (trimmed.length === 0) {
        cursor = end + 1;
        continue;
      }
      if (indent <= baseIndent) {
        break;
      }
      const match = line.match(/([A-Za-z_][A-Za-z0-9_]*)\s*\(/);
      if (match) {
        calls.push(match[1]);
      }
      cursor = end + 1;
    }
  }
  return calls;
}
