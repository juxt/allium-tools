/**
 * TypeScript types mirroring the Rust AST from crates/allium-parser/src/ast.rs.
 *
 * These match the JSON shape produced by serde serialisation of the Rust types.
 * Rust enums serialise as externally tagged objects: { "VariantName": { ...fields } }.
 */

// ---------------------------------------------------------------------------
// WASM entry point
// ---------------------------------------------------------------------------

let _parse: ((source: string) => string) | undefined;

function getWasmParse(): (source: string) => string {
	if (!_parse) {
		// eslint-disable-next-line @typescript-eslint/no-require-imports
		_parse = require("allium-parser-wasm").parse;
	}
	return _parse!;
}

export function parseAllium(source: string): WasmParseResult {
	const json = getWasmParse()(source);
	return JSON.parse(json) as WasmParseResult;
}

// ---------------------------------------------------------------------------
// Top level
// ---------------------------------------------------------------------------

export interface WasmParseResult {
	module: WasmModule;
	diagnostics: WasmDiagnostic[];
}

export interface WasmModule {
	span: WasmSpan;
	version: number | null;
	declarations: WasmDecl[];
}

export interface WasmSpan {
	start: number;
	end: number;
}

export interface WasmDiagnostic {
	span: WasmSpan;
	message: string;
	severity: "Error" | "Warning";
}

// ---------------------------------------------------------------------------
// Declarations (externally tagged enum)
// ---------------------------------------------------------------------------

export type WasmDecl =
	| { Use: WasmUseDecl }
	| { Block: WasmBlockDecl }
	| { Default: WasmDefaultDecl }
	| { Variant: WasmVariantDecl }
	| { Deferred: WasmDeferredDecl }
	| { OpenQuestion: WasmOpenQuestionDecl }
	| { Invariant: WasmInvariantDecl };

export interface WasmUseDecl {
	span: WasmSpan;
	path: WasmStringLiteral;
	alias: WasmIdent | null;
}

export interface WasmBlockDecl {
	span: WasmSpan;
	kind: WasmBlockKind;
	name: WasmIdent | null;
	items: WasmBlockItem[];
}

export type WasmBlockKind =
	| "Entity"
	| "ExternalEntity"
	| "Value"
	| "Enum"
	| "Given"
	| "Config"
	| "Rule"
	| "Surface"
	| "Actor"
	| "Contract"
	| "Invariant";

export interface WasmDefaultDecl {
	span: WasmSpan;
	type_name: WasmIdent | null;
	name: WasmIdent;
	value: WasmExpr;
}

export interface WasmVariantDecl {
	span: WasmSpan;
	name: WasmIdent;
	base: WasmExpr;
	items: WasmBlockItem[];
}

export interface WasmDeferredDecl {
	span: WasmSpan;
	path: WasmExpr;
}

export interface WasmOpenQuestionDecl {
	span: WasmSpan;
	text: WasmStringLiteral;
}

export interface WasmInvariantDecl {
	span: WasmSpan;
	name: WasmIdent;
	body: WasmExpr;
}

// ---------------------------------------------------------------------------
// Block items (externally tagged enum for kind)
// ---------------------------------------------------------------------------

export interface WasmBlockItem {
	span: WasmSpan;
	kind: WasmBlockItemKind;
}

export type WasmBlockItemKind =
	| { Clause: { keyword: string; value: WasmExpr } }
	| { Assignment: { name: WasmIdent; value: WasmExpr } }
	| {
			ParamAssignment: {
				name: WasmIdent;
				params: WasmIdent[];
				value: WasmExpr;
			};
		}
	| { Let: { name: WasmIdent; value: WasmExpr } }
	| { EnumVariant: { name: WasmIdent; backtick_quoted: boolean } }
	| {
			ForBlock: {
				binding: WasmForBinding;
				collection: WasmExpr;
				filter: WasmExpr | null;
				items: WasmBlockItem[];
			};
		}
	| {
			IfBlock: {
				branches: WasmCondBlockBranch[];
				else_items: WasmBlockItem[] | null;
			};
		}
	| { PathAssignment: { path: WasmExpr; value: WasmExpr } }
	| { OpenQuestion: { text: WasmStringLiteral } }
	| { ContractsClause: { entries: WasmContractBinding[] } }
	| { Annotation: WasmAnnotation }
	| { InvariantBlock: { name: WasmIdent; body: WasmExpr } }
	| { TransitionsBlock: WasmTransitionGraph }
	| {
			FieldWithWhen: {
				name: WasmIdent;
				value: WasmExpr;
				when_clause: WasmWhenClause;
			};
		};

