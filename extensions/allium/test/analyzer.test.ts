import test from "node:test";
import assert from "node:assert/strict";
import { analyzeAllium } from "../src/language-tools/analyzer";

test("reports missing ensures", () => {
  const findings = analyzeAllium(`rule A {\n  when: Ping()\n}`);
  assert.ok(findings.some((f) => f.code === "allium.rule.missingEnsures"));
});

test("reports missing when trigger", () => {
  const findings = analyzeAllium(`rule A {\n  ensures: Done()\n}`);
  const finding = findings.find((f) => f.code === "allium.rule.missingWhen");
  assert.ok(finding);
  assert.equal(finding?.start.line, 0);
  assert.equal(finding?.start.character, 5);
  assert.equal(finding?.end.line, 0);
  assert.equal(finding?.end.character, 6);
});

test("reports invalid trigger shape in when clause", () => {
  const findings = analyzeAllium(
    `rule A {\n  when: totally invalid trigger\n  ensures: Done()\n}`,
  );
  assert.ok(findings.some((f) => f.code === "allium.rule.invalidTrigger"));
});

test("accepts valid state-transition trigger shape", () => {
  const findings = analyzeAllium(
    `rule A {\n  when: invitation: Invitation.status becomes accepted\n  ensures: Done()\n}`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.rule.invalidTrigger"),
    false,
  );
});

test("accepts combined external-stimulus triggers joined by or", () => {
  const findings = analyzeAllium(
    `rule A {\n  when: Opened(doc) or Saved(doc)\n  ensures: Done()\n}`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.rule.invalidTrigger"),
    false,
  );
});

test("reports temporal trigger without guard", () => {
  const findings = analyzeAllium(
    `rule Expires {\n  when: invitation: Invitation.expires_at <= now\n  ensures: invitation.status = expired\n}`,
  );
  assert.ok(findings.some((f) => f.code === "allium.temporal.missingGuard"));
});

test("does not report temporal guard if requires exists", () => {
  const findings = analyzeAllium(
    `rule Expires {\n  when: invitation: Invitation.expires_at <= now\n  requires: invitation.status = pending\n  ensures: invitation.status = expired\n}`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.temporal.missingGuard"),
    false,
  );
});

test("reports duplicate config keys", () => {
  const findings = analyzeAllium(
    `config {\n  timeout: Integer = 1\n  timeout: Integer = 2\n}`,
  );
  assert.ok(findings.some((f) => f.code === "allium.config.duplicateKey"));
});

