import * as vscode from "vscode";
import * as path from "node:path";
import {
  LanguageClient,
  type LanguageClientOptions,
  type ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

const ALLIUM_LANGUAGE_ID = "allium";

let client: LanguageClient | undefined;

export function activate(context: vscode.ExtensionContext): void {
  console.log("Allium extension activating...");
  try {
    const serverModule = context.asAbsolutePath(
      path.join("dist", "allium-lsp.js"),
    );
    const serverOptions: ServerOptions = {
      run: { command: "node", args: [serverModule], transport: TransportKind.stdio },
      debug: { command: "node", args: [serverModule], transport: TransportKind.stdio },
    };
    const clientOptions: LanguageClientOptions = {
      documentSelector: [{ scheme: "file", language: ALLIUM_LANGUAGE_ID }],
    };
    client = new LanguageClient(
      "allium",
      "Allium Language Server",
      serverOptions,
      clientOptions,
    );
    void client.start();
    context.subscriptions.push({ dispose: () => void client?.stop() });

    context.subscriptions.push(
      vscode.commands.registerCommand("allium.runChecks", () => {
        void vscode.window.showInformationMessage(
          "Allium checks run automatically by the language server. Save the file to trigger a re-check.",
        );
      }),
    );
    context.subscriptions.push(
      vscode.commands.registerCommand("allium.applySafeFixes", async () => {
        await vscode.commands.executeCommand("editor.action.sourceAction", {
          kind: "source.fixAll.allium",
          apply: "always",
        });
      }),
    );

    context.subscriptions.push(
      vscode.commands.registerCommand("allium.showSpecHealth", async () => {
        await showSpecHealthSummary();
      }),
    );
    context.subscriptions.push(
      vscode.commands.registerCommand("allium.showProblemsSummary", async () => {
        await showProblemsSummary();
      }),
    );
    context.subscriptions.push(
      vscode.commands.registerCommand("allium.generateDiagram", async () => {
        await showDiagramPreview();
      }),
    );
    context.subscriptions.push(
      vscode.commands.registerCommand("allium.previewRename", async () => {
        void vscode.window.showInformationMessage(
          "Renaming is handled by the language server. Use the standard Rename command (F2).",
        );
      }),
    );
    context.subscriptions.push(
      vscode.commands.registerCommand(
        "allium.previewRuleSimulation",
        async () => {
          await previewRuleSimulation();
        },
      ),
    );
    context.subscriptions.push(
      vscode.commands.registerCommand(
        "allium.generateRuleTestScaffold",
        async () => {
          await generateRuleTestScaffold();
        },
      ),
    );
    context.subscriptions.push(
      vscode.commands.registerCommand(
        "allium.applyQuickFixesInFile",
        async () => {
          await applyAllQuickFixesInActiveFile();
        },
      ),
    );
    context.subscriptions.push(
      vscode.commands.registerCommand(
        "allium.cleanStaleSuppressions",
        async () => {
          await cleanStaleSuppressions();
        },
      ),
    );
    context.subscriptions.push(
      vscode.commands.registerCommand(
        "allium.openRelatedSpecOrTest",
        async () => {
          await openRelatedSpecOrTest();
        },
      ),
    );
    context.subscriptions.push(
      vscode.commands.registerCommand("allium.explainFinding", async () => {
        await explainFindingAtCursor();
      }),
    );
    context.subscriptions.push(
      vscode.commands.registerCommand("allium.checkSpecDrift", async () => {
        await checkSpecDriftReport();
      }),
    );
    context.subscriptions.push(
      vscode.commands.registerCommand(
        "allium.explainFindingDiagnostic",
        async (code: string, message: string) => {
          await showFindingExplanation(code, message);
        },
      ),
    );
    context.subscriptions.push(
      vscode.commands.registerCommand(
        "allium.createImportedSymbolStub",
        async (uri: vscode.Uri, alias: string, symbol: string) => {
          await createImportedSymbolStub(uri, alias, symbol);
        },
      ),
    );
    context.subscriptions.push(
      vscode.commands.registerCommand("allium.manageBaseline", async () => {
        await manageWorkspaceBaseline();
      }),
    );
    console.log("Allium extension activated successfully.");
  } catch (err) {
    console.error("Failed to activate Allium extension:", err);
    void vscode.window.showErrorMessage(`Allium extension failed to activate: ${err}`);
  }
}

export function deactivate(): Thenable<void> | undefined {
  return client?.stop();
}

async function applyAllQuickFixesInActiveFile(): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== ALLIUM_LANGUAGE_ID) {
    void vscode.window.showInformationMessage(
      "Open an .allium file to apply quick fixes.",
    );
    return;
  }
  const document = editor.document;
  const wholeRange = new vscode.Range(
    document.positionAt(0),
    document.positionAt(document.getText().length),
  );
  const actions =
    (await vscode.commands.executeCommand<vscode.CodeAction[]>(
      "vscode.executeCodeActionProvider",
      document.uri,
      wholeRange,
      vscode.CodeActionKind.QuickFix.value,
    )) ?? [];
  const quickFixEdits = actions
    .filter(
      (action) =>
        !!action.edit &&
        action.diagnostics?.some((diag) =>
          String(diag.code ?? "").startsWith("allium."),
        ),
    )
    .map((action) => action.edit as vscode.WorkspaceEdit);

  if (quickFixEdits.length === 0) {
    void vscode.window.showInformationMessage(
      "No Allium quick fixes available in this file.",
    );
    return;
  }

  const previewLines = actions
    .filter(
      (action) =>
        !!action.edit &&
        action.diagnostics?.some((diag) =>
          String(diag.code ?? "").startsWith("allium."),
        ),
    )
    .map(
      (action) =>
        `- ${action.title}${
          action.diagnostics?.[0]?.code
            ? ` (\`${String(action.diagnostics[0].code)}\`)`
            : ""
        }`,
    );
  const previewDoc = await vscode.workspace.openTextDocument({
    content: [
      "# Allium Quick Fix Preview",
      "",
      `File: \`${path.basename(document.uri.fsPath)}\``,
      `Planned fixes: ${previewLines.length}`,
      "",
      ...previewLines,
      "",
    ].join("\n"),
    language: "markdown",
  });
  await vscode.window.showTextDocument(previewDoc, { preview: true });
  const decision = await vscode.window.showQuickPick(["Apply fixes", "Cancel"], {
    placeHolder: "Apply these quick fixes?",
  });
  if (decision !== "Apply fixes") {
    return;
  }

  let applied = 0;
  for (const edit of quickFixEdits) {
    const ok = await vscode.workspace.applyEdit(edit);
    if (ok) {
      applied += 1;
    }
  }
  void vscode.window.showInformationMessage(
    `Applied ${applied} Allium quick fix(es).`,
  );
}

