import { parseAllium } from "./wasm-ast";
import type {
	WasmBlockDecl,
	WasmBlockKind,
	WasmDecl,
	WasmDefaultDecl,
	WasmVariantDecl,
} from "./wasm-ast";

export type AlliumSymbolType =
  | "entity"
  | "external entity"
  | "value"
  | "variant"
  | "enum"
  | "default"
  | "rule"
  | "surface"
  | "actor"
  | "config"
  | "contract"
  | "invariant"
  | "deferred";

export interface AlliumSymbol {
  type: AlliumSymbolType;
  name: string;
  startOffset: number;
  endOffset: number;
  nameStartOffset: number;
  nameEndOffset: number;
}

const BLOCK_KIND_TO_SYMBOL: Record<WasmBlockKind, AlliumSymbolType> = {
  Entity: "entity",
  ExternalEntity: "external entity",
  Value: "value",
  Enum: "enum",
  Given: "config",
  Config: "config",
  Rule: "rule",
  Surface: "surface",
  Actor: "actor",
  Contract: "contract",
  Invariant: "invariant",
};

export function collectAlliumSymbols(text: string): AlliumSymbol[] {
  const result = parseAllium(text);
  const symbols: AlliumSymbol[] = [];

  for (const decl of result.module.declarations) {
    const key = Object.keys(decl)[0] as keyof WasmDecl;

    if (key === "Block") {
      const block = (decl as { Block: WasmBlockDecl }).Block;
      const type = BLOCK_KIND_TO_SYMBOL[block.kind];
      if (!type) continue;

      const name = block.name?.name ?? type;
      const nameStart = block.name?.span.start ?? block.span.start;
      const nameEnd = block.name?.span.end ?? (nameStart + name.length);

      symbols.push({
        type,
        name,
        startOffset: block.span.start,
        endOffset: block.span.end - 1,
        nameStartOffset: nameStart,
        nameEndOffset: nameEnd,
      });
    } else if (key === "Variant") {
      const v = (decl as { Variant: WasmVariantDecl }).Variant;
      symbols.push({
        type: "variant",
        name: v.name.name,
        startOffset: v.span.start,
        endOffset: v.span.end,
        nameStartOffset: v.name.span.start,
        nameEndOffset: v.name.span.end,
      });
    } else if (key === "Default") {
      const d = (decl as { Default: WasmDefaultDecl }).Default;
      symbols.push({
        type: "default",
        name: d.name.name,
        startOffset: d.span.start,
        endOffset: d.span.end,
        nameStartOffset: d.name.span.start,
        nameEndOffset: d.name.span.end,
      });
    } else if (key === "Invariant") {
      const inv = (decl as { Invariant: { span: { start: number; end: number }; name: { span: { start: number; end: number }; name: string } } }).Invariant;
      symbols.push({
        type: "invariant",
        name: inv.name.name,
        startOffset: inv.span.start,
        endOffset: inv.span.end,
        nameStartOffset: inv.name.span.start,
        nameEndOffset: inv.name.span.end,
      });
    } else if (key === "Deferred") {
      const d = (decl as { Deferred: { span: { start: number; end: number }; path: unknown } }).Deferred;
      symbols.push({
        type: "deferred",
        name: "deferred",
        startOffset: d.span.start,
        endOffset: d.span.end,
        nameStartOffset: d.span.start,
        nameEndOffset: d.span.end,
      });
    }
  }

  return symbols.sort((a, b) => a.startOffset - b.startOffset);
}