test("reports duplicate named default instances", () => {
  const findings = analyzeAllium(
    `default Role viewer = {\n  name: "viewer"\n}\n\ndefault Role viewer = {\n  name: "viewer_v2"\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.default.duplicateName"));
});

test("does not report distinct named default instances", () => {
  const findings = analyzeAllium(
    `default Role viewer = {\n  name: "viewer"\n}\n\ndefault Role editor = {\n  name: "editor"\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.default.duplicateName"),
    false,
  );
});

test("reports undefined type in default declaration", () => {
  const findings = analyzeAllium(`default MissingType item = {\n}\n`);
  assert.ok(findings.some((f) => f.code === "allium.default.undefinedType"));
});

test("does not report defined type in default declaration", () => {
  const findings = analyzeAllium(
    `entity Role {\n  name: String\n}\n\ndefault Role viewer = {\n  name: "viewer"\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.default.undefinedType"),
    false,
  );
});

test("reports duplicate let bindings", () => {
  const findings = analyzeAllium(
    `rule A {\n  when: Ping()\n  let x = 1\n  let x = 2\n  ensures: Done()\n}`,
  );
  assert.ok(findings.some((f) => f.code === "allium.let.duplicateBinding"));
});

test("warns when requires clauses are contradictory", () => {
  const findings = analyzeAllium(
    `rule A {\n  when: Ping(user)\n  requires: user.status = active\n  requires: user.status = suspended\n  ensures: Done()\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.rule.neverFires"));
});

test("does not warn when requires clauses are compatible", () => {
  const findings = analyzeAllium(
    `rule A {\n  when: Ping(user)\n  requires: user.status = active\n  requires: user.region != blocked\n  ensures: Done()\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.rule.neverFires"),
    false,
  );
});

test("reports expression type mismatch for ordered comparison with string", () => {
  const findings = analyzeAllium(
    `rule A {\n  when: Ping(user)\n  requires: user.age < "old"\n  ensures: Done()\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.expression.typeMismatch"));
});

test("reports expression type mismatch for arithmetic with string", () => {
  const findings = analyzeAllium(
    `rule A {\n  when: Ping(user)\n  ensures: user.score = "bad" + 1\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.expression.typeMismatch"));
});

test("reports circular dependencies between derived entity values", () => {
  const findings = analyzeAllium(`entity Stats {\n  a: b + 1\n  b: a + 1\n}\n`);
  assert.ok(
    findings.some((f) => f.code === "allium.derived.circularDependency"),
  );
});

test("reports undefined config reference", () => {
  const findings = analyzeAllium(
    `rule A {\n  when: Ping()\n  ensures: now + config.missing\n}`,
  );
  assert.ok(
    findings.some((f) => f.code === "allium.config.undefinedReference"),
  );
});

test("reports open question as warning finding", () => {
  const findings = analyzeAllium(`open question "Needs decision?"`);
  const finding = findings.find(
    (f) => f.code === "allium.openQuestion.present",
  );
  assert.ok(finding);
  assert.equal(finding.severity, "warning");
});

test("relaxed mode suppresses temporal guard warning", () => {
  const findings = analyzeAllium(
    `rule Expires {\n  when: invitation: Invitation.expires_at <= now\n  ensures: invitation.status = expired\n}`,
    { mode: "relaxed" },
  );
  assert.equal(
    findings.some((f) => f.code === "allium.temporal.missingGuard"),
    false,
  );
});

test("relaxed mode downgrades undefined config severity", () => {
  const findings = analyzeAllium(
    `rule A {\n  when: Ping()\n  ensures: now + config.missing\n}`,
    { mode: "relaxed" },
  );
  const finding = findings.find(
    (f) => f.code === "allium.config.undefinedReference",
  );
  assert.ok(finding);
  assert.equal(finding.severity, "info");
});

test("reports missing actor referenced by surface", () => {
  const findings = analyzeAllium(
    `surface ChildView {\n  facing parent: Parent\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.surface.missingActor"));
});

test("reports unused actor when not referenced by any surface", () => {
  const findings = analyzeAllium(
    `actor Parent {\n  identified_by: User.email\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.actor.unused"));
});

test("suppresses finding using allium-ignore on previous line", () => {
  const findings = analyzeAllium(
    `rule A {\n  when: Ping()\n  -- allium-ignore allium.config.undefinedReference\n  ensures: now + config.missing\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.config.undefinedReference"),
    false,
  );
});

test("does not treat config references inside comments as findings", () => {
  const findings = analyzeAllium(
    `rule A {\n  when: Ping()\n  -- config.missing\n  ensures: Done()\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.config.undefinedReference"),
    false,
  );
});

test("reports implicit lambda shorthand in collection operators", () => {
  const findings = analyzeAllium(
    `rule A {\n  when: Ping()\n  requires: users.any(can_solo)\n  ensures: Done()\n}\n`,
  );
  assert.ok(
    findings.some((f) => f.code === "allium.expression.implicitLambda"),
  );
});

test("does not report explicit lambda in collection operators", () => {
  const findings = analyzeAllium(
    `rule A {\n  when: Ping()\n  requires: users.any(u => u.can_solo)\n  ensures: Done()\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.expression.implicitLambda"),
    false,
  );
});

test("reports duplicate enum literals", () => {
  const findings = analyzeAllium(
    `enum Recommendation {\n  yes | no | yes\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.enum.duplicateLiteral"));
});

test("reports empty enum declarations", () => {
  const findings = analyzeAllium(`enum Recommendation {\n}\n`);
  assert.ok(findings.some((f) => f.code === "allium.enum.empty"));
});

test("reports duplicate context binding names", () => {
  const findings = analyzeAllium(
    `entity Pipeline {\n  status: String\n}\n\ngiven {\n  pipeline: Pipeline\n  pipeline: Pipeline\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.context.duplicateBinding"));
});

test("reports undefined context binding type", () => {
  const findings = analyzeAllium(`given {\n  pipeline: MissingType\n}\n`);
  assert.ok(findings.some((f) => f.code === "allium.context.undefinedType"));
});

test("does not report context type for imported alias reference", () => {
  const findings = analyzeAllium(
    `use "./shared.allium" as scheduling\n\ngiven {\n  calendar: scheduling/calendar\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.context.undefinedType"),
    false,
  );
});

test("reports undefined related surface reference", () => {
  const findings = analyzeAllium(
    `surface Dashboard {\n  facing user: User\n  related:\n    MissingSurface\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.surface.relatedUndefined"));
});

test("does not report related surface when declared", () => {
  const findings = analyzeAllium(
    `surface Dashboard {\n  facing user: User\n  related:\n    DetailView\n}\n\nsurface DetailView {\n  facing user: User\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.surface.relatedUndefined"),
    false,
  );
});

test("reports unused surface facing-binding", () => {
  const findings = analyzeAllium(
    `surface Dashboard {\n  facing viewer: User\n  exposes:\n    System.status\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.surface.unusedBinding"));
});

test("does not report used surface bindings", () => {
  const findings = analyzeAllium(
    `surface Dashboard {\n  facing viewer: User\n  context assignment: SlotConfirmation\n  exposes:\n    assignment.status\n  provides:\n    DashboardViewed(viewer: viewer)\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.surface.unusedBinding"),
    false,
  );
});

test("reports undefined field path in surface clauses", () => {
  const findings = analyzeAllium(
    `entity SlotConfirmation {\n  status: String\n}\n\nsurface Dashboard {\n  facing viewer: User\n  context assignment: SlotConfirmation\n  exposes:\n    assignment.missing_field\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.surface.undefinedPath"));
});

test("does not report valid field path in surface clauses", () => {
  const findings = analyzeAllium(
    `entity SlotConfirmation {\n  status: String\n}\n\nsurface Dashboard {\n  facing viewer: User\n  context assignment: SlotConfirmation\n  exposes:\n    assignment.status\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.surface.undefinedPath"),
    false,
  );
});

test("reports surface iteration over non-collection expression", () => {
  const findings = analyzeAllium(
    `entity SlotConfirmation {\n  status: String\n}\n\nsurface Dashboard {\n  facing viewer: User\n  context assignment: SlotConfirmation\n  exposes:\n    for item in assignment.status:\n      item\n}\n`,
  );
  assert.ok(
    findings.some((f) => f.code === "allium.surface.nonCollectionIteration"),
  );
});

test("does not report surface iteration over collection expression", () => {
  const findings = analyzeAllium(
    `entity SlotConfirmation {\n  statuses: List<String>\n}\n\nsurface Dashboard {\n  facing viewer: User\n  context assignment: SlotConfirmation\n  exposes:\n    for item in assignment.statuses:\n      item\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.surface.nonCollectionIteration"),
    false,
  );
});

test("reports surface path not observed in rule references", () => {
  const findings = analyzeAllium(
    `entity SlotConfirmation {\n  status: String\n  score: Integer\n}\n\nrule KeepStatus {\n  when: assignment: SlotConfirmation.created\n  ensures: assignment.status = active\n}\n\nsurface Dashboard {\n  facing viewer: User\n  context assignment: SlotConfirmation\n  exposes:\n    assignment.score\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.surface.unusedPath"));
});

test("warns on contradictory surface when condition", () => {
  const findings = analyzeAllium(
    `surface Dashboard {\n  facing viewer: User\n  provides:\n    Opened()\n      when viewer.status = active and viewer.status = suspended\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.surface.impossibleWhen"));
});

test("reports config parameter missing explicit type/default", () => {
  const findings = analyzeAllium(`config {\n  timeout: Integer\n}\n`);
  assert.ok(findings.some((f) => f.code === "allium.config.invalidParameter"));
});

test("reports unknown alias in external config reference", () => {
  const findings = analyzeAllium(
    `rule A {\n  when: Ping()\n  ensures: now + oauth/config.session_duration\n}\n`,
  );
  assert.ok(
    findings.some((f) => f.code === "allium.config.undefinedExternalReference"),
  );
});

test("does not report known alias in external config reference", () => {
  const findings = analyzeAllium(
    `use "./oauth.allium" as oauth\n\nrule A {\n  when: Ping()\n  ensures: now + oauth/config.session_duration\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.config.undefinedExternalReference"),
    false,
  );
});

test("reports discriminator references without matching variant declarations", () => {
  const findings = analyzeAllium(
    `entity Node {\n  kind: Branch | Leaf\n}\n\nvariant Branch : Node {\n  children: List<Node>\n}\n`,
  );
  assert.ok(
    findings.some((f) => f.code === "allium.sum.discriminatorUnknownVariant"),
  );
});

test("reports variant missing from base discriminator field", () => {
  const findings = analyzeAllium(
    `entity Node {\n  kind: Branch | Leaf\n}\n\nvariant Branch : Node {\n  children: List<Node>\n}\nvariant Trunk : Node {\n  rings: Integer\n}\nvariant Leaf : Node {\n  data: String\n}\n`,
  );
  assert.ok(
    findings.some((f) => f.code === "allium.sum.variantMissingInDiscriminator"),
  );
});

test("reports direct base instantiation for sum type entity", () => {
  const findings = analyzeAllium(
    `entity Node {\n  kind: Branch | Leaf\n}\n\nvariant Branch : Node {\n  children: List<Node>\n}\nvariant Leaf : Node {\n  data: String\n}\n\nrule CreateNode {\n  when: Ping()\n  ensures: Node.created(kind: Branch)\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.sum.baseInstantiation"));
});

test("reports variant-like declaration missing keyword", () => {
  const findings = analyzeAllium(
    `entity Node {\n  kind: Branch | Leaf\n}\n\nBranch : Node {\n  children: List<Node>\n}\n`,
  );
  assert.ok(
    findings.some((f) => f.code === "allium.sum.missingVariantKeyword"),
  );
});

test("reports unguarded variant-specific field access", () => {
  const findings = analyzeAllium(
    `entity Node {\n  kind: Branch | Leaf\n}\n\nvariant Branch : Node {\n  children: List<Node>\n}\n\nvariant Leaf : Node {\n  value: String\n}\n\nrule Invalid {\n  when: node: Node.created\n  ensures: Results.created(size: node.children.count)\n}\n`,
  );
  assert.ok(
    findings.some((f) => f.code === "allium.sum.unguardedVariantFieldAccess"),
  );
});

test("does not report guarded variant-specific field access", () => {
  const findings = analyzeAllium(
    `entity Node {\n  kind: Branch | Leaf\n}\n\nvariant Branch : Node {\n  children: List<Node>\n}\n\nvariant Leaf : Node {\n  value: String\n}\n\nrule Valid {\n  when: node: Node.created\n  requires: node.kind = Branch\n  ensures: Results.created(size: node.children.count)\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.sum.unguardedVariantFieldAccess"),
    false,
  );
});

test("reports undefined local type reference in entity field", () => {
  const findings = analyzeAllium(
    `entity Invitation {\n  policy: MissingPolicy\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.type.undefinedReference"));
});

test("does not report declared local type references", () => {
  const findings = analyzeAllium(
    `value Policy {\n  retries: Integer\n}\n\nentity Invitation {\n  policy: Policy\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.type.undefinedReference"),
    false,
  );
});

test("reports unknown imported alias in type reference", () => {
  const findings = analyzeAllium(
    `entity Invitation {\n  policy: shared/Policy\n}\n`,
  );
  assert.ok(
    findings.some((f) => f.code === "allium.type.undefinedImportedAlias"),
  );
});

test("does not report known imported alias in type reference", () => {
  const findings = analyzeAllium(
    `use "./shared.allium" as shared\n\nentity Invitation {\n  policy: shared/Policy\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.type.undefinedImportedAlias"),
    false,
  );
});

test("does not treat slash in inline comment as imported alias", () => {
  const findings = analyzeAllium(
    `entity Version {\n  is_display_version: Boolean     -- true for the ordered/booked version, or latest\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.type.undefinedImportedAlias"),
    false,
  );
});

test("still reports unknown alias when not in a comment", () => {
  const findings = analyzeAllium(
    `entity Version {\n  policy: ordered/Policy\n}\n`,
  );
  assert.ok(
    findings.some((f) => f.code === "allium.type.undefinedImportedAlias"),
  );
});

test("strips inline comment before checking field type references", () => {
  const findings = analyzeAllium(
    `entity Order {\n  status: String  -- the current status\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.type.undefinedReference"),
    false,
  );
});

test("reports undefined relationship target type", () => {
  const findings = analyzeAllium(
    `entity Order {\n  items: LineItem for this order\n}\n`,
  );
  assert.ok(
    findings.some((f) => f.code === "allium.relationship.undefinedTarget"),
  );
});

test("reports non-singular relationship target type name", () => {
  const findings = analyzeAllium(
    `entity User {\n}\nentity Team {\n  members: Users for this team\n}\nentity Users {\n  id: String\n}\n`,
  );
  assert.ok(
    findings.some((f) => f.code === "allium.relationship.nonSingularTarget"),
  );
});

test("does not report valid singular relationship target", () => {
  const findings = analyzeAllium(
    `entity User {\n  id: String\n}\nentity Team {\n  members: User for this team\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.relationship.undefinedTarget"),
    false,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.relationship.nonSingularTarget"),
    false,
  );
});

test("reports duplicate named requires blocks in surface", () => {
  const findings = analyzeAllium(
    `surface Dashboard {\n  facing viewer: User\n  requires Visibility:\n    viewer.id != null\n  requires Visibility:\n    viewer.active = true\n}\n`,
  );
  assert.ok(
    findings.some((f) => f.code === "allium.surface.duplicateRequiresBlock"),
  );
});

test("warns for named requires block without deferred hint", () => {
  const findings = analyzeAllium(
    `surface Dashboard {\n  facing viewer: User\n  requires ApprovalFlow:\n    viewer.id != null\n}\n`,
  );
  const finding = findings.find(
    (f) => f.code === "allium.surface.requiresWithoutDeferred",
  );
  assert.ok(finding);
  assert.equal(finding.severity, "warning");
});

test("does not warn when named requires block has deferred hint", () => {
  const findings = analyzeAllium(
    `deferred Dashboard.ApprovalFlow\n\nsurface Dashboard {\n  facing viewer: User\n  requires ApprovalFlow:\n    viewer.id != null\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.surface.requiresWithoutDeferred"),
    false,
  );
});

test("reports duplicate named provides blocks in surface", () => {
  const findings = analyzeAllium(
    `surface Dashboard {\n  facing viewer: User\n  provides Navigate:\n    Opened()\n  provides Navigate:\n    Refreshed()\n}\n`,
  );
  assert.ok(
    findings.some((f) => f.code === "allium.surface.duplicateProvidesBlock"),
  );
});

test("reports undefined trigger type reference in rule", () => {
  const findings = analyzeAllium(
    `rule Expire {\n  when: invite: MissingType.expires_at <= now\n  ensures: Done()\n}\n`,
  );
  assert.ok(
    findings.some((f) => f.code === "allium.rule.undefinedTypeReference"),
  );
});

test("reports undefined imported alias in rule type reference", () => {
  const findings = analyzeAllium(
    `rule Expire {\n  when: invite: shared/Invite.expires_at <= now\n  ensures: Done()\n}\n`,
  );
  assert.ok(
    findings.some((f) => f.code === "allium.rule.undefinedImportedAlias"),
  );
});

test("does not report known rule type references", () => {
  const findings = analyzeAllium(
    `entity Invite {\n  expires_at: Timestamp\n}\n\nrule Expire {\n  when: invite: Invite.expires_at <= now\n  ensures: Invite.created(expires_at: now)\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.rule.undefinedTypeReference"),
    false,
  );
});

test("reports undefined rule binding used in dotted reference", () => {
  const findings = analyzeAllium(
    `rule Notify {\n  when: Ping()\n  requires: user.status = active\n  ensures: Done()\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.rule.undefinedBinding"));
});

test("does not report binding defined by trigger parameter", () => {
  const findings = analyzeAllium(
    `rule Notify {\n  when: UserUpdated(user)\n  requires: user.status = active\n  ensures: Done()\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.rule.undefinedBinding"),
    false,
  );
});

test("does not report binding defined in context block", () => {
  const findings = analyzeAllium(
    `entity User {\n  status: String\n}\n\ngiven {\n  user: User\n}\n\nrule Notify {\n  when: Ping()\n  requires: user.status = active\n  ensures: Done()\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.rule.undefinedBinding"),
    false,
  );
});

test("does not report lambda parameter as undefined binding", () => {
  const findings = analyzeAllium(
    `rule Check {\n  when: Ping(users)\n  requires: users.any(u => u.active)\n  ensures: Done()\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.rule.undefinedBinding"),
    false,
  );
});

test("reports undefined binding in exists expression", () => {
  const findings = analyzeAllium(
    `rule Check {\n  when: Ping()\n  requires: exists candidate\n  ensures: Done()\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.rule.undefinedBinding"));
});

test("reports undefined binding in for-in source", () => {
  const findings = analyzeAllium(
    `rule Iterate {\n  when: Ping()\n  for item in items:\n  ensures: Done()\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.rule.undefinedBinding"));
});

test("reports entity declared but never referenced", () => {
  const findings = analyzeAllium(`entity Invitation {\n  status: String\n}\n`);
  assert.ok(findings.some((f) => f.code === "allium.entity.unused"));
});

test("reports unused named value declarations", () => {
  const findings = analyzeAllium(`value Amount {\n  cents: Integer\n}\n`);
  assert.ok(findings.some((f) => f.code === "allium.definition.unused"));
});

test("warns when rule trigger is not provided or emitted", () => {
  const findings = analyzeAllium(
    `rule A {\n  when: InvitationExpired(invitation)\n  ensures: Done()\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.rule.unreachableTrigger"));
});

test("unreachable trigger diagnostic range is anchored to trigger call in when clause", () => {
  const findings = analyzeAllium(
    `rule InvalidModeIsRejected {\n  when: CheckCommandInvoked(mode)\n  ensures: Done()\n}\n`,
  );
  const finding = findings.find(
    (item) => item.code === "allium.rule.unreachableTrigger",
  );
  assert.ok(finding);
  assert.equal(finding?.start.line, 1);
  assert.equal(finding?.start.character, 8);
  assert.equal(finding?.end.line, 1);
  assert.equal(finding?.end.character, 27);
});

test("does not warn when rule trigger is provided by a surface", () => {
  const findings = analyzeAllium(
    `surface Dashboard {\n  facing user: User\n  provides:\n    InvitationExpired(invitation)\n}\n\nrule A {\n  when: InvitationExpired(invitation)\n  ensures: Done()\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.rule.unreachableTrigger"),
    false,
  );
});

test("warns when two rules have duplicate behavior", () => {
  const findings = analyzeAllium(
    `rule A {\n  when: Ping(user)\n  requires: user.status = active\n  ensures: Done()\n}\n\nrule B {\n  when: Ping(user)\n  requires: user.status = active\n  ensures: Done()\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.rule.duplicateBehavior"));
});

test("reports potential shadowed rule when requires are stricter", () => {
  const findings = analyzeAllium(
    `rule Broad {\n  when: Ping(user)\n  ensures: Done()\n}\n\nrule Narrow {\n  when: Ping(user)\n  requires: user.status = active\n  ensures: Done()\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.rule.potentialShadow"));
});

test("reports equality type mismatches between string and numeric literals", () => {
  const findings = analyzeAllium(
    `rule A {\n  when: Ping(user)\n  requires: "old" = 1\n  ensures: Done()\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.expression.typeMismatch"));
});

test("reports field declared but never referenced", () => {
  const findings = analyzeAllium(
    `entity Invitation {\n  status: String\n  ignored: String\n}\n\nrule TouchStatus {\n  when: invitation: Invitation.created\n  ensures: invitation.status = active\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.field.unused"));
});

test("does not report field when referenced", () => {
  const findings = analyzeAllium(
    `entity Invitation {\n  status: String\n}\n\nrule TouchStatus {\n  when: invitation: Invitation.created\n  ensures: invitation.status = active\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.field.unused"),
    false,
  );
});

test("reports external entity without import source hints", () => {
  const findings = analyzeAllium(
    `external entity DirectoryUser {\n  id: String\n}\n`,
  );
  assert.ok(
    findings.some((f) => f.code === "allium.externalEntity.missingSourceHint"),
  );
});

test("does not report external entity source warning when imports exist", () => {
  const findings = analyzeAllium(
    `use "./directory.allium" as directory\n\nexternal entity DirectoryUser {\n  id: String\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.externalEntity.missingSourceHint"),
    false,
  );
});

test("reports deferred specification without location hint", () => {
  const findings = analyzeAllium(`deferred EscalationPolicy.at_level\n`);
  assert.ok(
    findings.some((f) => f.code === "allium.deferred.missingLocationHint"),
  );
});

test("does not report deferred specification with location hint", () => {
  const findings = analyzeAllium(
    `deferred EscalationPolicy.at_level "https://example.com/specs/escalation.allium"\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.deferred.missingLocationHint"),
    false,
  );
});

test("reports undefined status assignment value", () => {
  const findings = analyzeAllium(
    `entity Invitation {\n  status: pending | active | completed\n}\n\nrule CloseInvitation {\n  when: invitation: Invitation.created_at <= now\n  ensures: invitation.status = archived\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.status.undefinedValue"));
});

test("does not report known status assignment value", () => {
  const findings = analyzeAllium(
    `entity Invitation {\n  status: pending | active | completed\n}\n\nrule CloseInvitation {\n  when: invitation: Invitation.created_at <= now\n  ensures: invitation.status = completed\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.status.undefinedValue"),
    false,
  );
});

test("warns when status enum value is never assigned by any rule", () => {
  const findings = analyzeAllium(
    `entity Invitation {\n  status: pending | active | completed\n}\n\nrule CloseInvitation {\n  when: invitation: Invitation.created_at <= now\n  ensures: invitation.status = completed\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.status.unreachableValue"));
});

test("warns when non-terminal status has no observed exit transition", () => {
  const findings = analyzeAllium(
    `entity Invitation {\n  status: pending | active | completed\n}\n\nrule ActivateInvitation {\n  when: invitation: Invitation.created_at <= now\n  requires: invitation.status = pending\n  ensures: invitation.status = active\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.status.noExit"));
});

test("reports provides trigger missing external stimulus definition", () => {
  const findings = analyzeAllium(
    `rule TriggerA {\n  when: UserRequested()\n  ensures: Done()\n}\n\nsurface Dashboard {\n  facing viewer: User\n  provides:\n    MissingTrigger(viewer: viewer)\n}\n`,
  );
  assert.ok(
    findings.some((f) => f.code === "allium.surface.undefinedProvidesTrigger"),
  );
});

test("does not report provides trigger when defined by external stimulus rule", () => {
  const findings = analyzeAllium(
    `rule TriggerA {\n  when: TriggerA(viewer)\n  ensures: Done()\n}\n\nsurface Dashboard {\n  facing viewer: User\n  provides:\n    TriggerA(viewer: viewer)\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.surface.undefinedProvidesTrigger"),
    false,
  );
});

test("accepts transitions_to trigger shape", () => {
  const findings = analyzeAllium(
    `entity Order {\n  status: String\n}\n\nrule NotifyOnChange {\n  when: order: Order.status transitions_to shipped\n  ensures: Done()\n}\n`,
  );
  assert.equal(findings.filter((f) => f.code === "allium.rule.unknownTrigger").length, 0);
});

// Fix 1: related: clause parsing extracts only the surface name
test("related clause with parenthesised binding and when guard only reports surface name", () => {
  const findings = analyzeAllium(
    `surface QuoteVersions {\n  facing user: User\n}\n\nsurface Dashboard {\n  facing user: User\n  related:\n    QuoteVersions(quote) when quote.version_count > 1\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.surface.relatedUndefined"),
    false,
  );
});

test("related clause with unknown surface still reports error", () => {
  const findings = analyzeAllium(
    `surface Dashboard {\n  facing user: User\n  related:\n    MissingSurface(quote) when quote.count > 1\n}\n`,
  );
  const related = findings.filter(
    (f) => f.code === "allium.surface.relatedUndefined",
  );
  assert.equal(related.length, 1);
  assert.ok(related[0].message.includes("MissingSurface"));
});

// Fix 2: v1 capitalised inline enum detection
test("reports v1 inline enum when capitalised pipe values have no variant declarations", () => {
  const findings = analyzeAllium(
    `entity Quote {\n  status: Quoted | OrderSubmitted | Filled\n}\n`,
  );
  assert.ok(findings.some((f) => f.code === "allium.sum.v1InlineEnum"));
});

// Fix 3: discard binding _ should not warn
test("does not report unused binding for discard binding _", () => {
  const findings = analyzeAllium(
    `surface QuoteFeed {\n  facing _: Service\n  exposes:\n    System.status\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.surface.unusedBinding"),
    false,
  );
});

// Fix 4: variable status assignment suppresses unreachable/noExit
test("does not report unreachable status when assigned from variable", () => {
  const findings = analyzeAllium(
    `entity Quote {\n  status: pending | quoted | filled\n}\n\nrule ApplyStatusUpdate {\n  when: update: Quote.status becomes pending\n  ensures: update.status = new_status\n}\n`,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.status.unreachableValue"),
    false,
  );
  assert.equal(
    findings.some((f) => f.code === "allium.status.noExit"),
    false,
  );
});

// Fix 5: external entity source hint downgraded when referenced in rules
test("downgrades external entity source hint to info when referenced in rule logic", () => {
  const findings = analyzeAllium(
    `external entity Client {\n  id: String\n}\n\nrule IngestQuote {\n  when: RawQuoteReceived(data)\n  ensures:\n    Client.lookup(data.client_id)\n}\n`,
  );
  const hint = findings.find(
    (f) => f.code === "allium.externalEntity.missingSourceHint",
  );
  assert.ok(hint);
  assert.equal(hint?.severity, "info");
});
