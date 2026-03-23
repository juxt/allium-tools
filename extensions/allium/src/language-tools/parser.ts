import { parseAllium } from "./wasm-ast";
import { wasmBlocksToParsedBlocks } from "./wasm-adapter";

export interface ParsedBlock {
  kind: "rule" | "given" | "config" | "surface" | "actor" | "enum" | "use" | "contract" | "invariant" | "entity" | "value";
  name: string;
  nameStartOffset: number;
  startOffset: number;
  bodyStartOffset: number;
  endOffset: number;
  body: string;
  sourcePath?: string;
  alias?: string;
}

export function parseAlliumBlocks(text: string): ParsedBlock[] {
  const result = parseAllium(text);
  return wasmBlocksToParsedBlocks(text, result);
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
