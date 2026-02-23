import {
  createConnection,
  TextDocuments,
  ProposedFeatures,
  TextDocumentSyncKind,
  DiagnosticSeverity,
  SymbolKind,
  CompletionItemKind,
  FoldingRangeKind,
  CodeActionKind,
  Range,
  type InitializeParams,
  type InitializeResult,
  type Hover,
  type Location,
  type CompletionList,
  type CompletionItem,
  type DocumentSymbol,
  type WorkspaceSymbol,
  type CodeAction,
  type WorkspaceEdit,
  type FoldingRange,
  type SemanticTokens,
  type SemanticTokensParams,
  type CodeLens,
  type DocumentLink,
  type TextEdit,
  type Diagnostic,
  type Position,
  ResponseError,
  ErrorCodes,
} from "vscode-languageserver/node";
import { TextDocument } from "vscode-languageserver-textdocument";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as path from "node:path";
import * as fs from "node:fs";

import { analyzeAllium } from "../../../extensions/allium/src/language-tools/analyzer";
import {
  hoverTextAtOffset,
  findLeadingDocComment,
} from "../../../extensions/allium/src/language-tools/hover";
import {
  findDefinitionsAtOffset,
  parseUseAliases,
  type DefinitionSite,
} from "../../../extensions/allium/src/language-tools/definitions";
import { findReferencesInText } from "../../../extensions/allium/src/language-tools/references";
import {
  collectAlliumSymbols,
  type AlliumSymbolType,
} from "../../../extensions/allium/src/language-tools/outline";
import { collectWorkspaceSymbolRecords } from "../../../extensions/allium/src/language-tools/workspace-symbols";
import { collectCompletionCandidates } from "../../../extensions/allium/src/language-tools/completion";
import { planExtractLiteralToConfig } from "../../../extensions/allium/src/language-tools/extract-literal-refactor";
import { planExtractInlineEnumToNamedEnum } from "../../../extensions/allium/src/language-tools/extract-inline-enum-refactor";
import { planInsertTemporalGuard } from "../../../extensions/allium/src/language-tools/insert-temporal-guard-refactor";
import {
  planSafeFixesByCategory,
} from "../../../extensions/allium/src/language-tools/fix-all";
import {
  renderSimulationMarkdown,
  simulateRuleAtOffset,
} from "../../../extensions/allium/src/language-tools/rule-sim";
import {
  buildDiagramResult,
  renderDiagram,
  type DiagramFormat,
  type DiagramModel,
} from "../../../extensions/allium/src/language-tools/diagram";
import { buildRuleTestScaffold } from "../../../extensions/allium/src/language-tools/test-scaffold";
import {
  removeStaleSuppressions,
  buildSuppressionDirectiveEdit,
} from "../../../extensions/allium/src/language-tools/suppression";
import { buildFindingExplanationMarkdown } from "../../../extensions/allium/src/language-tools/finding-help";
import {
  buildDriftReport,
  extractAlliumDiagnosticCodes,
  extractSpecCommands,
  extractSpecDiagnosticCodes,
  renderDriftMarkdown,
} from "../../../extensions/allium/src/language-tools/spec-drift";
import {
  collectWorkspaceFiles,
  readCommandManifest,
  readDiagnosticsManifest,
  readWorkspaceAlliumConfig,
} from "../../../extensions/allium/src/language-tools/drift-workspace";
import {
  buildTestFileMatcher,
  resolveTestDiscoveryOptions,
} from "../../../extensions/allium/src/language-tools/test-discovery";
import {
  prepareRenameTarget,
  planRename,
} from "../../../extensions/allium/src/language-tools/rename";
import { planWorkspaceImportedRename } from "../../../extensions/allium/src/language-tools/cross-file-rename";
import { getFramework, frameworkIds } from "../../../extensions/allium/src/language-tools/test-scaffold-frameworks";
import { formatAlliumText } from "../../../extensions/allium/src/format";
import { collectTopLevelFoldingBlocks } from "../../../extensions/allium/src/language-tools/folding";
import {
  collectSemanticTokenEntries,
  ALLIUM_SEMANTIC_TOKEN_TYPES,
} from "../../../extensions/allium/src/language-tools/semantic-tokens";
import { collectCodeLensTargets } from "../../../extensions/allium/src/language-tools/codelens";
import { collectUseImportPaths } from "../../../extensions/allium/src/language-tools/document-links";
import {
  buildWorkspaceIndex,
  resolveImportedDefinition,
  type WorkspaceIndex,
} from "../../../extensions/allium/src/language-tools/workspace-index";

// ---------------------------------------------------------------------------
// Connection + document store
// ---------------------------------------------------------------------------

const connection = createConnection(ProposedFeatures.all);
const documents = new TextDocuments(TextDocument);

// ---------------------------------------------------------------------------
// Workspace state
// ---------------------------------------------------------------------------

let workspaceRoot: string | null = null;
let workspaceIndex: WorkspaceIndex = { documents: [] };

function refreshWorkspaceIndex(): void {
  if (!workspaceRoot) return;
  try {
    workspaceIndex = buildWorkspaceIndex(workspaceRoot);
  } catch {
    // Non-fatal: cross-file features degrade gracefully to single-file mode
  }
}

// ---------------------------------------------------------------------------
// Coordinate helpers
// ---------------------------------------------------------------------------

export function offsetToPosition(text: string, offset: number): Position {
  const before = text.slice(0, offset);
  const lines = before.split("\n");
  return {
    line: lines.length - 1,
    character: lines[lines.length - 1].length,
  };
}