async function cleanStaleSuppressions(): Promise<void> {
  if (!client) return;
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== ALLIUM_LANGUAGE_ID) {
    void vscode.window.showInformationMessage(
      "Open an .allium file to clean suppressions.",
    );
    return;
  }
  const result = await client.sendRequest<{
    text: string;
    removedLines: number;
    removedCodes: number;
  } | null>("allium/cleanSuppressions", {
    uri: editor.document.uri.toString(),
  });
  if (!result) return;
  if (result.removedLines === 0 && result.removedCodes === 0) {
    void vscode.window.showInformationMessage("No stale suppressions found.");
    return;
  }
  const edit = new vscode.WorkspaceEdit();
  edit.replace(
    editor.document.uri,
    new vscode.Range(0, 0, editor.document.lineCount, 0),
    result.text,
  );
  await vscode.workspace.applyEdit(edit);
  void vscode.window.showInformationMessage(
    `Removed ${result.removedLines} stale suppression line(s) and ${result.removedCodes} stale code reference(s).`,
  );
}

async function openRelatedSpecOrTest(): Promise<void> {
  if (!client) return;
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== ALLIUM_LANGUAGE_ID) {
    void vscode.window.showInformationMessage("Open an .allium file first.");
    return;
  }
  const symbolRange = editor.document.getWordRangeAtPosition(
    editor.selection.active,
    /[A-Za-z_][A-Za-z0-9_]*/,
  );
  if (!symbolRange) {
    void vscode.window.showInformationMessage(
      "Place cursor on a symbol name first.",
    );
    return;
  }
  const symbol = editor.document.getText(symbolRange);
  const result = await client.sendRequest<{
    locations: { label: string; description: string; uri: string }[];
  }>("allium/resolveRelatedFiles", {
    uri: editor.document.uri.toString(),
    symbol,
  });

  if (result.locations.length === 0) {
    void vscode.window.showInformationMessage(
      `No related spec/test files found for '${symbol}'.`,
    );
    return;
  }
  if (result.locations.length === 1) {
    const doc = await vscode.workspace.openTextDocument(
      vscode.Uri.parse(result.locations[0].uri),
    );
    await vscode.window.showTextDocument(doc);
    return;
  }
  const item = await vscode.window.showQuickPick(result.locations, {
    placeHolder: `Related files for '${symbol}'`,
  });
  if (!item) {
    return;
  }
  const doc = await vscode.workspace.openTextDocument(
    vscode.Uri.parse(item.uri),
  );
  await vscode.window.showTextDocument(doc);
}

