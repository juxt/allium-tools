import test from "node:test";
import assert from "node:assert/strict";
import { parseAlliumBlocks } from "../src/language-tools/parser";

// --- Structural: WASM parser produces correct ParsedBlock output ---

test("parser: empty file", () => {
	const blocks = parseAlliumBlocks("");
	assert.equal(blocks.length, 0);
});

test("parser: single entity", () => {
	const blocks = parseAlliumBlocks("entity Order {\n  total: Integer\n}");
	assert.equal(blocks.length, 1);
	assert.equal(blocks[0].kind, "entity");
	assert.equal(blocks[0].name, "Order");
	assert.ok(blocks[0].body.includes("total"));
});

test("parser: entity with enum", () => {
	const blocks = parseAlliumBlocks(
		"entity Order {\n  status: pending | done\n}\n\nenum Priority { low | high }",
	);
	assert.equal(blocks.length, 2);
	assert.equal(blocks[0].kind, "entity");
	assert.equal(blocks[1].kind, "enum");
});

test("parser: rule with clauses", () => {
	const blocks = parseAlliumBlocks(
		"rule Confirm {\n  when: Confirm(order)\n  requires: order.status = pending\n  ensures: order.status = done\n}",
	);
	assert.equal(blocks.length, 1);
	assert.equal(blocks[0].kind, "rule");
	assert.equal(blocks[0].name, "Confirm");
	assert.ok(blocks[0].body.includes("requires"));
});

test("parser: surface and actor", () => {
	const blocks = parseAlliumBlocks(
		"surface Dashboard {\n  facing viewer: Admin\n  exposes: order.status\n}\n\nactor Admin {\n  identified_by: User.email\n}",
	);
	assert.equal(blocks.length, 2);
	assert.equal(blocks[0].kind, "surface");
	assert.equal(blocks[1].kind, "actor");
});

test("parser: config block", () => {
	const blocks = parseAlliumBlocks("config {\n  max_retries: Integer = 3\n}");
	assert.equal(blocks.length, 1);
	assert.equal(blocks[0].kind, "config");
});

test("parser: use statement", () => {
	const blocks = parseAlliumBlocks('use "./core.allium" as core');
	assert.equal(blocks.length, 1);
	assert.equal(blocks[0].kind, "use");
	assert.equal(blocks[0].alias, "core");
	assert.equal(blocks[0].sourcePath, "./core.allium");
});

test("parser: contract and invariant", () => {
	const blocks = parseAlliumBlocks(
		"contract Codec {\n  encode: (value: Any) -> ByteArray\n}\n\ninvariant NonNeg {\n  total >= 0\n}",
	);
	assert.equal(blocks.length, 2);
	assert.equal(blocks[0].kind, "contract");
	assert.equal(blocks[1].kind, "invariant");
});

test("parser: value type", () => {
	const blocks = parseAlliumBlocks(
		"value Money {\n  amount: Decimal\n  currency: String\n}",
	);
	assert.equal(blocks.length, 1);
	assert.equal(blocks[0].kind, "value");
});

test("parser: given block", () => {
	const blocks = parseAlliumBlocks("given {\n  item: Order\n}");
	assert.equal(blocks.length, 1);
	assert.equal(blocks[0].kind, "given");
});

test("parser: blocks sorted by offset", () => {
	const source =
		"entity A {\n  x: Integer\n}\nrule B {\n  when: B()\n  ensures: Done()\n}\nenum C { a | b }";
	const blocks = parseAlliumBlocks(source);
	for (let i = 1; i < blocks.length; i++) {
		assert.ok(
			blocks[i].startOffset > blocks[i - 1].startOffset,
			"blocks should be sorted by startOffset",
		);
	}
});

// --- v3-specific constructs ---

test("parser: v3 when-qualified field", () => {
	const source =
		"-- allium: 3\nentity Order {\n  status: pending | shipped\n  tracking: String when status = shipped\n}";
	const blocks = parseAlliumBlocks(source);
	assert.equal(blocks.length, 1);
	assert.equal(blocks[0].kind, "entity");
	assert.ok(blocks[0].body.includes("tracking"));
});

test("parser: v3 transitions block", () => {
	const source =
		"-- allium: 3\nentity Order {\n  status: pending | done\n  transitions status {\n    pending -> done\n    terminal: done\n  }\n}";
	const blocks = parseAlliumBlocks(source);
	assert.equal(blocks.length, 1);
	assert.ok(blocks[0].body.includes("transitions"));
});

test("parser: v3 backtick enum literal", () => {
	const source =
		"-- allium: 3\nenum Locale {\n  en | fr | `de-CH-1996`\n}";
	const blocks = parseAlliumBlocks(source);
	assert.equal(blocks.length, 1);
	assert.equal(blocks[0].kind, "enum");
	assert.ok(blocks[0].body.includes("`de-CH-1996`"));
});

test("parser: v3 invariant with for quantifier", () => {
	const source =
		"-- allium: 3\ninvariant AllPositive {\n  for a in Accounts:\n    a.balance >= 0\n}";
	const blocks = parseAlliumBlocks(source);
	assert.equal(blocks.length, 1);
	assert.equal(blocks[0].kind, "invariant");
});
