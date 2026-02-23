import test from "node:test";
import assert from "node:assert/strict";
import {
  applyDiagramFilters,
  buildDiagramResult,
  renderDiagram,
} from "../src/language-tools/diagram";

test("builds diagram model with entities, rules, and surfaces", () => {
  const model = buildDiagramResult(
    `entity Invitation {\n  status: pending | accepted\n}\n\nrule AcceptInvitation {\n  when: invitation: Invitation.status becomes pending\n  ensures: Invitation.created(status: accepted)\n}\n\nsurface InvitationPortal {\n  for user: User\n  context invitation: Invitation\n  provides:\n    AcceptInvitation(invitation)\n}\n`,
  ).model;

  assert.ok(model.nodes.some((n) => n.key === "entity:Invitation"));
  assert.ok(model.nodes.some((n) => n.key === "rule:AcceptInvitation"));
  assert.ok(model.nodes.some((n) => n.key === "surface:InvitationPortal"));
  assert.ok(
    model.edges.some(
      (e) => e.label === "when" && e.to === "rule_AcceptInvitation",
    ),
  );
  assert.ok(
    model.edges.some(
      (e) => e.label === "provides" && e.from === "surface_InvitationPortal",
    ),
  );
});

test("collects skipped declaration issues and module names", () => {
  const result = buildDiagramResult(
    `module onboarding\n\nconfig {\n  timeout: Integer = 10\n}\n\ndefault Role viewer = {}\n\nentity Role {\n  name: String\n}\n`,
  );

  assert.deepEqual(result.modules, ["onboarding"]);
  assert.ok(
    result.issues.some(
      (issue) => issue.code === "allium.diagram.skippedDeclaration",
    ),
  );
});

test("applies focus and kind filters", () => {
  const base = buildDiagramResult(
    `entity Invitation {\n  status: pending | accepted\n}\n\nentity Role {\n  name: String\n}\n\nrule AcceptInvitation {\n  when: AcceptInvitation(invitation)\n  ensures: Invitation.created(status: accepted)\n}\n`,
  ).model;

  const filtered = applyDiagramFilters(base, {
    focusNames: ["Invitation"],
    kinds: ["entity", "rule"],
  });

  assert.ok(filtered.nodes.every((node) => node.kind !== "trigger"));
  assert.ok(filtered.nodes.some((node) => node.id === "entity_Invitation"));
  assert.equal(
    filtered.nodes.some((node) => node.id === "entity_Role"),
    false,
  );
});

test("renders grouped d2 and mermaid output", () => {
  const model = buildDiagramResult(
    `entity Ticket {\n  status: open | closed\n}\nrule Close {\n  when: CloseTicket(ticket)\n  ensures: Ticket.created(status: closed)\n}\nsurface Console {\n  for user: User\n  provides:\n    CloseTicket(ticket)\n}\n`,
  ).model;

  const d2 = renderDiagram(model, "d2");
  const mermaid = renderDiagram(model, "mermaid");

  assert.match(d2, /entity_group: \{/);
  assert.match(d2, /rule_group: \{/);
  assert.match(
    d2,
    /surface_group\.surface_Console -> trigger_group\.trigger_CloseTicket: "provides"/,
  );
  assert.match(mermaid, /subgraph entity_group/);
  assert.match(mermaid, /subgraph rule_group/);
});