async function showSpecHealthSummary(): Promise<void> {
  if (!client) return;
  const result = await client.sendRequest<{
    summaries: string[];
    totalErrors: number;
    totalWarnings: number;
    totalInfos: number;
  }>("allium/getSpecHealth", {});
  const pick = await vscode.window.showQuickPick(result.summaries, {
    placeHolder: `Allium spec health — Errors: ${result.totalErrors}, Warnings: ${result.totalWarnings}, Info: ${result.totalInfos}`,
  });
  if (!pick) {
    return;
  }
  void vscode.window.showInformationMessage(pick);
}

async function showProblemsSummary(): Promise<void> {
  if (!client) return;
  const summary = await client.sendRequest<{
    items: { label: string; code: string }[];
  }>("allium/getProblemsSummary", {});
  if (summary.items.length === 0) {
    void vscode.window.showInformationMessage("No Allium findings.");
    return;
  }

  const summaryPick = await vscode.window.showQuickPick(summary.items, {
    placeHolder: "Allium problems grouped by code",
  });
  if (!summaryPick) {
    return;
  }

  const fileResults = await client.sendRequest<{
    items: { label: string; filePath: string }[];
  }>("allium/getProblemsByCode", { code: summaryPick.code });
  const filePick = await vscode.window.showQuickPick(fileResults.items, {
    placeHolder: summaryPick.code,
  });
  if (!filePick) {
    return;
  }
  const doc = await vscode.workspace.openTextDocument(
    vscode.Uri.file(filePick.filePath),
  );
  await vscode.window.showTextDocument(doc);
}

async function checkSpecDriftReport(): Promise<void> {
  if (!client) return;
  const result = await client.sendRequest<{ markdown: string }>(
    "allium/getDriftReport",
    {},
  );
  const doc = await vscode.workspace.openTextDocument({
    content: result.markdown,
    language: "markdown",
  });
  await vscode.window.showTextDocument(doc, { preview: true });
}

async function previewRuleSimulation(): Promise<void> {
  if (!client) return;
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== ALLIUM_LANGUAGE_ID) {
    void vscode.window.showInformationMessage("Open an .allium file first.");
    return;
  }
  const raw = await vscode.window.showInputBox({
    prompt:
      'Enter sample bindings JSON object for simulation (for example: {"status":"approved"})',
    value: "{}",
  });
  if (raw === undefined) {
    return;
  }
  let bindings: Record<string, unknown>;
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      throw new Error("Bindings must be a JSON object.");
    }
    bindings = parsed as Record<string, unknown>;
  } catch {
    void vscode.window.showErrorMessage(
      "Invalid JSON bindings. Please provide an object.",
    );
    return;
  }
  const result = await client.sendRequest<{ markdown: string } | null>(
    "allium/simulateRule",
    {
      uri: editor.document.uri.toString(),
      position: editor.selection.active,
      bindings,
    },
  );
  if (!result) {
    void vscode.window.showInformationMessage(
      "Place cursor inside a rule block first.",
    );
    return;
  }
  const doc = await vscode.workspace.openTextDocument({
    content: result.markdown,
    language: "markdown",
  });
  await vscode.window.showTextDocument(doc, { preview: true });
}

