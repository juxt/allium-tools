import { parseAllium } from "./wasm-ast";
import type { WasmDiagnostic } from "./wasm-ast";
import { wasmBlocksToParsedBlocks } from "./wasm-adapter";

export interface ParsedBlock {
  kind:
    | "rule"
    | "given"
    | "config"
    | "surface"
    | "actor"
    | "enum"
    | "use"
    | "contract"
    | "invariant"
    | "entity"
    | "value";
  name: string;
  nameStartOffset: number;
  startOffset: number;
  bodyStartOffset: number;
  endOffset: number;
  body: string;
  sourcePath?: string;
  alias?: string;
}

export interface ParsedDocument {
  blocks: ParsedBlock[];
  /** Parse diagnostics from the Rust front end (syntax errors, missing
   * version marker, etc.). Previously discarded; surfaced so the TS analyzer
   * reports the same parse failures as `allium check`. */
  diagnostics: WasmDiagnostic[];
}

export function parseAlliumDocument(text: string): ParsedDocument {
  const result = parseAllium(text);
  return {
    blocks: wasmBlocksToParsedBlocks(text, result),
    diagnostics: result.diagnostics,
  };
}

export function parseAlliumBlocks(text: string): ParsedBlock[] {
  return parseAlliumDocument(text).blocks;
}

export function findMatchingBrace(text: string, openOffset: number): number {
  let depth = 0;
  for (let i = openOffset; i < text.length; i += 1) {
    const char = text[i];
    if (char === "{") {
      depth += 1;
    } else if (char === "}") {
      depth -= 1;
      if (depth === 0) {
        return i;
      }
    }
  }
  return -1;
}