// ---------------------------------------------------------------------------
// Transition graphs and when clauses (v3)
// ---------------------------------------------------------------------------

export interface WasmTransitionGraph {
	span: WasmSpan;
	field: WasmIdent;
	edges: WasmTransitionEdge[];
	terminal: WasmIdent[];
}

export interface WasmTransitionEdge {
	span: WasmSpan;
	from: WasmIdent;
	to: WasmIdent;
}

export interface WasmWhenClause {
	span: WasmSpan;
	status_field: WasmIdent;
	qualifying_states: WasmIdent[];
}

// ---------------------------------------------------------------------------
// Contracts and annotations
// ---------------------------------------------------------------------------

export interface WasmContractBinding {
	direction: "Demands" | "Fulfils";
	name: WasmIdent;
	span: WasmSpan;
}

export interface WasmAnnotation {
	kind: "Invariant" | "Guidance" | "Guarantee";
	name: WasmIdent | null;
	body: string[];
	span: WasmSpan;
}

// ---------------------------------------------------------------------------
// Conditional block branches
// ---------------------------------------------------------------------------

export interface WasmCondBlockBranch {
	span: WasmSpan;
	condition: WasmExpr;
	items: WasmBlockItem[];
}

// ---------------------------------------------------------------------------
// Expressions (externally tagged enum)
// ---------------------------------------------------------------------------

export type WasmExpr =
	| { Ident: WasmIdent }
	| { StringLiteral: WasmStringLiteral }
	| { BacktickLiteral: { span: WasmSpan; value: string } }
	| { NumberLiteral: { span: WasmSpan; value: string } }
	| { BoolLiteral: { span: WasmSpan; value: boolean } }
	| { Null: { span: WasmSpan } }
	| { Now: { span: WasmSpan } }
	| { This: { span: WasmSpan } }
	| { Within: { span: WasmSpan } }
	| { DurationLiteral: { span: WasmSpan; value: string } }
	| { SetLiteral: { span: WasmSpan; elements: WasmExpr[] } }
	| { ObjectLiteral: { span: WasmSpan; fields: WasmNamedArg[] } }
	| { GenericType: { span: WasmSpan; name: WasmExpr; args: WasmExpr[] } }
	| { MemberAccess: { span: WasmSpan; object: WasmExpr; field: WasmIdent } }
	| { OptionalAccess: { span: WasmSpan; object: WasmExpr; field: WasmIdent } }
	| { NullCoalesce: { span: WasmSpan; left: WasmExpr; right: WasmExpr } }
	| {
			Call: {
				span: WasmSpan;
				function: WasmExpr;
				args: WasmCallArg[];
			};
		}
	| {
			JoinLookup: {
				span: WasmSpan;
				entity: WasmExpr;
				fields: WasmJoinField[];
			};
		}
	| {
			BinaryOp: {
				span: WasmSpan;
				left: WasmExpr;
				op: WasmBinaryOp;
				right: WasmExpr;
			};
		}
	| {
			Comparison: {
				span: WasmSpan;
				left: WasmExpr;
				op: WasmComparisonOp;
				right: WasmExpr;
			};
		}
	| {
			LogicalOp: {
				span: WasmSpan;
				left: WasmExpr;
				op: WasmLogicalOp;
				right: WasmExpr;
			};
		}
	| { Not: { span: WasmSpan; operand: WasmExpr } }
	| { In: { span: WasmSpan; element: WasmExpr; collection: WasmExpr } }
	| { NotIn: { span: WasmSpan; element: WasmExpr; collection: WasmExpr } }
	| { Exists: { span: WasmSpan; operand: WasmExpr } }
	| { NotExists: { span: WasmSpan; operand: WasmExpr } }
	| { Where: { span: WasmSpan; source: WasmExpr; condition: WasmExpr } }
	| { With: { span: WasmSpan; source: WasmExpr; predicate: WasmExpr } }
	| { Pipe: { span: WasmSpan; left: WasmExpr; right: WasmExpr } }
	| { Lambda: { span: WasmSpan; param: WasmExpr; body: WasmExpr } }
	| {
			Conditional: {
				span: WasmSpan;
				branches: WasmCondBranch[];
				else_body: WasmExpr | null;
			};
		}
	| {
			For: {
				span: WasmSpan;
				binding: WasmForBinding;
				collection: WasmExpr;
				filter: WasmExpr | null;
				body: WasmExpr;
			};
		}
	| { ProjectionMap: { span: WasmSpan; source: WasmExpr; field: WasmIdent } }
	| {
			TransitionsTo: {
				span: WasmSpan;
				subject: WasmExpr;
				new_state: WasmExpr;
			};
		}
	| { Becomes: { span: WasmSpan; subject: WasmExpr; new_state: WasmExpr } }
	| { Binding: { span: WasmSpan; name: WasmIdent; value: WasmExpr } }
	| { WhenGuard: { span: WasmSpan; action: WasmExpr; condition: WasmExpr } }
	| { TypeOptional: { span: WasmSpan; inner: WasmExpr } }
	| { LetExpr: { span: WasmSpan; name: WasmIdent; value: WasmExpr } }
	| { QualifiedName: WasmQualifiedName }
	| { Block: { span: WasmSpan; items: WasmExpr[] } };

