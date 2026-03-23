import test from "node:test";
import assert from "node:assert/strict";
import {
	parseAlliumBlocks,
	parseAlliumBlocksWasm,
} from "../src/language-tools/parser";

/**
 * Compare regex-parsed blocks against WASM-parsed blocks for the same input.
 * Fields that may legitimately differ (exact body whitespace) are compared
 * with tolerance; structural fields must match exactly.
 */
function assertBlocksMatch(source: string) {
	const regex = parseAlliumBlocks(source);
	const wasm = parseAlliumBlocksWasm(source);

	assert.equal(
		wasm.length,
		regex.length,
		`block count mismatch: regex=${regex.length}, wasm=${wasm.length}`,
	);

	for (let i = 0; i < regex.length; i++) {
		const r = regex[i];
		const w = wasm[i];
		assert.equal(w.kind, r.kind, `block ${i} kind`);
		assert.equal(w.name, r.name, `block ${i} name`);
		assert.equal(w.startOffset, r.startOffset, `block ${i} startOffset`);
		assert.equal(
			w.bodyStartOffset,
			r.bodyStartOffset,
			`block ${i} bodyStartOffset`,
		);
		assert.equal(w.endOffset, r.endOffset, `block ${i} endOffset`);
		assert.equal(w.body, r.body, `block ${i} body`);
		assert.equal(w.sourcePath, r.sourcePath, `block ${i} sourcePath`);
		assert.equal(w.alias, r.alias, `block ${i} alias`);
	}
}

// --- Conformance: WASM adapter matches regex parser ---

test("adapter: empty file", () => {
	assertBlocksMatch("");
});

test("adapter: single entity", () => {
	assertBlocksMatch("entity Order {\n  total: Integer\n}");
});

test("adapter: entity with enum", () => {
	assertBlocksMatch(
		"entity Order {\n  status: pending | done\n}\n\nenum Priority { low | high }",
	);
});

test("adapter: rule with clauses", () => {
	assertBlocksMatch(
		"rule Confirm {\n  when: Confirm(order)\n  requires: order.status = pending\n  ensures: order.status = done\n}",
	);
});

test("adapter: surface and actor", () => {
	assertBlocksMatch(
		"surface Dashboard {\n  facing viewer: Admin\n  exposes: order.status\n}\n\nactor Admin {\n  identified_by: User.email\n}",
	);
});

test("adapter: config block", () => {
	assertBlocksMatch("config {\n  max_retries: Integer = 3\n}");
});

test("adapter: use statement", () => {
	assertBlocksMatch('use "./core.allium" as core');
});

test("adapter: contract and invariant", () => {
	assertBlocksMatch(
		"contract Codec {\n  encode: (value: Any) -> ByteArray\n}\n\ninvariant NonNeg {\n  total >= 0\n}",
	);
});

test("adapter: value type", () => {
	assertBlocksMatch("value Money {\n  amount: Decimal\n  currency: String\n}");
});

test("adapter: given block", () => {
	assertBlocksMatch("given {\n  item: Order\n}");
});

test("adapter: multiple declarations sorted by offset", () => {
	const source =
		"entity A {\n  x: Integer\n}\nrule B {\n  when: B()\n  ensures: Done()\n}\nenum C { a | b }";
	const wasm = parseAlliumBlocksWasm(source);
	for (let i = 1; i < wasm.length; i++) {
		assert.ok(
			wasm[i].startOffset > wasm[i - 1].startOffset,
			"blocks should be sorted by startOffset",
		);
	}
});

// --- v3-specific: WASM can parse constructs the regex parser cannot ---

test("wasm: v3 when-qualified field", () => {
	const source =
		"-- allium: 3\nentity Order {\n  status: pending | shipped\n  tracking: String when status = shipped\n}";
	const blocks = parseAlliumBlocksWasm(source);
	assert.equal(blocks.length, 1);
	assert.equal(blocks[0].kind, "entity");
	assert.ok(blocks[0].body.includes("tracking"));
});

test("wasm: v3 transitions block", () => {
	const source =
		"-- allium: 3\nentity Order {\n  status: pending | done\n  transitions status {\n    pending -> done\n    terminal: done\n  }\n}";
	const blocks = parseAlliumBlocksWasm(source);
	assert.equal(blocks.length, 1);
	assert.ok(blocks[0].body.includes("transitions"));
});

test("wasm: v3 backtick enum literal", () => {
	const source =
		"-- allium: 3\nenum Locale {\n  en | fr | `de-CH-1996`\n}";
	const blocks = parseAlliumBlocksWasm(source);
	assert.equal(blocks.length, 1);
	assert.equal(blocks[0].kind, "enum");
	assert.ok(blocks[0].body.includes("`de-CH-1996`"));
});

test("wasm: v3 invariant with for quantifier", () => {
	const source =
		"-- allium: 3\ninvariant AllPositive {\n  for a in Accounts:\n    a.balance >= 0\n}";
	const blocks = parseAlliumBlocksWasm(source);
	assert.equal(blocks.length, 1);
	assert.equal(blocks[0].kind, "invariant");
});