async function generateRuleTestScaffold(): Promise<void> {
  if (!client) return;
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== ALLIUM_LANGUAGE_ID) {
    void vscode.window.showInformationMessage("Open an .allium file first.");
    return;
  }
  const result = await client.sendRequest<{
    scaffold: string | null;
    languageId?: string;
    noFramework?: boolean;
    unknownFramework?: string;
    supportedFrameworks?: string[];
  }>("allium/generateScaffold", { uri: editor.document.uri.toString() });
  if (!result) {
    void vscode.window.showInformationMessage(
      "No rules found in current spec.",
    );
    return;
  }
  if (result.noFramework) {
    void vscode.window.showInformationMessage(
      `Set "scaffold": { "framework": "..." } in allium.config.json to generate test scaffolds.`,
    );
    return;
  }
  if (result.unknownFramework) {
    const supported = result.supportedFrameworks?.join(", ") ?? "(unknown)";
    void vscode.window.showInformationMessage(
      `Unknown scaffold framework "${result.unknownFramework}". Supported: ${supported}`,
    );
    return;
  }
  if (!result.scaffold) {
    void vscode.window.showInformationMessage(
      "No rules found in current spec.",
    );
    return;
  }
  const doc = await vscode.workspace.openTextDocument({
    content: result.scaffold,
    language: result.languageId ?? "plaintext",
  });
  await vscode.window.showTextDocument(doc);
}

async function manageWorkspaceBaseline(): Promise<void> {
  if (!client) return;
  const action = await vscode.window.showQuickPick(
    ["Write baseline", "Preview baseline findings", "Cancel"],
    { placeHolder: "Allium baseline manager" },
  );
  if (!action || action === "Cancel") {
    return;
  }
  const baselinePath = await vscode.window.showInputBox({
    prompt: "Baseline output path",
    value: ".allium-baseline.json",
  });
  if (!baselinePath) {
    return;
  }

  const result = await client.sendRequest<{ findings: string[] }>(
    "allium/manageBaseline",
    {
      action: action === "Write baseline" ? "write" : "preview",
      baselinePath,
    },
  );

  if (action === "Preview baseline findings") {
    const preview = await vscode.workspace.openTextDocument({
      content: [
        "# Baseline Preview",
        "",
        ...result.findings.map((line) => `- \`${line}\``),
      ].join("\n"),
      language: "markdown",
    });
    await vscode.window.showTextDocument(preview, { preview: true });
    return;
  }

  void vscode.window.showInformationMessage(
    `Wrote baseline with ${result.findings.length} finding fingerprints to ${baselinePath}.`,
  );
}

