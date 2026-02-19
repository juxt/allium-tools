import * as path from "node:path";
import {
  buildDefinitionLookup,
  parseUseAliases,
  type DefinitionSite,
} from "./definitions";
import { findReferencesInText } from "./references";
import type { WorkspaceIndex } from "./workspace-index";

export interface WorkspaceRenameEdit {
  filePath: string;
  startOffset: number;
  endOffset: number;
}

export function planWorkspaceImportedRename(
  index: WorkspaceIndex,
  targetFilePath: string,
  definition: DefinitionSite,
  newName: string,
): { edits: WorkspaceRenameEdit[]; error?: string } {
  const targetDoc = index.documents.find(
    (doc) => path.resolve(doc.filePath) === path.resolve(targetFilePath),
  );
  if (!targetDoc) {
    return { edits: [] };
  }

  const lookup = buildDefinitionLookup(targetDoc.text);
  const existing = [...lookup.symbols, ...lookup.configKeys].find(
    (item) =>
      item.name === newName && item.startOffset !== definition.startOffset,
  );
  if (existing) {
    return {
      edits: [],
      error: `Rename would collide with existing ${existing.kind} '${newName}' in ${path.basename(targetFilePath)}.`,
    };
  }

  const edits: WorkspaceRenameEdit[] = [];
  const localRefs = findReferencesInText(targetDoc.text, definition);
  for (const reference of localRefs) {
    edits.push({
      filePath: targetDoc.filePath,
      startOffset: reference.startOffset,
      endOffset: reference.endOffset,
    });
  }

  for (const doc of index.documents) {
    const aliases = parseUseAliases(doc.text);
    for (const alias of aliases) {
      const resolved = resolveImportPath(doc.filePath, alias.sourcePath);
      if (path.resolve(resolved) !== path.resolve(targetFilePath)) {
        continue;
      }
      const pattern = new RegExp(
        `\\b${escapeRegex(alias.alias)}[\\/.](${escapeRegex(definition.name)})\\b`,
        "g",
      );
      for (
        let match = pattern.exec(doc.text);
        match;
        match = pattern.exec(doc.text)
      ) {
        const start = match.index + alias.alias.length + 1;
        edits.push({
          filePath: doc.filePath,
          startOffset: start,
          endOffset: start + definition.name.length,
        });
      }
    }
  }

  return { edits: dedupeEdits(edits) };
}

function dedupeEdits(edits: WorkspaceRenameEdit[]): WorkspaceRenameEdit[] {
  const out: WorkspaceRenameEdit[] = [];
  const seen = new Set<string>();
  for (const edit of edits) {
    const key = `${path.resolve(edit.filePath)}:${edit.startOffset}:${edit.endOffset}`;
    if (seen.has(key)) {
      continue;
    }
    seen.add(key);
    out.push(edit);
  }
  return out;
}

function resolveImportPath(
  currentFilePath: string,
  sourcePath: string,
): string {
  if (path.extname(sourcePath) !== ".allium") {
    return path.resolve(path.dirname(currentFilePath), `${sourcePath}.allium`);
  }
  return path.resolve(path.dirname(currentFilePath), sourcePath);
}

function escapeRegex(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
