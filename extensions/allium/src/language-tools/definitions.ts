import { parseAllium } from "./wasm-ast";
import type {
  WasmBlockDecl,
  WasmBlockItem,
  WasmBlockItemKind,
  WasmDecl,
  WasmDefaultDecl,
  WasmVariantDecl,
} from "./wasm-ast";

export interface DefinitionSite {
  name: string;
  kind:
    | "entity"
    | "external_entity"
    | "value"
    | "variant"
    | "enum"
    | "default_instance"
    | "rule"
    | "surface"
    | "actor"
    | "config_key";
  startOffset: number;
  endOffset: number;
}

export interface DefinitionLookup {
  symbols: DefinitionSite[];
  configKeys: DefinitionSite[];
}

const BLOCK_KIND_TO_DEF: Record<string, DefinitionSite["kind"] | undefined> = {
  Entity: "entity",
  ExternalEntity: "external_entity",
  Value: "value",
  Enum: "enum",
  Rule: "rule",
  Surface: "surface",
  Actor: "actor",
};

export function buildDefinitionLookup(text: string): DefinitionLookup {
  const result = parseAllium(text);
  const symbols: DefinitionSite[] = [];
  const configKeys: DefinitionSite[] = [];

  for (const decl of result.module.declarations) {
    const key = Object.keys(decl)[0] as keyof WasmDecl;

    if (key === "Block") {
      const block = (decl as { Block: WasmBlockDecl }).Block;
      const kind = BLOCK_KIND_TO_DEF[block.kind];

      if (kind && block.name) {
        symbols.push({
          name: block.name.name,
          kind,
          startOffset: block.name.span.start,
          endOffset: block.name.span.end,
        });
      }

      if (block.kind === "Config" || block.kind === "Given") {
        for (const item of block.items) {
          const configKey = extractConfigKey(item);
          if (configKey) {
            configKeys.push(configKey);
          }
        }
      }
    } else if (key === "Variant") {
      const v = (decl as { Variant: WasmVariantDecl }).Variant;
      symbols.push({
        name: v.name.name,
        kind: "variant",
        startOffset: v.name.span.start,
        endOffset: v.name.span.end,
      });
    } else if (key === "Default") {
      const d = (decl as { Default: WasmDefaultDecl }).Default;
      symbols.push({
        name: d.name.name,
        kind: "default_instance",
        startOffset: d.name.span.start,
        endOffset: d.name.span.end,
      });
    }
  }

  return { symbols, configKeys };
}

function extractConfigKey(item: WasmBlockItem): DefinitionSite | null {
  const kind = item.kind;
  if ("Assignment" in kind) {
    const a = (kind as Extract<WasmBlockItemKind, { Assignment: unknown }>).Assignment;
    return {
      name: a.name.name,
      kind: "config_key",
      startOffset: a.name.span.start,
      endOffset: a.name.span.end,
    };
  }
  return null;
}

export function findDefinitionsAtOffset(
  text: string,
  offset: number,
): DefinitionSite[] {
  const token = tokenAtOffset(text, offset);
  if (!token) {
    return [];
  }

  const lookup = buildDefinitionLookup(text);
  if (token.kind === "configKey") {
    return lookup.configKeys.filter((entry) => entry.name === token.name);
  }
  return lookup.symbols.filter((entry) => entry.name === token.name);
}

export function tokenAtOffset(
  text: string,
  offset: number,
): { name: string; kind: "symbol" | "configKey" } | null {
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

  const name = text.slice(start, end);
  const prefixStart = start - "config.".length;
  if (prefixStart >= 0 && text.slice(prefixStart, start) === "config.") {
    return { name, kind: "configKey" };
  }
  return { name, kind: "symbol" };
}

export interface UseAlias {
  alias: string;
  sourcePath: string;
}

export function parseUseAliases(text: string): UseAlias[] {
  const result = parseAllium(text);
  const aliases: UseAlias[] = [];

  for (const decl of result.module.declarations) {
    if ("Use" in decl) {
      const use_ = decl.Use;
      if (use_.alias) {
        const pathText = use_.path.parts
          .map((p) => ("Text" in p ? p.Text : ""))
          .join("");
        aliases.push({ sourcePath: pathText, alias: use_.alias.name });
      }
    }
  }
  return aliases;
}

export function importedSymbolAtOffset(
  text: string,
  offset: number,
): { alias: string; symbol: string } | null {
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
  const symbol = text.slice(start, end);
  const separatorIndex = start - 1;
  if (separatorIndex < 1) {
    return null;
  }
  const separator = text[separatorIndex];
  if (separator !== "/" && separator !== ".") {
    return null;
  }

  const aliasEnd = separatorIndex;
  let aliasStart = aliasEnd;
  while (aliasStart > 0 && isIdent(text[aliasStart - 1])) {
    aliasStart -= 1;
  }
  if (aliasStart === aliasEnd) {
    return null;
  }
  const alias = text.slice(aliasStart, aliasEnd);
  return { alias, symbol };
}