async function showDiagramPreview(): Promise<void> {
  if (!client) return;
  const active = vscode.window.activeTextEditor?.document;
  const choices: Array<{
    label: string;
    detail: string;
    scope: "active" | "workspace";
  }> = [];
  if (active?.languageId === ALLIUM_LANGUAGE_ID) {
    choices.push({
      label: "Active .allium file",
      detail: path.basename(active.uri.fsPath),
      scope: "active",
    });
  }
  choices.push({
    label: "All workspace .allium files",
    detail: "Merge all specs into one diagram",
    scope: "workspace",
  });

  const scopePick = await vscode.window.showQuickPick(choices, {
    placeHolder: "Choose diagram source",
  });
  if (!scopePick) {
    return;
  }

  const formatPick = (await vscode.window.showQuickPick(["d2", "mermaid"], {
    placeHolder: "Choose diagram format",
  })) as "d2" | "mermaid" | undefined;
  if (!formatPick) {
    return;
  }

  let uris: string[] = [];
  if (scopePick.scope === "active") {
    if (!active || active.languageId !== ALLIUM_LANGUAGE_ID) {
      void vscode.window.showInformationMessage("Open an .allium file first.");
      return;
    }
    uris = [active.uri.toString()];
  } else {
    const files = await vscode.workspace.findFiles(
      "**/*.allium",
      "**/{node_modules,dist,.git}/**",
    );
    if (files.length === 0) {
      void vscode.window.showInformationMessage(
        "No .allium files found in workspace.",
      );
      return;
    }
    uris = files.map((f) => f.toString());
  }

  const result = await client.sendRequest<{
    diagramText: string;
    issues: Array<{ message: string }>;
    model: {
      nodes: Array<{ id: string; label: string }>;
      edges: Array<{ from: string; to: string; label: string }>;
    };
    sourceByNodeId: Record<string, { uri: string; offset: number }>;
    sourceByEdgeId: Record<string, { uri: string; offset: number }>;
  }>("allium/getDiagram", { uris, format: formatPick });

  const panel = vscode.window.createWebviewPanel(
    "allium.diagram.preview",
    `Allium Diagram (${formatPick})`,
    vscode.ViewColumn.Beside,
    { enableScripts: true },
  );
  panel.webview.html = buildDiagramPreviewHtml({
    format: formatPick,
    diagramText: result.diagramText,
    issues: result.issues,
    nodes: result.model.nodes.map((node: { id: string; label: string }) => ({
      id: node.id,
      label: node.label,
    })),
    edges: result.model.edges.map(
      (edge: { from: string; to: string; label: string }) => ({
        id: `${edge.from}|${edge.to}|${edge.label}`,
        label: `${edge.from} -> ${edge.to} (${edge.label})`,
      }),
    ),
  });

  panel.webview.onDidReceiveMessage(
    async (message: {
      type: string;
      nodeId?: string;
      edgeId?: string;
    }) => {
      if (message.type === "copy") {
        await vscode.env.clipboard.writeText(result.diagramText);
        void vscode.window.showInformationMessage("Allium diagram copied.");
        return;
      }
      if (message.type === "export") {
        const extension = formatPick === "mermaid" ? "mmd" : "d2";
        const uri = await vscode.window.showSaveDialog({
          defaultUri: vscode.Uri.file(`allium-diagram.${extension}`),
        });
        if (!uri) return;
        await vscode.workspace.fs.writeFile(
          uri,
          Buffer.from(result.diagramText, "utf8"),
        );
        void vscode.window.showInformationMessage(
          `Allium diagram exported to ${uri.fsPath}.`,
        );
        return;
      }
      if (message.type === "reveal") {
        if (!message.nodeId) return;
        const source = result.sourceByNodeId[message.nodeId];
        if (!source) return;
        const document = await vscode.workspace.openTextDocument(
          vscode.Uri.parse(source.uri),
        );
        const editor = await vscode.window.showTextDocument(document);
        const position = document.positionAt(source.offset);
        editor.selection = new vscode.Selection(position, position);
        editor.revealRange(new vscode.Range(position, position));
      }
      if (message.type === "revealEdge") {
        if (!message.edgeId) return;
        const source = result.sourceByEdgeId[message.edgeId];
        if (!source) return;
        const document = await vscode.workspace.openTextDocument(
          vscode.Uri.parse(source.uri),
        );
        const editor = await vscode.window.showTextDocument(document);
        const position = document.positionAt(source.offset);
        editor.selection = new vscode.Selection(position, position);
        editor.revealRange(new vscode.Range(position, position));
      }
    },
    undefined,
    [],
  );
}

async function explainFindingAtCursor(): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== ALLIUM_LANGUAGE_ID) {
    void vscode.window.showInformationMessage("Open an .allium file first.");
    return;
  }
  const position = editor.selection.active;
  const allDiagnostics = vscode.languages.getDiagnostics(editor.document.uri);
  const entry = allDiagnostics.find(
    (diagnostic) =>
      diagnostic.source === "allium" &&
      typeof diagnostic.code === "string" &&
      diagnostic.range.contains(position),
  );
  if (!entry || typeof entry.code !== "string") {
    void vscode.window.showInformationMessage(
      "No Allium finding at cursor position.",
    );
    return;
  }
  await showFindingExplanation(entry.code, entry.message);
}

async function showFindingExplanation(
  code: string,
  message: string,
): Promise<void> {
  if (!client) return;
  const result = await client.sendRequest<{ markdown: string }>(
    "allium/explainFinding",
    { code, message },
  );
  const doc = await vscode.workspace.openTextDocument({
    content: result.markdown,
    language: "markdown",
  });
  await vscode.window.showTextDocument(doc, { preview: true });
}

