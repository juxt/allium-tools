/**
 * Adapter: maps the WASM Rust AST into the ParsedBlock shape consumed by
 * existing language-tools modules. This allows incremental migration —
 * modules continue to receive ParsedBlock[] without rewriting.
 */

import type { ParsedBlock } from "./parser";
import type {
	WasmBlockDecl,
	WasmBlockKind,
	WasmDecl,
	WasmInvariantDecl,
	WasmParseResult,
	WasmStringPart,
	WasmUseDecl,
} from "./wasm-ast";

const BLOCK_KIND_MAP: Record<WasmBlockKind, ParsedBlock["kind"] | null> = {
	Entity: "entity",
	ExternalEntity: "entity",
	Value: "value",
	Enum: "enum",
	Given: "given",
	Config: "config",
	Rule: "rule",
	Surface: "surface",
	Actor: "actor",
	Contract: "contract",
	Invariant: "invariant",
};

export function wasmBlocksToParsedBlocks(
	source: string,
	result: WasmParseResult,
): ParsedBlock[] {
	const blocks: ParsedBlock[] = [];

	for (const decl of result.module.declarations) {
		const key = Object.keys(decl)[0] as keyof WasmDecl;

		if (key === "Block") {
			const block = (decl as { Block: WasmBlockDecl }).Block;
			const mapped = mapBlockDecl(source, block);
			if (mapped) {
				blocks.push(mapped);
			}
		} else if (key === "Use") {
			const use_ = (decl as { Use: WasmUseDecl }).Use;
			blocks.push(mapUseDecl(source, use_));
		} else if (key === "Invariant") {
			const inv = (decl as { Invariant: WasmInvariantDecl }).Invariant;
			blocks.push(mapInvariantDecl(source, inv));
		}
		// Default, Variant, Deferred, OpenQuestion are not represented as
		// ParsedBlock in the regex parser, so we skip them.
	}

	return blocks.sort((a, b) => a.startOffset - b.startOffset);
}

/**
 * Find the start offset matching the regex parser's `^\s*keyword` behaviour.
 *
 * With `/gm`, `^` matches at line boundaries. Then `\s*` can consume
 * whitespace across blank lines before the keyword. So we find the
 * earliest line-start position from which only whitespace leads to the
 * keyword.
 */
function blockStartOffset(source: string, spanStart: number): number {
	// Walk back to the start of the current line.
	let lineStart = spanStart;
	while (lineStart > 0 && source[lineStart - 1] !== "\n") {
		lineStart--;
	}
	// Check if the content from lineStart to spanStart is all whitespace.
	// If so, keep walking back across blank lines.
	while (lineStart > 0) {
		const prevLineEnd = lineStart;
		let prevLineStart = prevLineEnd - 1;
		while (prevLineStart > 0 && source[prevLineStart - 1] !== "\n") {
			prevLineStart--;
		}
		const lineContent = source.slice(prevLineStart, prevLineEnd);
		if (/^\s*$/.test(lineContent)) {
			lineStart = prevLineStart;
		} else {
			break;
		}
	}
	return lineStart;
}

function mapBlockDecl(
	source: string,
	block: WasmBlockDecl,
): ParsedBlock | null {
	const kind = BLOCK_KIND_MAP[block.kind];
	if (!kind) return null;

	const name = block.name?.name ?? kind;
	const nameStartOffset = block.name?.span.start ?? block.span.start;
	const startOffset = blockStartOffset(source, block.span.start);

	// Find the opening brace to determine body start.
	const braceOffset = source.indexOf("{", block.span.start);
	const bodyStartOffset =
		braceOffset >= 0 && braceOffset < block.span.end
			? braceOffset + 1
			: block.span.start;

	// Body is the raw text between braces, matching what the regex parser provides.
	const body = source.slice(bodyStartOffset, block.span.end - 1);

	return {
		kind,
		name,
		nameStartOffset,
		startOffset,
		bodyStartOffset,
		endOffset: block.span.end - 1,
		body,
	};
}

function mapInvariantDecl(
	source: string,
	inv: WasmInvariantDecl,
): ParsedBlock {
	const startOffset = blockStartOffset(source, inv.span.start);
	const braceOffset = source.indexOf("{", inv.span.start);
	const bodyStartOffset =
		braceOffset >= 0 && braceOffset < inv.span.end
			? braceOffset + 1
			: inv.span.start;
	const body = source.slice(bodyStartOffset, inv.span.end - 1);

	return {
		kind: "invariant",
		name: inv.name.name,
		nameStartOffset: inv.name.span.start,
		startOffset,
		bodyStartOffset,
		endOffset: inv.span.end - 1,
		body,
	};
}

function mapUseDecl(source: string, use_: WasmUseDecl): ParsedBlock {
	const alias = use_.alias?.name ?? "";
	const pathText = use_.path.parts
		.map((p: WasmStringPart) => {
			if ("Text" in p) return p.Text;
			if ("Interpolation" in p) return `\${${p.Interpolation.name}}`;
			return "";
		})
		.join("");

	return {
		kind: "use",
		name: alias,
		nameStartOffset: use_.alias?.span.start ?? use_.span.start,
		alias,
		sourcePath: pathText,
		startOffset: use_.span.start,
		bodyStartOffset: use_.span.start,
		endOffset: use_.span.end,
		body: "",
	};
}