export function positionToOffset(text: string, position: Position): number {
  const lines = text.split("\n");
  let offset = 0;
  for (let i = 0; i < position.line && i < lines.length; i++) {
    offset += lines[i].length + 1; // +1 for \n
  }
  offset += Math.min(position.character, (lines[position.line] ?? "").length);
  return offset;
}

function tokenBoundsAtOffset(
  text: string,
  offset: number,
): { startOffset: number; endOffset: number } | null {
  if (offset < 0 || offset >= text.length) {
    return null;
  }
  const isIdent = (char: string | undefined): boolean =>
    !!char && /[A-Za-z0-9_]/.test(char);
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
  return { startOffset: start, endOffset: end };
}

function uriToPath(uri: string): string {
  try {
    return fileURLToPath(uri);
  } catch {
    return uri.replace(/^file:\/\//, "");
  }
}

function pathToUri(filePath: string): string {
  return pathToFileURL(filePath).toString();
}

function resolveImportPath(currentFilePath: string, sourcePath: string): string {
  if (path.extname(sourcePath) !== ".allium") {
    return path.resolve(path.dirname(currentFilePath), `${sourcePath}.allium`);
  }
  return path.resolve(path.dirname(currentFilePath), sourcePath);
}

function escapeRegex(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

// ---------------------------------------------------------------------------
// SymbolKind mappings
// ---------------------------------------------------------------------------

function definitionKindToSymbolKind(kind: DefinitionSite["kind"]): SymbolKind {
  switch (kind) {
    case "entity":
      return SymbolKind.Class;
    case "external_entity":
      return SymbolKind.Interface;
    case "value":
      return SymbolKind.Constant;
    case "variant":
      return SymbolKind.EnumMember;
    case "enum":
      return SymbolKind.Enum;
    case "default_instance":
      return SymbolKind.Object;
    case "rule":
      return SymbolKind.Function;
    case "surface":
      return SymbolKind.Interface;
    case "actor":
      return SymbolKind.Class;
    case "config_key":
      return SymbolKind.Property;
  }
}

function alliumSymbolTypeToSymbolKind(type: AlliumSymbolType): SymbolKind {
  switch (type) {
    case "entity":
      return SymbolKind.Class;
    case "external entity":
      return SymbolKind.Interface;
    case "value":
      return SymbolKind.Constant;
    case "variant":
      return SymbolKind.EnumMember;
    case "enum":
      return SymbolKind.Enum;
    case "default":
      return SymbolKind.Object;
    case "rule":
      return SymbolKind.Function;
    case "surface":
      return SymbolKind.Interface;
    case "actor":
      return SymbolKind.Class;
    case "config":
      return SymbolKind.Module;
  }
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

connection.onInitialize((params: InitializeParams): InitializeResult => {
  if (params.rootUri) {
    workspaceRoot = uriToPath(params.rootUri);
  } else if (params.rootPath) {
    workspaceRoot = params.rootPath;
  } else if (params.workspaceFolders?.length) {
    workspaceRoot = uriToPath(params.workspaceFolders[0].uri);
  }

  return {
    capabilities: {
      textDocumentSync: TextDocumentSyncKind.Full,
      hoverProvider: true,
      definitionProvider: true,
      referencesProvider: true,
      documentSymbolProvider: true,
      workspaceSymbolProvider: true,
      completionProvider: { triggerCharacters: [".", " "] },
      codeActionProvider: true,
      renameProvider: { prepareProvider: true },
      documentFormattingProvider: true,
      foldingRangeProvider: true,
      semanticTokensProvider: {
        legend: {
          tokenTypes: [...ALLIUM_SEMANTIC_TOKEN_TYPES],
          tokenModifiers: [],
        },
        full: true,
      },
      codeLensProvider: { resolveProvider: false },
      documentLinkProvider: { resolveProvider: false },
    },
  };
});

connection.onInitialized(() => {
  refreshWorkspaceIndex();
});

// ---------------------------------------------------------------------------
// Document sync + diagnostics  (T1.3)
// ---------------------------------------------------------------------------

function findingSeverityToDiagnostic(
  severity: "error" | "warning" | "info",
): DiagnosticSeverity {
  switch (severity) {
    case "error":
      return DiagnosticSeverity.Error;
    case "warning":
      return DiagnosticSeverity.Warning;
    default:
      return DiagnosticSeverity.Information;
  }
}

function publishDiagnostics(document: TextDocument): void {
  const text = document.getText();
  const findings = analyzeAllium(text);

  const diagnostics: Diagnostic[] = findings.map((finding) => ({
    range: Range.create(
      finding.start.line,
      finding.start.character,
      finding.end.line,
      finding.end.character,
    ),
    severity: findingSeverityToDiagnostic(finding.severity),
    code: finding.code,
    source: "allium",
    message: finding.message,
  }));

  connection.sendDiagnostics({ uri: document.uri, diagnostics });
}

documents.onDidOpen((event) => publishDiagnostics(event.document));
documents.onDidChangeContent((event) => publishDiagnostics(event.document));
documents.onDidSave((event) => {
  refreshWorkspaceIndex();
  publishDiagnostics(event.document);
});
documents.onDidClose((event) => {
  connection.sendDiagnostics({ uri: event.document.uri, diagnostics: [] });
});

// ---------------------------------------------------------------------------
// Hover  (T1.4)
// ---------------------------------------------------------------------------

connection.onHover((params): Hover | null => {
  const doc = documents.get(params.textDocument.uri);
  if (!doc) return null;

  const text = doc.getText();
  const offset = positionToOffset(text, params.position);
  const hoverText = hoverTextAtOffset(text, offset);
  if (!hoverText) return null;

  // Attach leading doc comment if hovering over a declaration
  const defs = findDefinitionsAtOffset(text, offset);
  let content = hoverText;
  if (defs.length > 0) {
    const docComment = findLeadingDocComment(text, defs[0].startOffset);
    if (docComment) {
      content = `${hoverText}\n\n---\n\n${docComment}`;
    }
  }

  return {
    contents: { kind: "markdown", value: content },
  };
});

// ---------------------------------------------------------------------------
// Go to definition  (T1.5)
// ---------------------------------------------------------------------------

connection.onDefinition((params): Location[] => {
  const doc = documents.get(params.textDocument.uri);
  if (!doc) return [];

  const text = doc.getText();
  const filePath = uriToPath(params.textDocument.uri);
  const offset = positionToOffset(text, params.position);

  // Cross-file (imported symbol resolution via use aliases)
  const crossFile = resolveImportedDefinition(
    filePath,
    text,
    offset,
    workspaceIndex,
  );
  if (crossFile.length > 0) {
    return crossFile.map(({ filePath: targetPath, definition }) => ({
      uri: pathToUri(targetPath),
      range: Range.create(
        offsetToPosition(
          workspaceIndex.documents.find(
            (d) => path.resolve(d.filePath) === path.resolve(targetPath),
          )?.text ?? "",
          definition.startOffset,
        ),
        offsetToPosition(
          workspaceIndex.documents.find(
            (d) => path.resolve(d.filePath) === path.resolve(targetPath),
          )?.text ?? "",
          definition.endOffset,
        ),
      ),
    }));
  }

  // Local definition lookup
  const defs = findDefinitionsAtOffset(text, offset);
  return defs.map((def) => ({
    uri: params.textDocument.uri,
    range: Range.create(
      offsetToPosition(text, def.startOffset),
      offsetToPosition(text, def.endOffset),
    ),
  }));
});

// ---------------------------------------------------------------------------
// Find references  (T1.6)
// ---------------------------------------------------------------------------

connection.onReferences((params): Location[] => {
  const doc = documents.get(params.textDocument.uri);
  if (!doc) return [];

  const text = doc.getText();
  const filePath = uriToPath(params.textDocument.uri);
  const offset = positionToOffset(text, params.position);
  const defs = findDefinitionsAtOffset(text, offset);
  const locations: Location[] = [];
  const seen = new Set<string>();
  const addLocation = (
    uri: string,
    sourceText: string,
    startOffset: number,
    endOffset: number,
  ): void => {
    const key = `${uri}:${startOffset}:${endOffset}`;
    if (seen.has(key)) {
      return;
    }
    seen.add(key);
    locations.push({
      uri,
      range: Range.create(
        offsetToPosition(sourceText, startOffset),
        offsetToPosition(sourceText, endOffset),
      ),
    });
  };

  if (defs.length > 0) {
    const definition = defs[0];
    const refs = findReferencesInText(text, definition);
    for (const ref of refs) {
      addLocation(params.textDocument.uri, text, ref.startOffset, ref.endOffset);
    }

    if (params.context.includeDeclaration) {
      addLocation(
        params.textDocument.uri,
        text,
        definition.startOffset,
        definition.endOffset,
      );
    }
    return locations;
  }

  // Imported symbol fallback: collect references in definition file and aliased uses.
  const imported = resolveImportedDefinition(filePath, text, offset, workspaceIndex);
  if (imported.length !== 1) {
    return [];
  }

  const { filePath: targetPath, definition } = imported[0];
  const targetDoc = workspaceIndex.documents.find(
    (d) => path.resolve(d.filePath) === path.resolve(targetPath),
  );
  if (!targetDoc) {
    return [];
  }

  const targetUri = pathToUri(targetDoc.filePath);
  for (const ref of findReferencesInText(targetDoc.text, definition)) {
    addLocation(targetUri, targetDoc.text, ref.startOffset, ref.endOffset);
  }

  for (const candidateDoc of workspaceIndex.documents) {
    const aliases = parseUseAliases(candidateDoc.text);
    for (const alias of aliases) {
      const resolved = resolveImportPath(candidateDoc.filePath, alias.sourcePath);
      if (path.resolve(resolved) !== path.resolve(targetDoc.filePath)) {
        continue;
      }
      const pattern = new RegExp(
        `\\b${escapeRegex(alias.alias)}[\\/.](${escapeRegex(definition.name)})\\b`,
        "g",
      );
      for (
        let match = pattern.exec(candidateDoc.text);
        match;
        match = pattern.exec(candidateDoc.text)
      ) {
        const startOffset = match.index + alias.alias.length + 1;
        addLocation(
          pathToUri(candidateDoc.filePath),
          candidateDoc.text,
          startOffset,
          startOffset + definition.name.length,
        );
      }
    }
  }

  return locations;
});

// ---------------------------------------------------------------------------
// Document symbols  (T1.7)
// ---------------------------------------------------------------------------

connection.onDocumentSymbol((params): DocumentSymbol[] => {
  const doc = documents.get(params.textDocument.uri);
  if (!doc) return [];

  const text = doc.getText();
  return collectAlliumSymbols(text).map((symbol) => ({
    name: symbol.name,
    kind: alliumSymbolTypeToSymbolKind(symbol.type),
    range: Range.create(
      offsetToPosition(text, symbol.startOffset),
      offsetToPosition(text, symbol.endOffset),
    ),
    selectionRange: Range.create(
      offsetToPosition(text, symbol.nameStartOffset),
      offsetToPosition(text, symbol.nameEndOffset),
    ),
  }));
});

// ---------------------------------------------------------------------------
// Workspace symbols  (T1.8)
// ---------------------------------------------------------------------------

connection.onWorkspaceSymbol((params): WorkspaceSymbol[] => {
  const records = collectWorkspaceSymbolRecords(workspaceIndex, params.query);
  return records.map((record) => {
    const doc = workspaceIndex.documents.find(
      (d) => path.resolve(d.filePath) === path.resolve(record.filePath),
    );
    const text = doc?.text ?? "";
    return {
      name: record.name,
      kind: definitionKindToSymbolKind(record.kind),
      location: {
        uri: pathToUri(record.filePath),
        range: Range.create(
          offsetToPosition(text, record.startOffset),
          offsetToPosition(text, record.endOffset),
        ),
      },
    };
  });
});

// ---------------------------------------------------------------------------
// Completions  (T1.9)
// ---------------------------------------------------------------------------

connection.onCompletion((params): CompletionList => {
  const doc = documents.get(params.textDocument.uri);
  if (!doc) return { isIncomplete: false, items: [] };

  const text = doc.getText();
  const offset = positionToOffset(text, params.position);
  const candidates = collectCompletionCandidates(text, offset);

  const items: CompletionItem[] = candidates.map((c) => ({
    label: c.label,
    kind:
      c.kind === "keyword"
        ? CompletionItemKind.Keyword
        : CompletionItemKind.Property,
  }));

  return { isIncomplete: false, items };
});

// ---------------------------------------------------------------------------
// Code actions  (T1.10)
// ---------------------------------------------------------------------------

function offsetsToTextEdit(
  text: string,
  startOffset: number,
  endOffset: number,
  newText: string,
): TextEdit {
  return {
    range: Range.create(
      offsetToPosition(text, startOffset),
      offsetToPosition(text, endOffset),
    ),
    newText,
  };
}

connection.onCodeAction((params): CodeAction[] => {
  const doc = documents.get(params.textDocument.uri);
  if (!doc) return [];

  const text = doc.getText();
  const uri = params.textDocument.uri;
  const selectionStart = positionToOffset(text, params.range.start);
  const selectionEnd = positionToOffset(text, params.range.end);
  const actions: CodeAction[] = [];

  // --- Refactors ---

  const extractLiteral = planExtractLiteralToConfig(
    text,
    selectionStart,
    selectionEnd,
  );
  if (extractLiteral) {
    actions.push({
      title: extractLiteral.title,
      kind: CodeActionKind.Refactor,
      edit: {
        changes: {
          [uri]: extractLiteral.edits.map((e) =>
            offsetsToTextEdit(text, e.startOffset, e.endOffset, e.text),
          ),
        },
      },
    });
  }

  const extractEnum = planExtractInlineEnumToNamedEnum(text, selectionStart);
  if (extractEnum) {
    actions.push({
      title: extractEnum.title,
      kind: CodeActionKind.Refactor,
      edit: {
        changes: {
          [uri]: extractEnum.edits.map((e) =>
            offsetsToTextEdit(text, e.startOffset, e.endOffset, e.text),
          ),
        },
      },
    });
  }

  const temporalGuard = planInsertTemporalGuard(text, selectionStart);
  if (temporalGuard) {
    actions.push({
      title: temporalGuard.title,
      kind: CodeActionKind.QuickFix,
      edit: {
        changes: {
          [uri]: [
            offsetsToTextEdit(
              text,
              temporalGuard.edit.startOffset,
              temporalGuard.edit.endOffset,
              temporalGuard.edit.text,
            ),
          ],
        },
      },
    });
  }

  // --- Suppression actions (per diagnostic in range) ---
  for (const diagnostic of params.context.diagnostics) {
    if (typeof diagnostic.code !== "string") continue;
    const edit = buildSuppressionDirectiveEdit(
      text,
      diagnostic.code,
      diagnostic.range.start.line,
    );
    if (edit) {
      const insertPos = offsetToPosition(text, edit.offset);
      actions.push({
        title: `Suppress: ${diagnostic.code}`,
        kind: CodeActionKind.QuickFix,
        edit: {
          changes: {
            [uri]: [{ range: Range.create(insertPos, insertPos), newText: edit.text }],
          },
        },
      });
    }
  }

  // --- Fix all (source action) ---
  const fixAllEdits = planSafeFixesByCategory(text, "strict", "all");
  if (fixAllEdits.length > 0) {
    actions.push({
      title: "Allium: Apply All Safe Fixes",
      kind: CodeActionKind.SourceFixAll,
      edit: {
        changes: {
          [uri]: fixAllEdits.map((e) =>
            offsetsToTextEdit(text, e.startOffset, e.endOffset, e.text),
          ),
        },
      },
    });
  }

  return actions;
});

// ---------------------------------------------------------------------------
// Rename  (T1.11)
// ---------------------------------------------------------------------------

connection.onPrepareRename((params) => {
  const doc = documents.get(params.textDocument.uri);
  if (!doc) return null;

  const text = doc.getText();
  const filePath = uriToPath(params.textDocument.uri);
  const offset = positionToOffset(text, params.position);
  const target = prepareRenameTarget(text, offset);
  if (target) {
    return Range.create(
      offsetToPosition(text, target.startOffset),
      offsetToPosition(text, target.endOffset),
    );
  }

  // Imported symbol fallback (e.g. alias.Symbol) for cross-file rename.
  const imported = resolveImportedDefinition(filePath, text, offset, workspaceIndex);
  if (imported.length === 1) {
    const bounds = tokenBoundsAtOffset(text, offset);
    if (bounds) {
      return Range.create(
        offsetToPosition(text, bounds.startOffset),
        offsetToPosition(text, bounds.endOffset),
      );
    }
  }

  throw new ResponseError(
    ErrorCodes.InvalidRequest,
    "No renameable symbol at cursor.",
  );
});

connection.onRenameRequest((params): WorkspaceEdit | null => {
  const doc = documents.get(params.textDocument.uri);
  if (!doc) return null;

  const text = doc.getText();
  const filePath = uriToPath(params.textDocument.uri);
  const offset = positionToOffset(text, params.position);
  const { plan, error } = planRename(text, offset, params.newName);

  if (error || !plan) {
    const imported = resolveImportedDefinition(filePath, text, offset, workspaceIndex);
    if (imported.length === 1) {
      const { filePath: targetFilePath, definition } = imported[0];
      const { edits, error: importedError } = planWorkspaceImportedRename(
        workspaceIndex,
        targetFilePath,
        definition,
        params.newName,
      );
      if (importedError) {
        throw new ResponseError(ErrorCodes.InvalidRequest, importedError);
      }

      const changes: Record<string, TextEdit[]> = {};
      for (const edit of edits) {
        const targetUri = pathToUri(edit.filePath);
        const targetDoc = workspaceIndex.documents.find(
          (d) => path.resolve(d.filePath) === path.resolve(edit.filePath),
        );
        const targetText = targetDoc?.text ?? "";
        if (!changes[targetUri]) changes[targetUri] = [];
        changes[targetUri].push(
          offsetsToTextEdit(
            targetText,
            edit.startOffset,
            edit.endOffset,
            params.newName,
          ),
        );
      }

      return { changes };
    }

    throw new ResponseError(ErrorCodes.InvalidRequest, error ?? "Rename failed.");
  }

  const changes: Record<string, TextEdit[]> = {};
  const uri = params.textDocument.uri;

  // Local edits (definition + references in this file)
  const localEdits: TextEdit[] = [
    offsetsToTextEdit(
      text,
      plan.definition.startOffset,
      plan.definition.endOffset,
      params.newName,
    ),
    ...plan.references.map((ref) =>
      offsetsToTextEdit(text, ref.startOffset, ref.endOffset, params.newName),
    ),
  ];
  changes[uri] = localEdits;

  // Cross-file edits (files that import this symbol via `use`)
  const { edits: crossFileEdits } = planWorkspaceImportedRename(
    workspaceIndex,
    filePath,
    plan.definition,
    params.newName,
  );
  for (const edit of crossFileEdits) {
    const targetUri = pathToUri(edit.filePath);
    const targetDoc = workspaceIndex.documents.find(
      (d) => path.resolve(d.filePath) === path.resolve(edit.filePath),
    );
    const targetText = targetDoc?.text ?? "";
    if (!changes[targetUri]) changes[targetUri] = [];
    changes[targetUri].push(
      offsetsToTextEdit(targetText, edit.startOffset, edit.endOffset, params.newName),
    );
  }

  return { changes };
});

// ---------------------------------------------------------------------------
// Formatting  (T1.12)
// ---------------------------------------------------------------------------

connection.onDocumentFormatting((params): TextEdit[] => {
  const doc = documents.get(params.textDocument.uri);
  if (!doc) return [];

  const text = doc.getText();
  const formatted = formatAlliumText(text);

  if (formatted === text) return [];

  // Replace entire document
  const lineCount = doc.lineCount;
  const lastLine = doc.getText({
    start: { line: lineCount - 1, character: 0 },
    end: { line: lineCount - 1, character: Number.MAX_SAFE_INTEGER },
  });

  return [
    {
      range: Range.create(0, 0, lineCount - 1, lastLine.length),
      newText: formatted,
    },
  ];
});

// ---------------------------------------------------------------------------
// Folding ranges  (T1.13)
// ---------------------------------------------------------------------------

connection.onFoldingRanges((params): FoldingRange[] => {
  const doc = documents.get(params.textDocument.uri);
  if (!doc) return [];

  const text = doc.getText();
  return collectTopLevelFoldingBlocks(text).map((block) => ({
    startLine: block.startLine,
    endLine: block.endLine,
    kind: FoldingRangeKind.Region,
  }));
});

// ---------------------------------------------------------------------------
// Semantic tokens  (T1.14)
// ---------------------------------------------------------------------------

connection.languages.semanticTokens.on(
  (_params: SemanticTokensParams): SemanticTokens => {
    const doc = documents.get(_params.textDocument.uri);
    if (!doc) return { data: [] };

    const text = doc.getText();
    const entries = collectSemanticTokenEntries(text);
    const tokenTypeIndex = Object.fromEntries(
      ALLIUM_SEMANTIC_TOKEN_TYPES.map((t, i) => [t, i]),
    );

    // Encode as delta-compressed [deltaLine, deltaChar, length, tokenType, modifiers]
    const data: number[] = [];
    let prevLine = 0;
    let prevChar = 0;

    for (const entry of entries) {
      const deltaLine = entry.line - prevLine;
      const deltaChar =
        deltaLine === 0 ? entry.character - prevChar : entry.character;
      data.push(
        deltaLine,
        deltaChar,
        entry.length,
        tokenTypeIndex[entry.tokenType] ?? 0,
        0,
      );
      prevLine = entry.line;
      prevChar = entry.character;
    }

    return { data };
  },
);

// ---------------------------------------------------------------------------
// Code lens  (T1.15)
// ---------------------------------------------------------------------------

connection.onCodeLens((params): CodeLens[] => {
  const doc = documents.get(params.textDocument.uri);
  if (!doc) return [];

  const text = doc.getText();
  const targets = collectCodeLensTargets(text);

  return targets.map((target) => {
    const range = Range.create(
      offsetToPosition(text, target.startOffset),
      offsetToPosition(text, target.endOffset),
    );
    return {
      range,
      command: {
        title: "Find references",
        command: "allium.findReferences",
        arguments: [params.textDocument.uri, range.start],
      },
    };
  });
});

// ---------------------------------------------------------------------------
// Document links  (T1.16)
// ---------------------------------------------------------------------------

connection.onDocumentLinks((params): DocumentLink[] => {
  const doc = documents.get(params.textDocument.uri);
  if (!doc) return [];

  const text = doc.getText();
  const filePath = uriToPath(params.textDocument.uri);
  const dir = path.dirname(filePath);

  return collectUseImportPaths(text).map((imp) => {
    const resolved = imp.sourcePath.endsWith(".allium")
      ? path.resolve(dir, imp.sourcePath)
      : path.resolve(dir, `${imp.sourcePath}.allium`);

    return {
      range: Range.create(
        offsetToPosition(text, imp.startOffset),
        offsetToPosition(text, imp.endOffset),
      ),
      target: pathToUri(resolved),
    };
  });
});

// ---------------------------------------------------------------------------
// Custom requests
// ---------------------------------------------------------------------------

const DEFAULT_DRIFT_EXCLUDE_DIRS = [
  ".git",
  "node_modules",
  "dist",
  "build",
  "target",
  "out",
  ".next",
  ".venv",
  "venv",
  "__pycache__",
];

connection.onRequest(
  "allium/simulateRule",
  (params: {
    uri: string;
    position: Position;
    bindings: Record<string, unknown>;
  }) => {
    const doc = documents.get(params.uri);
    if (!doc) return null;
    const text = doc.getText();
    const offset = positionToOffset(text, params.position);
    const preview = simulateRuleAtOffset(text, offset, params.bindings);
    if (!preview) return null;
    return { markdown: renderSimulationMarkdown(preview, params.bindings) };
  },
);

connection.onRequest("allium/generateScaffold", (params: { uri: string }) => {
  const doc = documents.get(params.uri);
  if (!doc) return null;
  const text = doc.getText();
  const moduleName = path.basename(uriToPath(params.uri), ".allium");
  const config = workspaceRoot
    ? readWorkspaceAlliumConfig(workspaceRoot)
    : undefined;
  const frameworkId = config?.scaffold?.framework;
  if (!frameworkId) {
    return { scaffold: null, noFramework: true };
  }
  const framework = getFramework(frameworkId);
  if (!framework) {
    return { scaffold: null, unknownFramework: frameworkId, supportedFrameworks: frameworkIds() };
  }
  return {
    scaffold: buildRuleTestScaffold(text, moduleName, framework),
    languageId: framework.languageId,
  };
});

connection.onRequest(
  "allium/getDiagram",
  (params: { uris: string[]; format: DiagramFormat }) => {
    const results = params.uris.map((uri) => {
      const doc = documents.get(uri);
      const text = doc
        ? doc.getText()
        : path.resolve(uriToPath(uri)).endsWith(".allium")
          ? fs.readFileSync(uriToPath(uri), "utf8")
          : "";
      return {
        uri,
        result: buildDiagramResult(text),
      };
    });

    const models = results.map((r) => r.result.model);
    const mergedModel = mergeDiagramModels(models);
    const issues = results.flatMap((r) => r.result.issues);
    const diagramText = renderDiagram(mergedModel, params.format);

    const sourceByNodeId: Record<string, { uri: string; offset: number }> = {};
    const sourceByEdgeId: Record<string, { uri: string; offset: number }> = {};

    for (const r of results) {
      for (const node of r.result.model.nodes) {
        if (node.sourceOffset !== undefined && !sourceByNodeId[node.id]) {
          sourceByNodeId[node.id] = { uri: r.uri, offset: node.sourceOffset };
        }
      }
      for (const edge of r.result.model.edges) {
        if (edge.sourceOffset !== undefined) {
          const edgeId = `${edge.from}|${edge.to}|${edge.label}`;
          if (!sourceByEdgeId[edgeId]) {
            sourceByEdgeId[edgeId] = { uri: r.uri, offset: edge.sourceOffset };
          }
        }
      }
    }

    return {
      diagramText,
      issues,
      model: mergedModel,
      sourceByNodeId,
      sourceByEdgeId,
    };
  },
);

connection.onRequest("allium/getSpecHealth", () => {
  if (!workspaceRoot)
    return { summaries: [], totalErrors: 0, totalWarnings: 0, totalInfos: 0 };
  const files = (fs
    .readdirSync(workspaceRoot, { recursive: true }) as string[])
    .filter((f: string) => f.endsWith(".allium"));
  let errors = 0;
  let warnings = 0;
  let infos = 0;
  const summaries: string[] = [];

  for (const f of files) {
    const filePath = path.join(workspaceRoot, f);
    const text = fs.readFileSync(filePath, "utf8");
    const findings = analyzeAllium(text);
    const e = findings.filter((f) => f.severity === "error").length;
    const w = findings.filter((f) => f.severity === "warning").length;
    const i = findings.filter((f) => f.severity === "info").length;
    errors += e;
    warnings += w;
    infos += i;
    summaries.push(`${path.basename(filePath)}  E:${e} W:${w} I:${i}`);
  }
  return { summaries: summaries.sort(), totalErrors: errors, totalWarnings: warnings, totalInfos: infos };
});

connection.onRequest("allium/getProblemsSummary", () => {
  if (!workspaceRoot) return { items: [] };
  const files = (fs
    .readdirSync(workspaceRoot, { recursive: true }) as string[])
    .filter((f: string) => f.endsWith(".allium"));
  const codeCounts = new Map<string, number>();
  for (const f of files) {
    const filePath = path.join(workspaceRoot, f);
    const text = fs.readFileSync(filePath, "utf8");
    const findings = analyzeAllium(text);
    for (const finding of findings) {
      codeCounts.set(finding.code, (codeCounts.get(finding.code) ?? 0) + 1);
    }
  }
  return {
    items: [...codeCounts.entries()]
      .sort((a, b) => b[1] - a[1])
      .map(([code, count]) => ({ label: `${code} (${count})`, code })),
  };
});

connection.onRequest("allium/getProblemsByCode", (params: { code: string }) => {
  if (!workspaceRoot) return { items: [] };
  const files = (fs
    .readdirSync(workspaceRoot, { recursive: true }) as string[])
    .filter((f: string) => f.endsWith(".allium"));
  const fileCounts = new Map<string, number>();
  for (const f of files) {
    const filePath = path.join(workspaceRoot, f);
    const text = fs.readFileSync(filePath, "utf8");
    const findings = analyzeAllium(text);
    const count = findings.filter((f) => f.code === params.code).length;
    if (count > 0) {
      fileCounts.set(filePath, count);
    }
  }
  return {
    items: [...fileCounts.entries()]
      .sort((a, b) => b[1] - a[1])
      .map(([filePath, count]) => ({
        label: `${path.basename(filePath)} (${count})`,
        filePath,
      })),
  };
});

connection.onRequest("allium/getDriftReport", () => {
  if (!workspaceRoot) return { markdown: "No workspace root." };
  const alliumConfig = readWorkspaceAlliumConfig(workspaceRoot);
  const driftConfig = alliumConfig?.drift;
  const sourceInputs = driftConfig?.sources ?? ["."];
  const sourceExtensions = driftConfig?.sourceExtensions ?? [".ts"];
  const excludeDirs = driftConfig?.excludeDirs ?? DEFAULT_DRIFT_EXCLUDE_DIRS;
  const specInputs = driftConfig?.specs ?? ["."];

  const sourceFiles = collectWorkspaceFiles(
    workspaceRoot,
    sourceInputs,
    sourceExtensions,
    excludeDirs,
  );
  const specFiles = collectWorkspaceFiles(
    workspaceRoot,
    specInputs,
    [".allium"],
    excludeDirs,
  );

  if (specFiles.length === 0) return { markdown: "No .allium files found." };

  const sourceText = sourceFiles
    .map((f) => fs.readFileSync(f, "utf8"))
    .join("\n");
  const specText = specFiles
    .map((f) => fs.readFileSync(f, "utf8"))
    .join("\n");

  const implementedDiagnostics = new Set(
    extractAlliumDiagnosticCodes(sourceText),
  );
  if (driftConfig?.diagnosticsFrom) {
    try {
      for (const code of readDiagnosticsManifest(
        workspaceRoot,
        driftConfig.diagnosticsFrom,
      )) {
        implementedDiagnostics.add(code);
      }
    } catch { /* ignore */ }
  }

  const specifiedDiagnostics = extractSpecDiagnosticCodes(specText);
  let implementedCommands = new Set<string>();
  if (!driftConfig?.skipCommands && driftConfig?.commandsFrom) {
    try {
      implementedCommands = readCommandManifest(
        workspaceRoot,
        driftConfig.commandsFrom,
      );
    } catch { /* ignore */ }
  }
  const specifiedCommands = extractSpecCommands(specText);
  const diagnosticsDrift = buildDriftReport(
    implementedDiagnostics,
    specifiedDiagnostics,
  );
  const commandsDrift = buildDriftReport(
    driftConfig?.skipCommands ? new Set<string>() : implementedCommands,
    specifiedCommands,
  );
  return { markdown: renderDriftMarkdown(diagnosticsDrift, commandsDrift) };
});

connection.onRequest(
  "allium/explainFinding",
  (params: { code: string; message: string }) => {
    return { markdown: buildFindingExplanationMarkdown(params.code, params.message) };
  },
);

connection.onRequest("allium/cleanSuppressions", (params: { uri: string }) => {
  const doc = documents.get(params.uri);
  if (!doc) return null;
  const original = doc.getText();
  const findings = analyzeAllium(original);
  const activeCodes = new Set(findings.map((f) => f.code));
  const cleanup = removeStaleSuppressions(original, activeCodes);
  return {
    text: cleanup.text,
    removedLines: cleanup.removedLines,
    removedCodes: cleanup.removedCodes,
  };
});

connection.onRequest(
  "allium/resolveRelatedFiles",
  (params: { uri: string; symbol: string }) => {
    if (!workspaceRoot) return { locations: [] };
    const alliumConfig = readWorkspaceAlliumConfig(workspaceRoot);
    const testOptions = resolveTestDiscoveryOptions(alliumConfig);
    const testMatcher = buildTestFileMatcher(
      testOptions.testExtensions,
      testOptions.testNamePatterns,
    );
    const excludedDirs =
      alliumConfig?.drift?.excludeDirs ?? DEFAULT_DRIFT_EXCLUDE_DIRS;
    const specInputs = alliumConfig?.project?.specPaths ?? ["."];
    const escaped = params.symbol.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    const matcher = new RegExp(`\\b${escaped}\\b`, "m");
    const matches: { label: string; description: string; uri: string }[] = [];

    const searchIn = (filePaths: string[]): void => {
      for (const filePath of filePaths) {
        if (filePath === uriToPath(params.uri)) continue;
        const text = fs.readFileSync(filePath, "utf8");
        if (matcher.test(text)) {
          matches.push({
            label: path.basename(filePath),
            description: path.relative(workspaceRoot!, filePath),
            uri: pathToUri(filePath),
          });
        }
      }
    };

    searchIn(
      collectWorkspaceFiles(
        workspaceRoot,
        specInputs,
        [".allium"],
        excludedDirs,
      ),
    );
    searchIn(
      collectWorkspaceFiles(
        workspaceRoot,
        testOptions.testInputs,
        testOptions.testExtensions,
        excludedDirs,
      ).filter(testMatcher),
    );

    return {
      locations: matches.sort((a, b) => a.label.localeCompare(b.label)),
    };
  },
);

connection.onRequest(
  "allium/createImportedSymbolStub",
  (params: { uri: string; alias: string; symbol: string }) => {
    const doc = documents.get(params.uri);
    if (!doc) return null;
    const sourceText = doc.getText();
    const useAliases = parseUseAliases(
      sourceText,
    );
    const useAlias = useAliases.find((entry: { alias: string; sourcePath: string }) => entry.alias === params.alias);
    if (!useAlias) return null;

    const currentFilePath = uriToPath(params.uri);
    const targetPath = path.resolve(
      path.dirname(currentFilePath),
      useAlias.sourcePath.endsWith(".allium")
        ? useAlias.sourcePath
        : `${useAlias.sourcePath}.allium`,
    );

    let existingText = "";
    let fileExists = false;
    try {
      existingText = fs.readFileSync(targetPath, "utf8");
      fileExists = true;
    } catch { /* ignore */ }

    if (new RegExp(`\\b${params.symbol}\\b`).test(existingText)) {
      return { alreadyExists: true, targetPath };
    }

    const needsLeadingNewline =
      existingText.length > 0 && !existingText.endsWith("\n");
    const insertion = `${needsLeadingNewline ? "\n" : ""}\nvalue ${params.symbol} {\n    value: TODO\n}\n`;

    return {
      targetUri: pathToUri(targetPath),
      insertion,
      offset: existingText.length,
      fileExists,
    };
  },
);

connection.onRequest(
  "allium/manageBaseline",
  (params: { action: "write" | "preview"; baselinePath: string }) => {
    if (!workspaceRoot) return { findings: [] };
    const files = (fs
      .readdirSync(workspaceRoot, { recursive: true }) as string[])
      .filter((f: string) => f.endsWith(".allium"));
    const records: string[] = [];
    for (const f of files) {
      const filePath = path.join(workspaceRoot, f);
      const text = fs.readFileSync(filePath, "utf8");
      const findings = analyzeAllium(text);
      for (const finding of findings) {
        const rel = path.relative(workspaceRoot, filePath);
        records.push(
          `${rel}|${finding.start.line}|${finding.start.character}|${finding.code}|${finding.message}`,
        );
      }
    }
    const unique = [...new Set(records)].sort();
    if (params.action === "write") {
      const output = {
        version: 1,
        findings: unique.map((fingerprint) => ({ fingerprint })),
      };
      const target = path.resolve(workspaceRoot, params.baselinePath);
      fs.mkdirSync(path.dirname(target), { recursive: true });
      fs.writeFileSync(
        target,
        `${JSON.stringify(output, null, 2)}\n`,
        "utf8",
      );
    }
    return { findings: unique };
  },
);

function mergeDiagramModels(models: DiagramModel[]): DiagramModel {
  const nodes = new Map<string, DiagramModel["nodes"][number]>();
  const edges = new Map<string, DiagramModel["edges"][number]>();
  for (const model of models) {
    for (const node of model.nodes) {
      nodes.set(node.id, node);
    }
    for (const edge of model.edges) {
      edges.set(`${edge.from}|${edge.to}|${edge.label}`, edge);
    }
  }
  return {
    nodes: [...nodes.values()].sort((a, b) => a.id.localeCompare(b.id)),
    edges: [...edges.values()].sort((a, b) =>
      `${a.from}|${a.to}|${a.label}`.localeCompare(
        `${b.from}|${b.to}|${b.label}`,
      ),
    ),
  };
}

// ---------------------------------------------------------------------------
// Start
// ---------------------------------------------------------------------------

documents.listen(connection);
connection.listen();