async function createImportedSymbolStub(
  uri: vscode.Uri,
  alias: string,
  symbol: string,
): Promise<void> {
  if (!client) return;
  const result = await client.sendRequest<{
    targetUri?: string;
    insertion?: string;
    offset?: number;
    fileExists?: boolean;
    alreadyExists?: boolean;
    targetPath?: string;
  } | null>("allium/createImportedSymbolStub", {
    uri: uri.toString(),
    alias,
    symbol,
  });

  if (!result) {
    void vscode.window.showErrorMessage(
      `Could not resolve import alias '${alias}' in current document.`,
    );
    return;
  }

  if (result.alreadyExists) {
    void vscode.window.showInformationMessage(
      `Symbol '${symbol}' already exists in ${path.basename(
        result.targetPath!,
      )}.`,
    );
    return;
  }

  const targetUri = vscode.Uri.parse(result.targetUri!);
  const edit = new vscode.WorkspaceEdit();
  if (!result.fileExists) {
    edit.createFile(targetUri, { ignoreIfExists: true });
  }

  const insertPosition = result.fileExists
    ? (await vscode.workspace.openTextDocument(targetUri)).positionAt(
        result.offset!,
      )
    : new vscode.Position(0, 0);

  edit.insert(targetUri, insertPosition, result.insertion!);
  const applied = await vscode.workspace.applyEdit(edit);
  if (!applied) {
    void vscode.window.showErrorMessage(
      `Failed to create '${symbol}' in imported specification.`,
    );
    return;
  }
  const doc = await vscode.workspace.openTextDocument(targetUri);
  await vscode.window.showTextDocument(doc);
}

function buildDiagramPreviewHtml(params: {
  format: string;
  diagramText: string;
  issues: Array<{ message: string }>;
  nodes: Array<{ id: string; label: string }>;
  edges: Array<{ id: string; label: string }>;
}): string {
  const issuesHtml = params.issues
    .map((issue: { message: string }) => `<li>${issue.message}</li>`)
    .join("");
  const nodesJson = JSON.stringify(params.nodes);
  const edgesJson = JSON.stringify(params.edges);

  return `<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>Allium Diagram Preview</title>
    <style>
        body { font-family: sans-serif; padding: 20px; }
        pre { background: #f4f4f4; padding: 10px; overflow: auto; }
        .controls { margin-bottom: 20px; }
        .issues { color: #d9534f; }
    </style>
</head>
<body>
    <div class="controls">
        <button onclick="copy()">Copy to Clipboard</button>
        <button onclick="exportDiagram()">Export File</button>
    </div>
    ${
      params.issues.length > 0
        ? `<div class="issues"><h3>Issues:</h3><ul>${issuesHtml}</ul></div>`
        : ""
    }
    <div id="diagram">
        <h3>Diagram (${params.format})</h3>
        <pre>${params.diagramText}</pre>
    </div>
    <div id="navigation">
        <h3>Navigation</h3>
        <p>Nodes: <select id="nodesSelect" onchange="revealNode()"><option value="">Select a node to reveal</option></select></p>
        <p>Edges: <select id="edgesSelect" onchange="revealEdge()"><option value="">Select an edge to reveal</option></select></p>
    </div>
    <script>
        const vscode = acquireVsCodeApi();
        const nodes = ${nodesJson};
        const edges = ${edgesJson};

        const nodesSelect = document.getElementById('nodesSelect');
        nodes.forEach(n => {
            const opt = document.createElement('option');
            opt.value = n.id;
            opt.textContent = n.label;
            nodesSelect.appendChild(opt);
        });

        const edgesSelect = document.getElementById('edgesSelect');
        edges.forEach(e => {
            const opt = document.createElement('option');
            opt.value = e.id;
            opt.textContent = e.label;
            edgesSelect.appendChild(opt);
        });

        function copy() { vscode.postMessage({ type: 'copy' }); }
        function exportDiagram() { vscode.postMessage({ type: 'export' }); }
        function revealNode() {
            const val = nodesSelect.value;
            if (val) vscode.postMessage({ type: 'reveal', nodeId: val });
        }
        function revealEdge() {
            const val = edgesSelect.value;
            if (val) vscode.postMessage({ type: 'revealEdge', edgeId: val });
        }
    </script>
</body>
</html>`;
}