export interface WasmCondBranch {
	span: WasmSpan;
	condition: WasmExpr;
	body: WasmExpr;
}

export type WasmForBinding =
	| { Single: WasmIdent }
	| { Destructured: [WasmIdent[], WasmSpan] };

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

export interface WasmIdent {
	span: WasmSpan;
	name: string;
}

export interface WasmQualifiedName {
	span: WasmSpan;
	qualifier: string | null;
	name: string;
}

export interface WasmStringLiteral {
	span: WasmSpan;
	parts: WasmStringPart[];
}

export type WasmStringPart =
	| { Text: string }
	| { Interpolation: WasmIdent };

export interface WasmNamedArg {
	span: WasmSpan;
	name: WasmIdent;
	value: WasmExpr;
}

export type WasmCallArg =
	| { Positional: WasmExpr }
	| { Named: WasmNamedArg };

export interface WasmJoinField {
	span: WasmSpan;
	field: WasmIdent;
	value: WasmExpr | null;
}

export type WasmBinaryOp = "Add" | "Sub" | "Mul" | "Div";
export type WasmComparisonOp = "Eq" | "NotEq" | "Lt" | "LtEq" | "Gt" | "GtEq";
export type WasmLogicalOp = "And" | "Or" | "Implies";

// ---------------------------------------------------------------------------
// Helpers for working with externally-tagged enums
// ---------------------------------------------------------------------------

/** Extract the variant key from a serde externally-tagged enum value. */
export function declKind(decl: WasmDecl): string {
	return Object.keys(decl)[0];
}

/** Extract the payload of a serde externally-tagged enum value. */
export function declPayload<K extends keyof WasmDecl>(
	decl: WasmDecl & Record<K, unknown>,
): WasmDecl[K] {
	const key = Object.keys(decl)[0] as K;
	return (decl as Record<K, unknown>)[key] as WasmDecl[K];
}

/** Extract the variant key from a BlockItemKind. */
export function itemKindTag(kind: WasmBlockItemKind): string {
	return Object.keys(kind)[0];
}
