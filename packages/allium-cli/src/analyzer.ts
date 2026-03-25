import { findMatchingBrace, parseAlliumBlocks } from "./parser";

export type FindingSeverity = "error" | "warning" | "info";
export type DiagnosticsMode = "strict" | "relaxed";

export interface Finding {
  code: string;
  message: string;
  severity: FindingSeverity;
  start: { line: number; character: number };
  end: { line: number; character: number };
}

export interface AnalyzeOptions {
  mode?: DiagnosticsMode;
}

export function analyzeAllium(
  text: string,
  options: AnalyzeOptions = {},
): Finding[] {
  const findings: Finding[] = [];
  const lineStarts = buildLineStarts(text);
  const blocks = parseAlliumBlocks(text);

  const ruleBlocks = blocks.filter((block) => block.kind === "rule");
  for (const block of ruleBlocks) {
    const hasWhen = /^\s*when\s*:/m.test(block.body);
    const hasEnsures = /^\s*ensures\s*:/m.test(block.body);

    if (!hasWhen) {
      findings.push(
        rangeFinding(
          lineStarts,
          block.nameStartOffset,
          block.nameStartOffset + block.name.length,
          "allium.rule.missingWhen",
          `Rule '${block.name}' must define a 'when:' trigger.`,
          "error",
        ),
      );
    }

    if (!hasEnsures) {
      findings.push(
        rangeFinding(
          lineStarts,
          block.endOffset,
          block.endOffset + 1,
          "allium.rule.missingEnsures",
          `Rule '${block.name}' should include at least one 'ensures:' clause.`,
          "error",
        ),
      );
    }

    const whenMatch = block.body.match(/^\s*when\s*:\s*(.+)$/m);
    const hasRequires = /^\s*requires\s*:/m.test(block.body);
    if (whenMatch && isTemporalWhenClause(whenMatch[1]) && !hasRequires) {
      const lineOffset =
        block.bodyStartOffset + block.body.indexOf(whenMatch[0]);
      findings.push(
        rangeFinding(
          lineStarts,
          lineOffset,
          lineOffset + whenMatch[0].length,
          "allium.temporal.missingGuard",
          "Temporal trigger should include a 'requires:' guard to avoid re-firing.",
          "warning",
        ),
      );
    }

    const letNames = new Set<string>();
    const letRegex = /^\s*let\s+([A-Za-z_][A-Za-z0-9_]*)\s*=/gm;
    for (
      let match = letRegex.exec(block.body);
      match;
      match = letRegex.exec(block.body)
    ) {
      const name = match[1];
      if (letNames.has(name)) {
        const offset = block.bodyStartOffset + match.index;
        findings.push(
          rangeFinding(
            lineStarts,
            offset,
            offset + match[0].length,
            "allium.let.duplicateBinding",
            `Binding '${name}' is declared more than once in rule '${block.name}'.`,
            "error",
          ),
        );
      }
      letNames.add(name);
    }
  }
  findings.push(...findInvalidTriggerIssues(lineStarts, blocks));

  findings.push(...findDuplicateConfigKeys(text, lineStarts, blocks));
  findings.push(...findDuplicateDefaultNames(text, lineStarts));
  findings.push(...findDefaultTypeReferenceIssues(text, lineStarts, blocks));
  findings.push(...findConfigParameterShapeIssues(lineStarts, blocks));
  findings.push(...findUndefinedConfigReferences(text, lineStarts, blocks));
  findings.push(
    ...findUndefinedExternalConfigReferences(text, lineStarts, blocks),
  );
  findings.push(...findUndefinedStatusAssignments(text, lineStarts, blocks));
  findings.push(...findStatusStateMachineIssues(text, lineStarts, blocks));
  findings.push(...findEnumDeclarationIssues(lineStarts, blocks));
  findings.push(...findSumTypeIssues(text, lineStarts));
  findings.push(
    ...findUnguardedVariantFieldAccessIssues(text, lineStarts, blocks),
  );
  findings.push(...findTypeReferenceIssues(text, lineStarts, blocks));
  findings.push(...findRelationshipReferenceIssues(text, lineStarts, blocks));
  findings.push(...findRuleTypeReferenceIssues(lineStarts, blocks, text));
  findings.push(...findRuleUndefinedBindingIssues(lineStarts, blocks, text));
  findings.push(...findContextBindingIssues(text, lineStarts, blocks));
  findings.push(...findOpenQuestions(text, lineStarts));
  findings.push(...findSurfaceActorLinkIssues(text, lineStarts, blocks));
  findings.push(...findSurfaceRelatedIssues(lineStarts, blocks));
  findings.push(...findSurfaceBindingUsageIssues(lineStarts, blocks));
  findings.push(...findSurfacePathAndIterationIssues(text, lineStarts, blocks));
  findings.push(...findSurfaceRuleCoverageIssues(text, lineStarts, blocks));
  findings.push(...findSurfaceImpossibleWhenIssues(lineStarts, blocks));
  findings.push(...findSurfaceNamedBlockUniquenessIssues(lineStarts, blocks));
  findings.push(
    ...findSurfaceRequiresDeferredHintIssues(lineStarts, blocks, text),
  );
  findings.push(...findSurfaceProvidesTriggerIssues(lineStarts, blocks, text));
  findings.push(...findUnusedEntityIssues(text, lineStarts));
  findings.push(...findUnusedNamedDefinitionIssues(text, lineStarts));
  findings.push(...findUnusedFieldIssues(text, lineStarts));
  findings.push(...findUnreachableRuleTriggerIssues(lineStarts, blocks));
  findings.push(...findExternalEntitySourceHints(text, lineStarts, blocks));
  findings.push(...findDeferredLocationHints(text, lineStarts));
  findings.push(...findImplicitLambdaIssues(text, lineStarts));
  findings.push(...findNeverFireRuleIssues(lineStarts, blocks));
  findings.push(...findDuplicateRuleBehaviourIssues(lineStarts, blocks));
  findings.push(...findExpressionTypeMismatchIssues(lineStarts, blocks));
  findings.push(...findDerivedCircularDependencyIssues(text, lineStarts));

  return applySuppressions(
    applyDiagnosticsMode(findings, options.mode ?? "strict"),
    text,
    lineStarts,
  );
}

function applyDiagnosticsMode(
  findings: Finding[],
  mode: DiagnosticsMode,
): Finding[] {
  if (mode === "strict") {
    return findings;
  }

  return findings.flatMap((finding) => {
    if (finding.code === "allium.temporal.missingGuard") {
      return [];
    }
    if (finding.code === "allium.config.undefinedReference") {
      return [{ ...finding, severity: "info" }];
    }
    return [finding];
  });
}

function findOpenQuestions(text: string, lineStarts: number[]): Finding[] {
  const findings: Finding[] = [];
  const pattern = /^\s*open\s+question\s+"[^"]*"/gm;
  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    findings.push(
      rangeFinding(
        lineStarts,
        match.index,
        match.index + match[0].length,
        "allium.openQuestion.present",
        "Open question present: specification is likely incomplete.",
        "warning",
      ),
    );
  }
  return findings;
}

function findUndefinedConfigReferences(
  text: string,
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Finding[] {
  const findings: Finding[] = [];
  const declared = new Set<string>();

  const configBlocks = blocks.filter((block) => block.kind === "config");
  const keyPattern = /^\s*([A-Za-z_][A-Za-z0-9_]*)\s*:/gm;
  for (const block of configBlocks) {
    for (
      let keyMatch = keyPattern.exec(block.body);
      keyMatch;
      keyMatch = keyPattern.exec(block.body)
    ) {
      declared.add(keyMatch[1]);
    }
  }

  const refPattern = /\bconfig\.([A-Za-z_][A-Za-z0-9_]*)\b/g;
  for (
    let match = refPattern.exec(text);
    match;
    match = refPattern.exec(text)
  ) {
    if (isCommentLineAtIndex(text, match.index)) {
      continue;
    }
    const key = match[1];
    if (!declared.has(key)) {
      findings.push(
        rangeFinding(
          lineStarts,
          match.index,
          match.index + match[0].length,
          "allium.config.undefinedReference",
          `Reference '${match[0]}' has no matching declaration in a local config block.`,
          "warning",
        ),
      );
    }
  }

  return findings;
}

function findUndefinedExternalConfigReferences(
  text: string,
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Finding[] {
  const findings: Finding[] = [];
  const aliases = new Set(
    blocks
      .filter((block) => block.kind === "use")
      .map((block) => block.alias ?? block.name),
  );
  const pattern =
    /\b([A-Za-z_][A-Za-z0-9_]*)\/config\.([A-Za-z_][A-Za-z0-9_]*)\b/g;

  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    if (isCommentLineAtIndex(text, match.index)) {
      continue;
    }
    const alias = match[1];
    if (aliases.has(alias)) {
      continue;
    }
    findings.push(
      rangeFinding(
        lineStarts,
        match.index,
        match.index + match[0].length,
        "allium.config.undefinedExternalReference",
        `External config reference '${match[0]}' uses unknown import alias '${alias}'.`,
        "error",
      ),
    );
  }

  return findings;
}

function findUndefinedStatusAssignments(
  text: string,
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Finding[] {
  const findings: Finding[] = [];
  const statusByEntity = collectEntityStatusEnums(text);
  if (statusByEntity.size === 0) {
    return findings;
  }

  const ruleBlocks = blocks.filter((block) => block.kind === "rule");
  for (const rule of ruleBlocks) {
    const whenBindingMatch = rule.body.match(
      /^\s*when\s*:\s*([A-Za-z_][A-Za-z0-9_]*)\s*:\s*([A-Za-z_][A-Za-z0-9_]*)\./m,
    );
    if (!whenBindingMatch) {
      continue;
    }
    const bindingName = whenBindingMatch[1];
    const entityName = whenBindingMatch[2];
    const allowedStatuses = statusByEntity.get(entityName);
    if (!allowedStatuses || allowedStatuses.size === 0) {
      continue;
    }

    const ensuresPattern = new RegExp(
      `^\\s*ensures\\s*:\\s*${escapeRegex(bindingName)}\\.status\\s*=\\s*([a-z_][a-z0-9_]*)\\b`,
      "gm",
    );
    for (
      let match = ensuresPattern.exec(rule.body);
      match;
      match = ensuresPattern.exec(rule.body)
    ) {
      const status = match[1];
      if (allowedStatuses.has(status)) {
        continue;
      }
      const statusOffset =
        rule.bodyStartOffset + match.index + match[0].lastIndexOf(status);
      findings.push(
        rangeFinding(
          lineStarts,
          statusOffset,
          statusOffset + status.length,
          "allium.status.undefinedValue",
          `Status value '${status}' is not declared in ${entityName}.status enum.`,
          "error",
        ),
      );
    }
  }

  return findings;
}

function findStatusStateMachineIssues(
  text: string,
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Finding[] {
  const findings: Finding[] = [];
  const statusByEntity = collectEntityStatusEnums(text);
  if (statusByEntity.size === 0) {
    return findings;
  }

  const contextTypes = collectContextBindingTypes(blocks);
  const assignedByEntity = new Map<string, Set<string>>();
  const transitionsByEntity = new Map<string, Map<string, Set<string>>>();
  const assignmentLocations = new Map<string, number>();
  const statusDeclarationOffsets = collectEntityStatusDeclarationOffsets(text);

  const ruleBlocks = blocks.filter((block) => block.kind === "rule");
  for (const rule of ruleBlocks) {
    const bindingTypes = collectRuleBindingTypes(rule.body, contextTypes);
    const clauseLines = collectRuleClauseLines(rule.body);
    const requiresByBinding = new Map<string, Set<string>>();
    for (const line of clauseLines) {
      if (line.clause !== "requires") {
        continue;
      }
      const requiresMatch = line.text.match(
        /([a-z_][a-z0-9_]*)\.status\s*=\s*([a-z_][a-z0-9_]*)\b/,
      );
      if (!requiresMatch) {
        continue;
      }
      const binding = requiresMatch[1];
      const status = requiresMatch[2];
      const set = requiresByBinding.get(binding) ?? new Set<string>();
      set.add(status);
      requiresByBinding.set(binding, set);
    }

    for (const line of clauseLines) {
      if (line.clause !== "ensures") {
        continue;
      }
      const assignMatch = line.text.match(
        /([a-z_][a-z0-9_]*)\.status\s*=\s*([a-z_][a-z0-9_]*)\b/,
      );
      if (!assignMatch) {
        continue;
      }
      const binding = assignMatch[1];
      const target = assignMatch[2];
      let entityName = bindingTypes.get(binding);
      if (!entityName) {
        for (const [name] of statusByEntity) {
          if (name.toLowerCase() === binding.toLowerCase()) {
            entityName = name;
            break;
          }
        }
      }
      if (!entityName || !statusByEntity.has(entityName)) {
        continue;
      }
      const assigned = assignedByEntity.get(entityName) ?? new Set<string>();
      assigned.add(target);
      assignedByEntity.set(entityName, assigned);
      if (!assignmentLocations.has(`${entityName}:${target}`)) {
        assignmentLocations.set(
          `${entityName}:${target}`,
          rule.bodyStartOffset + line.startOffset + line.text.indexOf(target),
        );
      }

      const sources = requiresByBinding.get(binding);
      if (!sources) {
        continue;
      }
      const entityTransitions =
        transitionsByEntity.get(entityName) ?? new Map<string, Set<string>>();
      for (const source of sources) {
        const to = entityTransitions.get(source) ?? new Set<string>();
        to.add(target);
        entityTransitions.set(source, to);
      }
      transitionsByEntity.set(entityName, entityTransitions);
    }
  }

  for (const [entityName, values] of statusByEntity.entries()) {
    const assigned = assignedByEntity.get(entityName) ?? new Set<string>();
    const transitions = transitionsByEntity.get(entityName) ?? new Map();

    const hasVariableAssignment = [...assigned].some((v) => !values.has(v));
    if (hasVariableAssignment) {
      continue;
    }

    for (const value of values) {
      if (!assigned.has(value)) {
        const declOffset = statusDeclarationOffsets.get(
          `${entityName}:${value}`,
        );
        findings.push(
          rangeFinding(
            lineStarts,
            declOffset ?? 0,
            (declOffset ?? 0) + value.length,
            "allium.status.unreachableValue",
            `Status '${value}' in entity '${entityName}' is never assigned by any rule ensures clause.`,
            "warning",
          ),
        );
      }

      if (isLikelyTerminalStatus(value)) {
        continue;
      }
      const exits = transitions.get(value);
      if (exits && exits.size > 0) {
        continue;
      }
      const offset =
        assignmentLocations.get(`${entityName}:${value}`) ??
        statusDeclarationOffsets.get(`${entityName}:${value}`) ??
        0;
      findings.push(
        rangeFinding(
          lineStarts,
          offset,
          offset + value.length,
          "allium.status.noExit",
          `Status '${value}' in entity '${entityName}' has no observed transition to a different status.`,
          "warning",
        ),
      );
    }
  }

  return findings;
}

function findDuplicateConfigKeys(
  text: string,
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Finding[] {
  const findings: Finding[] = [];
  const configBlocks = blocks.filter((block) => block.kind === "config");

  for (const block of configBlocks) {
    const seen = new Set<string>();
    const pattern = /^\s*([A-Za-z_][A-Za-z0-9_]*)\s*:/gm;
    for (
      let match = pattern.exec(block.body);
      match;
      match = pattern.exec(block.body)
    ) {
      const key = match[1];
      if (seen.has(key)) {
        const offset = block.bodyStartOffset + match.index;
        findings.push(
          rangeFinding(
            lineStarts,
            offset,
            offset + match[0].length,
            "allium.config.duplicateKey",
            `Config key '${key}' is declared more than once in this block.`,
            "error",
          ),
        );
      }
      seen.add(key);
    }
  }

  return findings;
}

function findConfigParameterShapeIssues(
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Finding[] {
  const findings: Finding[] = [];
  const configBlocks = blocks.filter((block) => block.kind === "config");
  const validPattern =
    /^\s*[A-Za-z_][A-Za-z0-9_]*\s*:\s*[A-Za-z_][A-Za-z0-9_<?>[\]| ]*\s*=\s*.+$/;
  const keyLinePattern = /^\s*([A-Za-z_][A-Za-z0-9_]*)\s*:/;

  for (const block of configBlocks) {
    const body = block.body;
    let cursor = 0;
    while (cursor < body.length) {
      const lineEnd = body.indexOf("\n", cursor);
      const end = lineEnd >= 0 ? lineEnd : body.length;
      const line = body.slice(cursor, end);
      const trimmed = line.trim();
      if (trimmed.length > 0 && !trimmed.startsWith("--")) {
        const keyMatch = line.match(keyLinePattern);
        if (keyMatch && !validPattern.test(line)) {
          const keyOffset =
            block.bodyStartOffset + cursor + line.indexOf(keyMatch[1]);
          findings.push(
            rangeFinding(
              lineStarts,
              keyOffset,
              keyOffset + keyMatch[1].length,
              "allium.config.invalidParameter",
              `Config parameter '${keyMatch[1]}' must declare both explicit type and default value.`,
              "error",
            ),
          );
        }
      }
      cursor = end + 1;
    }
  }
  return findings;
}

function findDuplicateDefaultNames(
  text: string,
  lineStarts: number[],
): Finding[] {
  const findings: Finding[] = [];
  const seen = new Set<string>();
  const pattern =
    /^\s*default\s+[A-Za-z_][A-Za-z0-9_]*(?:\s+([A-Za-z_][A-Za-z0-9_]*))?\s*=/gm;
  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    const instanceName = match[1];
    if (!instanceName) {
      continue;
    }
    if (seen.has(instanceName)) {
      const offset = match.index + match[0].indexOf(instanceName);
      findings.push(
        rangeFinding(
          lineStarts,
          offset,
          offset + instanceName.length,
          "allium.default.duplicateName",
          `Default instance '${instanceName}' is declared more than once.`,
          "error",
        ),
      );
    }
    seen.add(instanceName);
  }
  return findings;
}

function findDefaultTypeReferenceIssues(
  text: string,
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Finding[] {
  const findings: Finding[] = [];
  const declaredTypes = new Set<string>([
    ...collectDeclaredTypeNames(text),
    "String",
    "Integer",
    "Decimal",
    "Boolean",
    "Timestamp",
    "Duration",
    "List",
    "Set",
    "Map",
  ]);
  const aliases = new Set(
    blocks
      .filter((block) => block.kind === "use")
      .map((block) => block.alias ?? block.name),
  );
  const pattern =
    /^\s*default\s+([A-Za-z_][A-Za-z0-9_]*(?:\/[A-Za-z_][A-Za-z0-9_]*)?)(?:\s+[A-Za-z_][A-Za-z0-9_]*)?\s*=/gm;
  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    const typeName = match[1];
    const offset = match.index + match[0].indexOf(typeName);
    findings.push(
      ...validateTypeNameReference(
        typeName,
        offset,
        lineStarts,
        declaredTypes,
        aliases,
        "allium.default.undefinedType",
        "allium.default.undefinedImportedAlias",
      ),
    );
  }
  return findings;
}

function findEnumDeclarationIssues(
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Finding[] {
  const findings: Finding[] = [];
  const enumBlocks = blocks.filter((block) => block.kind === "enum");

  for (const block of enumBlocks) {
    const literals = new Set<string>();
    let foundAny = false;
    const literalPattern = /\b([a-z_][a-z0-9_]*)\b/g;
    for (
      let literal = literalPattern.exec(block.body);
      literal;
      literal = literalPattern.exec(block.body)
    ) {
      foundAny = true;
      const value = literal[1];
      if (literals.has(value)) {
        const offset = block.bodyStartOffset + literal.index;
        findings.push(
          rangeFinding(
            lineStarts,
            offset,
            offset + value.length,
            "allium.enum.duplicateLiteral",
            `Enum '${block.name}' declares literal '${value}' more than once.`,
            "error",
          ),
        );
      }
      literals.add(value);
    }

    if (!foundAny) {
      findings.push(
        rangeFinding(
          lineStarts,
          block.nameStartOffset,
          block.nameStartOffset + block.name.length,
          "allium.enum.empty",
          `Enum '${block.name}' should declare at least one literal.`,
          "warning",
        ),
      );
    }
  }

  return findings;
}

function findSumTypeIssues(text: string, lineStarts: number[]): Finding[] {
  const findings: Finding[] = [];
  const variants = parseVariantDeclarations(text);
  const variantsByBase = new Map<string, Set<string>>();
  for (const variant of variants) {
    const set = variantsByBase.get(variant.base) ?? new Set<string>();
    set.add(variant.name);
    variantsByBase.set(variant.base, set);
  }

  const entities = parseEntityBlocks(text);
  const discriminatorByEntity = new Map<string, Set<string>>();
  for (const entity of entities) {
    for (const field of entity.pipeFields) {
      if (!field.hasCapitalizedName) {
        continue;
      }
      if (!field.allNamesCapitalized) {
        findings.push(
          rangeFinding(
            lineStarts,
            field.startOffset,
            field.startOffset + field.rawNames.length,
            "allium.sum.invalidDiscriminator",
            `Entity '${entity.name}' discriminator '${field.fieldName}' must use only capitalized variant names.`,
            "error",
          ),
        );
        continue;
      }

      const listed = new Set(field.names);
      discriminatorByEntity.set(entity.name, listed);
      const declaredForBase =
        variantsByBase.get(entity.name) ?? new Set<string>();
      const missingVariants = field.names.filter(
        (name) => !declaredForBase.has(name),
      );
      if (
        missingVariants.length === field.names.length &&
        declaredForBase.size === 0
      ) {
        findings.push(
          rangeFinding(
            lineStarts,
            field.startOffset,
            field.startOffset + field.rawNames.length,
            "allium.sum.v1InlineEnum",
            `Entity '${entity.name}' field '${field.fieldName}' uses capitalised pipe values with no variant declarations. In v3, capitalised values are variant references requiring 'variant X : ${entity.name}' declarations. Use lowercase values for a plain enum.`,
            "error",
          ),
        );
      } else {
        for (const name of missingVariants) {
          findings.push(
            rangeFinding(
              lineStarts,
              field.startOffset,
              field.startOffset + field.rawNames.length,
              "allium.sum.discriminatorUnknownVariant",
              `Entity '${entity.name}' discriminator references '${name}' without matching 'variant ${name} : ${entity.name}'.`,
              "error",
            ),
          );
        }
      }
    }
  }

  for (const variant of variants) {
    const listed = discriminatorByEntity.get(variant.base);
    if (!listed || listed.has(variant.name)) {
      continue;
    }
    findings.push(
      rangeFinding(
        lineStarts,
        variant.startOffset,
        variant.startOffset + variant.name.length,
        "allium.sum.variantMissingInDiscriminator",
        `Variant '${variant.name}' extends '${variant.base}' but is missing from '${variant.base}' discriminator field.`,
        "error",
      ),
    );
  }

  for (const entity of entities) {
    if (!discriminatorByEntity.has(entity.name)) {
      continue;
    }
    const pattern = new RegExp(
      `\\b${escapeRegex(entity.name)}\\.created\\s*\\(`,
      "g",
    );
    for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
      if (isCommentLineAtIndex(text, match.index)) {
        continue;
      }
      findings.push(
        rangeFinding(
          lineStarts,
          match.index,
          match.index + entity.name.length,
          "allium.sum.baseInstantiation",
          `Base entity '${entity.name}' with discriminator cannot be instantiated directly; instantiate a variant instead.`,
          "error",
        ),
      );
    }
  }

  const missingKeywordPattern =
    /^\s*([A-Z][A-Za-z0-9_]*)\s*:\s*([A-Z][A-Za-z0-9_]*)\s*\{/gm;
  for (
    let match = missingKeywordPattern.exec(text);
    match;
    match = missingKeywordPattern.exec(text)
  ) {
    const lineEnd = text.indexOf("\n", match.index);
    const line = text.slice(
      text.lastIndexOf("\n", match.index) + 1,
      lineEnd >= 0 ? lineEnd : text.length,
    );
    if (
      /^\s*(entity|external\s+entity|value|variant|rule|surface|actor|enum|config|context)\b/.test(
        line,
      )
    ) {
      continue;
    }
    findings.push(
      rangeFinding(
        lineStarts,
        match.index,
        match.index + match[1].length,
        "allium.sum.missingVariantKeyword",
        `Declaration '${match[1]} : ${match[2]} { ... }' must use 'variant ${match[1]} : ${match[2]} { ... }'.`,
        "error",
      ),
    );
  }

  return findings;
}

function findUnguardedVariantFieldAccessIssues(
  text: string,
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Finding[] {
  const findings: Finding[] = [];
  const variants = parseVariantFieldDefinitions(text);
  if (variants.length === 0) {
    return findings;
  }
  const discriminatorByBase = collectDiscriminatorFieldsByEntity(text);
  const variantFieldsByBase = new Map<
    string,
    Map<string, { variant: string; field: string }>
  >();
  for (const variant of variants) {
    const byField = variantFieldsByBase.get(variant.base) ?? new Map();
    for (const field of variant.fields) {
      byField.set(field, { variant: variant.name, field });
    }
    variantFieldsByBase.set(variant.base, byField);
  }

  const contextTypes = collectContextBindingTypes(blocks);
  const rules = blocks.filter((block) => block.kind === "rule");
  for (const rule of rules) {
    const bindingTypes = collectRuleBindingTypes(rule.body, contextTypes);
    const guardByBinding = new Map<string, Set<string>>();
    const lines = collectRuleClauseLines(rule.body);
    for (const line of lines) {
      const guard = line.text.match(
        /([a-z_][a-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*)\s*=\s*([A-Z][A-Za-z0-9_]*)/,
      );
      if (!guard) {
        continue;
      }
      const binding = guard[1];
      const discriminator = guard[2];
      const variant = guard[3];
      const base = bindingTypes.get(binding);
      if (!base || discriminatorByBase.get(base) !== discriminator) {
        continue;
      }
      const set = guardByBinding.get(binding) ?? new Set<string>();
      set.add(variant);
      guardByBinding.set(binding, set);
    }

    const accessPattern = /([a-z_][a-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*)\b/g;
    for (
      let access = accessPattern.exec(rule.body);
      access;
      access = accessPattern.exec(rule.body)
    ) {
      if (isCommentLineAtIndex(rule.body, access.index)) {
        continue;
      }
      const binding = access[1];
      const field = access[2];
      const base = bindingTypes.get(binding);
      if (!base) {
        continue;
      }
      const variantField = variantFieldsByBase.get(base)?.get(field);
      if (!variantField) {
        continue;
      }
      const guards = guardByBinding.get(binding);
      if (guards && guards.has(variantField.variant)) {
        continue;
      }
      const absoluteOffset = rule.bodyStartOffset + access.index;
      findings.push(
        rangeFinding(
          lineStarts,
          absoluteOffset,
          absoluteOffset + access[0].length,
          "allium.sum.unguardedVariantFieldAccess",
          `Variant-specific field access '${access[0]}' requires a guard on ${binding}.${discriminatorByBase.get(base)} = ${variantField.variant}.`,
          "error",
        ),
      );
    }
  }

  return findings;
}

function findTypeReferenceIssues(
  text: string,
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Finding[] {
  const findings: Finding[] = [];
  const declaredTypes = new Set<string>([
    ...collectDeclaredTypeNames(text),
    "String",
    "Integer",
    "Decimal",
    "Boolean",
    "Timestamp",
    "Duration",
    "List",
    "Set",
    "Map",
  ]);
  const aliases = new Set(
    blocks
      .filter((block) => block.kind === "use")
      .map((block) => block.alias ?? block.name),
  );

  const typeSites = collectFieldTypeSites(text);
  for (const site of typeSites) {
    const pattern = /([A-Za-z_][A-Za-z0-9_]*(?:\/[A-Za-z_][A-Za-z0-9_]*)?)/g;
    for (
      let token = pattern.exec(site.typeExpression);
      token;
      token = pattern.exec(site.typeExpression)
    ) {
      const value = token[1];
      const absoluteOffset = site.startOffset + token.index;
      if (value.includes("/")) {
        const alias = value.split("/")[0];
        if (!aliases.has(alias)) {
          findings.push(
            rangeFinding(
              lineStarts,
              absoluteOffset,
              absoluteOffset + value.length,
              "allium.type.undefinedImportedAlias",
              `Type reference '${value}' uses unknown import alias '${alias}'.`,
              "error",
            ),
          );
        }
        continue;
      }
      if (/^[a-z]/.test(value)) {
        continue;
      }
      if (!declaredTypes.has(value)) {
        findings.push(
          rangeFinding(
            lineStarts,
            absoluteOffset,
            absoluteOffset + value.length,
            "allium.type.undefinedReference",
            `Type reference '${value}' is not declared locally or imported.`,
            "error",
          ),
        );
      }
    }
  }

  return findings;
}

function findRelationshipReferenceIssues(
  text: string,
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Finding[] {
  const findings: Finding[] = [];
  const declaredTypes = new Set<string>(collectDeclaredTypeNames(text));
  const aliases = new Set(
    blocks
      .filter((block) => block.kind === "use")
      .map((block) => block.alias ?? block.name),
  );
  const relationships = collectRelationshipTypeSites(text);

  for (const rel of relationships) {
    findings.push(
      ...validateTypeNameReference(
        rel.targetType,
        rel.startOffset,
        lineStarts,
        declaredTypes,
        aliases,
        "allium.relationship.undefinedTarget",
        "allium.relationship.undefinedImportedAlias",
      ),
    );

    if (rel.targetType.includes("/")) {
      continue;
    }
    if (looksLikePluralTypeName(rel.targetType)) {
      findings.push(
        rangeFinding(
          lineStarts,
          rel.startOffset,
          rel.startOffset + rel.targetType.length,
          "allium.relationship.nonSingularTarget",
          `Relationship target '${rel.targetType}' looks plural; use singular entity type names in relationships.`,
          "warning",
        ),
      );
    }
  }

  return findings;
}

function findRuleTypeReferenceIssues(
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
  text: string,
): Finding[] {
  const findings: Finding[] = [];
  const declaredTypes = new Set<string>(collectDeclaredTypeNames(text));
  const aliases = new Set(
    blocks
      .filter((block) => block.kind === "use")
      .map((block) => block.alias ?? block.name),
  );
  const ruleBlocks = blocks.filter((block) => block.kind === "rule");
  const patterns = [
    /^\s*when\s*:\s*[A-Za-z_][A-Za-z0-9_]*\s*:\s*([A-Za-z_][A-Za-z0-9_]*(?:\/[A-Za-z_][A-Za-z0-9_]*)?)\./gm,
    /^\s*when\s*:\s*([A-Za-z_][A-Za-z0-9_]*(?:\/[A-Za-z_][A-Za-z0-9_]*)?)\.created\s*\(/gm,
    /^\s*ensures\s*:\s*([A-Za-z_][A-Za-z0-9_]*(?:\/[A-Za-z_][A-Za-z0-9_]*)?)\.created\s*\(/gm,
  ];

  for (const rule of ruleBlocks) {
    for (const pattern of patterns) {
      for (
        let match = pattern.exec(rule.body);
        match;
        match = pattern.exec(rule.body)
      ) {
        const typeName = match[1];
        const offset =
          rule.bodyStartOffset + match.index + match[0].indexOf(typeName);
        findings.push(
          ...validateTypeNameReference(
            typeName,
            offset,
            lineStarts,
            declaredTypes,
            aliases,
            "allium.rule.undefinedTypeReference",
            "allium.rule.undefinedImportedAlias",
          ),
        );
      }
    }
  }
  return findings;
}

function findRuleUndefinedBindingIssues(
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
  text: string,
): Finding[] {
  const findings: Finding[] = [];
  const contextBindings = collectContextBindingNames(blocks);
  const defaultInstances = collectDefaultInstanceNames(text);
  const ruleBlocks = blocks.filter((block) => block.kind === "rule");

  for (const rule of ruleBlocks) {
    const bound = collectRuleBoundNames(
      rule.body,
      contextBindings,
      defaultInstances,
    );
    const seenUnknown = new Set<string>();
    const referencePattern = /\b([a-z_][a-z0-9_]*)\s*\./g;
    for (
      let match = referencePattern.exec(rule.body);
      match;
      match = referencePattern.exec(rule.body)
    ) {
      if (match.index > 0 && rule.body[match.index - 1] === ".") {
        continue;
      }
      if (isCommentLineAtIndex(rule.body, match.index)) {
        continue;
      }
      if (isInsideDoubleQuotedStringAtIndex(rule.body, match.index)) {
        continue;
      }
      const root = match[1];
      if (root === "config" || root === "now" || bound.has(root)) {
        continue;
      }
      if (seenUnknown.has(root)) {
        continue;
      }
      seenUnknown.add(root);
      const absoluteOffset =
        rule.bodyStartOffset + match.index + match[0].indexOf(root);
      findings.push(
        rangeFinding(
          lineStarts,
          absoluteOffset,
          absoluteOffset + root.length,
          "allium.rule.undefinedBinding",
          `Rule '${rule.name}' references '${root}' but no matching binding exists in context, trigger params, default instances, or local lets.`,
          "error",
        ),
      );
    }

    const existsPattern = /\bexists\s+([a-z_][a-z0-9_]*)\b/g;
    for (
      let match = existsPattern.exec(rule.body);
      match;
      match = existsPattern.exec(rule.body)
    ) {
      const root = match[1];
      if (isCommentLineAtIndex(rule.body, match.index)) {
        continue;
      }
      if (root === "config" || root === "now" || bound.has(root)) {
        continue;
      }
      if (seenUnknown.has(root)) {
        continue;
      }
      seenUnknown.add(root);
      const absoluteOffset =
        rule.bodyStartOffset + match.index + match[0].indexOf(root);
      findings.push(
        rangeFinding(
          lineStarts,
          absoluteOffset,
          absoluteOffset + root.length,
          "allium.rule.undefinedBinding",
          `Rule '${rule.name}' references '${root}' but no matching binding exists in context, trigger params, default instances, or local lets.`,
          "error",
        ),
      );
    }

    const forInPattern =
      /^\s*for\s+[A-Za-z_][A-Za-z0-9_]*\s+in\s+([a-z_][a-z0-9_]*)\b/gm;
    for (
      let match = forInPattern.exec(rule.body);
      match;
      match = forInPattern.exec(rule.body)
    ) {
      const root = match[1];
      if (root === "config" || root === "now" || bound.has(root)) {
        continue;
      }
      if (seenUnknown.has(root)) {
        continue;
      }
      seenUnknown.add(root);
      const absoluteOffset =
        rule.bodyStartOffset + match.index + match[0].indexOf(root);
      findings.push(
        rangeFinding(
          lineStarts,
          absoluteOffset,
          absoluteOffset + root.length,
          "allium.rule.undefinedBinding",
          `Rule '${rule.name}' references '${root}' but no matching binding exists in context, trigger params, default instances, or local lets.`,
          "error",
        ),
      );
    }
  }

  return findings;
}

function findContextBindingIssues(
  text: string,
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Finding[] {
  const findings: Finding[] = [];
  const localEntityTypes = new Set<string>();
  const declaredEntityPattern =
    /^\s*(?:external\s+)?entity\s+([A-Za-z_][A-Za-z0-9_]*)\b/gm;
  for (
    let match = declaredEntityPattern.exec(text);
    match;
    match = declaredEntityPattern.exec(text)
  ) {
    localEntityTypes.add(match[1]);
  }
  const variantPattern = /^\s*variant\s+([A-Za-z_][A-Za-z0-9_]*)\s*:/gm;
  for (
    let match = variantPattern.exec(text);
    match;
    match = variantPattern.exec(text)
  ) {
    localEntityTypes.add(match[1]);
  }

  const importAliases = new Set(
    blocks
      .filter((block) => block.kind === "use")
      .map((block) => block.alias ?? block.name),
  );
  const contextBlocks = blocks.filter((block) => block.kind === "given");
  const bindingPattern =
    /^\s*([A-Za-z_][A-Za-z0-9_]*)\s*:\s*([A-Za-z_][A-Za-z0-9_]*(?:\/[A-Za-z_][A-Za-z0-9_]*)?)\s*$/gm;

  for (const block of contextBlocks) {
    const seenBindings = new Set<string>();
    for (
      let match = bindingPattern.exec(block.body);
      match;
      match = bindingPattern.exec(block.body)
    ) {
      const bindingName = match[1];
      const bindingType = match[2];
      const bindingOffset =
        block.bodyStartOffset + match.index + match[0].indexOf(bindingName);

      if (seenBindings.has(bindingName)) {
        findings.push(
          rangeFinding(
            lineStarts,
            bindingOffset,
            bindingOffset + bindingName.length,
            "allium.context.duplicateBinding",
            `Context binding '${bindingName}' is declared more than once.`,
            "error",
          ),
        );
      }
      seenBindings.add(bindingName);

      if (bindingType.includes("/")) {
        const alias = bindingType.split("/")[0];
        if (!importAliases.has(alias)) {
          const typeOffset =
            block.bodyStartOffset + match.index + match[0].indexOf(bindingType);
          findings.push(
            rangeFinding(
              lineStarts,
              typeOffset,
              typeOffset + bindingType.length,
              "allium.context.undefinedType",
              `Context binding type '${bindingType}' does not resolve to a local entity or imported alias.`,
              "error",
            ),
          );
        }
        continue;
      }

      if (!localEntityTypes.has(bindingType)) {
        const typeOffset =
          block.bodyStartOffset + match.index + match[0].indexOf(bindingType);
        findings.push(
          rangeFinding(
            lineStarts,
            typeOffset,
            typeOffset + bindingType.length,
            "allium.context.undefinedType",
            `Context binding type '${bindingType}' does not resolve to a local entity or imported alias.`,
            "error",
          ),
        );
      }
    }
  }

  return findings;
}

function collectContextBindingNames(
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Set<string> {
  const names = new Set<string>();
  const contextBlocks = blocks.filter((block) => block.kind === "given");
  const bindingPattern = /^\s*([A-Za-z_][A-Za-z0-9_]*)\s*:/gm;
  for (const block of contextBlocks) {
    for (
      let match = bindingPattern.exec(block.body);
      match;
      match = bindingPattern.exec(block.body)
    ) {
      names.add(match[1]);
    }
  }
  return names;
}

function collectContextBindingTypes(
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Map<string, string> {
  const types = new Map<string, string>();
  const contextBlocks = blocks.filter((block) => block.kind === "given");
  const pattern =
    /^\s*([A-Za-z_][A-Za-z0-9_]*)\s*:\s*([A-Za-z_][A-Za-z0-9_]*(?:\/[A-Za-z_][A-Za-z0-9_]*)?)\s*$/gm;
  for (const block of contextBlocks) {
    for (
      let match = pattern.exec(block.body);
      match;
      match = pattern.exec(block.body)
    ) {
      const binding = match[1];
      const typeRef = match[2];
      if (typeRef.includes("/")) {
        continue;
      }
      types.set(binding, typeRef);
    }
  }
  return types;
}

function collectRuleBindingTypes(
  ruleBody: string,
  contextTypes: Map<string, string>,
): Map<string, string> {
  const bindingTypes = new Map<string, string>(contextTypes);
  const whenTyped =
    /^\s*when\s*:\s*([A-Za-z_][A-Za-z0-9_]*)\s*:\s*([A-Za-z_][A-Za-z0-9_]*)(?:\/[A-Za-z_][A-Za-z0-9_]*)?\./m;
  const whenMatch = ruleBody.match(whenTyped);
  if (whenMatch) {
    bindingTypes.set(whenMatch[1], whenMatch[2]);
  }
  return bindingTypes;
}

function collectRuleClauseLines(ruleBody: string): Array<{
  clause: "requires" | "ensures" | "other";
  text: string;
  startOffset: number;
}> {
  const lines: Array<{
    clause: "requires" | "ensures" | "other";
    text: string;
    startOffset: number;
  }> = [];
  let current: "requires" | "ensures" | "other" = "other";
  let cursor = 0;
  while (cursor < ruleBody.length) {
    const lineEnd = ruleBody.indexOf("\n", cursor);
    const end = lineEnd >= 0 ? lineEnd : ruleBody.length;
    const text = ruleBody.slice(cursor, end);
    const trimmed = text.trim();
    if (/^\s*requires\s*:/.test(text)) {
      current = "requires";
    } else if (/^\s*ensures\s*:/.test(text)) {
      current = "ensures";
    } else if (/^\s*(when|let|for)\b/.test(text)) {
      current = "other";
    } else if (trimmed.length === 0) {
      current = "other";
    }
    lines.push({ clause: current, text, startOffset: cursor });
    cursor = end + 1;
  }
  return lines;
}

function collectEntityStatusDeclarationOffsets(
  text: string,
): Map<string, number> {
  const offsets = new Map<string, number>();
  const entityPattern =
    /^\s*(?:external\s+)?entity\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{/gm;
  for (
    let entity = entityPattern.exec(text);
    entity;
    entity = entityPattern.exec(text)
  ) {
    const entityName = entity[1];
    const open = text.indexOf("{", entity.index);
    if (open < 0) {
      continue;
    }
    const close = findMatchingBrace(text, open);
    if (close < 0) {
      continue;
    }
    const body = text.slice(open + 1, close);
    const statusMatch = body.match(
      /^\s*status\s*:\s*([a-z_][a-z0-9_]*(?:\s*\|\s*[a-z_][a-z0-9_]*)+)\s*$/m,
    );
    if (!statusMatch) {
      continue;
    }
    const values = statusMatch[1]
      .split("|")
      .map((value) => value.trim())
      .filter(Boolean);
    const base =
      open +
      1 +
      body.indexOf(statusMatch[0]) +
      statusMatch[0].indexOf(statusMatch[1]);
    for (const value of values) {
      offsets.set(
        `${entityName}:${value}`,
        base + statusMatch[1].indexOf(value),
      );
    }
  }
  return offsets;
}

function isLikelyTerminalStatus(status: string): boolean {
  return /^(completed|cancelled|canceled|expired|closed|deleted|archived|failed|rejected|done)$/.test(
    status,
  );
}

function collectDefaultInstanceNames(text: string): Set<string> {
  const names = new Set<string>();
  const pattern =
    /^\s*default\s+([A-Za-z_][A-Za-z0-9_]*)(?:\s+([A-Za-z_][A-Za-z0-9_]*))?\s*=/gm;
  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    names.add(match[2] ?? match[1]);
  }
  return names;
}

function collectRuleBoundNames(
  ruleBody: string,
  contextBindings: Set<string>,
  defaultInstances: Set<string>,
): Set<string> {
  const bound = new Set<string>([...contextBindings, ...defaultInstances]);
  const whenBindingPattern =
    /^\s*when\s*:\s*([A-Za-z_][A-Za-z0-9_]*)\s*:\s*[A-Za-z_][A-Za-z0-9_]*(?:\/[A-Za-z_][A-Za-z0-9_]*)?\./m;
  const whenBindingMatch = ruleBody.match(whenBindingPattern);
  if (whenBindingMatch) {
    bound.add(whenBindingMatch[1]);
  }

  const whenCallPattern =
    /^\s*when\s*:\s*[A-Za-z_][A-Za-z0-9_]*(?:\/[A-Za-z_][A-Za-z0-9_]*)?\s*\(([^)]*)\)/m;
  const whenCallMatch = ruleBody.match(whenCallPattern);
  if (whenCallMatch) {
    for (const raw of whenCallMatch[1].split(",")) {
      const name = raw.trim();
      if (name.length === 0 || name === "_") {
        continue;
      }
      if (/^[A-Za-z_][A-Za-z0-9_]*\??$/.test(name)) {
        bound.add(name.replace(/\?$/, ""));
      }
    }
  }

  const forPattern = /^\s*for\s+([A-Za-z_][A-Za-z0-9_]*)\s+in\s+/gm;
  for (
    let match = forPattern.exec(ruleBody);
    match;
    match = forPattern.exec(ruleBody)
  ) {
    bound.add(match[1]);
  }

  const letPattern = /^\s*let\s+([A-Za-z_][A-Za-z0-9_]*)\s*=/gm;
  for (
    let match = letPattern.exec(ruleBody);
    match;
    match = letPattern.exec(ruleBody)
  ) {
    bound.add(match[1]);
  }

  const lambdaPattern = /\b([A-Za-z_][A-Za-z0-9_]*)\s*=>/g;
  for (
    let match = lambdaPattern.exec(ruleBody);
    match;
    match = lambdaPattern.exec(ruleBody)
  ) {
    bound.add(match[1]);
  }

  const wherePattern = /\bwhere\s+([A-Za-z_][A-Za-z0-9_]*)\b/g;
  for (
    let match = wherePattern.exec(ruleBody);
    match;
    match = wherePattern.exec(ruleBody)
  ) {
    bound.add(match[1]);
  }

  return bound;
}

function isInsideDoubleQuotedStringAtIndex(
  text: string,
  index: number,
): boolean {
  const lineStart = text.lastIndexOf("\n", index) + 1;
  let inString = false;
  for (let i = lineStart; i < index; i += 1) {
    if (text[i] !== '"' || text[i - 1] === "\\") {
      continue;
    }
    inString = !inString;
  }
  return inString;
}

function isTemporalWhenClause(clause: string): boolean {
  const normalized = clause.trim();
  if (/:[^\n]*(<=|>=|<|>)\s*now\b/.test(normalized)) {
    return true;
  }
  if (/\bnow\s*[+-]\s*\d/.test(normalized)) {
    return true;
  }
  return false;
}

function buildLineStarts(text: string): number[] {
  const starts = [0];
  for (let i = 0; i < text.length; i += 1) {
    if (text[i] === "\n") {
      starts.push(i + 1);
    }
  }
  return starts;
}

function offsetToPosition(
  lineStarts: number[],
  offset: number,
): { line: number; character: number } {
  let line = 0;
  let hi = lineStarts.length - 1;
  while (line <= hi) {
    const mid = Math.floor((line + hi) / 2);
    if (lineStarts[mid] <= offset) {
      if (mid === lineStarts.length - 1 || lineStarts[mid + 1] > offset) {
        return { line: mid, character: offset - lineStarts[mid] };
      }
      line = mid + 1;
    } else {
      hi = mid - 1;
    }
  }

  return { line: 0, character: offset };
}

function rangeFinding(
  lineStarts: number[],
  startOffset: number,
  endOffset: number,
  code: string,
  message: string,
  severity: FindingSeverity,
): Finding {
  return {
    code,
    message,
    severity,
    start: offsetToPosition(lineStarts, startOffset),
    end: offsetToPosition(lineStarts, endOffset),
  };
}

function findSurfaceActorLinkIssues(
  _text: string,
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Finding[] {
  const findings: Finding[] = [];
  const actorNames = new Set(
    blocks.filter((block) => block.kind === "actor").map((block) => block.name),
  );
  const surfaceBlocks = blocks.filter((block) => block.kind === "surface");
  const referencedActors = new Set<string>();
  const forPattern =
    /^\s*facing\s+[A-Za-z_][A-Za-z0-9_]*\s*:\s*([A-Za-z_][A-Za-z0-9_]*)\s*$/m;

  for (const surface of surfaceBlocks) {
    const match = surface.body.match(forPattern);
    if (!match) {
      continue;
    }
    const actorName = match[1];
    referencedActors.add(actorName);
    if (!actorNames.has(actorName)) {
      const lineOffset =
        surface.bodyStartOffset + surface.body.indexOf(match[0]);
      findings.push(
        rangeFinding(
          lineStarts,
          lineOffset,
          lineOffset + match[0].length,
          "allium.surface.missingActor",
          `Surface '${surface.name}' references actor '${actorName}' which is not declared locally.`,
          "warning",
        ),
      );
    }
  }

  for (const actor of blocks.filter((block) => block.kind === "actor")) {
    if (referencedActors.has(actor.name)) {
      continue;
    }
    findings.push(
      rangeFinding(
        lineStarts,
        actor.nameStartOffset,
        actor.nameStartOffset + actor.name.length,
        "allium.actor.unused",
        `Actor '${actor.name}' is not referenced by any local surface.`,
        "info",
      ),
    );
  }

  return findings;
}

function findSurfaceRelatedIssues(
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Finding[] {
  const findings: Finding[] = [];
  const surfaceBlocks = blocks.filter((block) => block.kind === "surface");
  const knownSurfaceNames = new Set(
    surfaceBlocks.map((surface) => surface.name),
  );

  for (const surface of surfaceBlocks) {
    const relatedRefs = parseRelatedReferences(surface.body);
    for (const ref of relatedRefs) {
      if (knownSurfaceNames.has(ref.name)) {
        continue;
      }
      const offset = surface.bodyStartOffset + ref.offsetInBody;
      findings.push(
        rangeFinding(
          lineStarts,
          offset,
          offset + ref.name.length,
          "allium.surface.relatedUndefined",
          `Surface '${surface.name}' references unknown related surface '${ref.name}'.`,
          "error",
        ),
      );
    }
  }

  return findings;
}

function findSurfaceBindingUsageIssues(
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Finding[] {
  const findings: Finding[] = [];
  const surfaceBlocks = blocks.filter((block) => block.kind === "surface");

  for (const surface of surfaceBlocks) {
    const body = surface.body;
    const forMatch = body.match(
      /^\s*facing\s+([A-Za-z_][A-Za-z0-9_]*)\s*:\s*[A-Za-z_][A-Za-z0-9_]*(?:\s+with\s+.+)?\s*$/m,
    );
    const contextMatch = body.match(
      /^\s*context\s+([A-Za-z_][A-Za-z0-9_]*)\s*:\s*[A-Za-z_][A-Za-z0-9_]*(?:\s+with\s+.+)?\s*$/m,
    );
    const bindings = [
      ...(forMatch
        ? [{ name: forMatch[1], source: "facing", line: forMatch[0] }]
        : []),
      ...(contextMatch
        ? [{ name: contextMatch[1], source: "context", line: contextMatch[0] }]
        : []),
    ];

    for (const binding of bindings) {
      if (binding.name === "_") {
        continue;
      }
      const usagePattern = new RegExp(
        `\\b${escapeRegex(binding.name)}\\b`,
        "g",
      );
      const matches = [...body.matchAll(usagePattern)];
      if (matches.length > 1) {
        continue;
      }

      const linePattern = new RegExp(
        `^\\s*${binding.source}\\s+${escapeRegex(binding.name)}\\s*:`,
        "m",
      );
      const lineMatch = body.match(linePattern);
      if (!lineMatch) {
        continue;
      }
      const offsetInBody = body.indexOf(lineMatch[0]);
      const absoluteOffset =
        surface.bodyStartOffset +
        offsetInBody +
        lineMatch[0].indexOf(binding.name);
      findings.push(
        rangeFinding(
          lineStarts,
          absoluteOffset,
          absoluteOffset + binding.name.length,
          "allium.surface.unusedBinding",
          `Surface '${surface.name}' binding '${binding.name}' from '${binding.source}' is not used in the surface body.`,
          "warning",
        ),
      );
    }
  }

  return findings;
}

function findSurfacePathAndIterationIssues(
  text: string,
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Finding[] {
  const findings: Finding[] = [];
  const schemas = collectTypeSchemas(text);
  const surfaces = blocks.filter((block) => block.kind === "surface");

  for (const surface of surfaces) {
    const bindings = collectSurfaceBindingTypes(surface.body);
    const pathPattern = /\b([a-z_][a-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)+)\b/g;
    for (
      let path = pathPattern.exec(surface.body);
      path;
      path = pathPattern.exec(surface.body)
    ) {
      if (isCommentLineAtIndex(surface.body, path.index)) {
        continue;
      }
      if (isInsideDoubleQuotedStringAtIndex(surface.body, path.index)) {
        continue;
      }
      const value = path[1];
      const parts = value.split(".");
      const root = parts[0];
      const rootType = bindings.get(root);
      if (!rootType) {
        continue;
      }
      if (!isReachablePath(parts, rootType, schemas)) {
        const offset = surface.bodyStartOffset + path.index;
        findings.push(
          rangeFinding(
            lineStarts,
            offset,
            offset + value.length,
            "allium.surface.undefinedPath",
            `Surface '${surface.name}' references unknown path '${value}'.`,
            "error",
          ),
        );
      }
    }

    const iterationPattern =
      /^\s*for\s+([A-Za-z_][A-Za-z0-9_]*)\s+in\s+([A-Za-z_][A-Za-z0-9_.]*)\s*:/gm;
    for (
      let iter = iterationPattern.exec(surface.body);
      iter;
      iter = iterationPattern.exec(surface.body)
    ) {
      const collectionExpr = iter[2];
      const resolved = resolvePathType(
        collectionExpr.split("."),
        bindings,
        schemas,
      );
      if (resolved && resolved.isCollection) {
        bindings.set(iter[1], resolved.baseType ?? resolved.typeName);
        continue;
      }
      const offset =
        surface.bodyStartOffset + iter.index + iter[0].indexOf(collectionExpr);
      findings.push(
        rangeFinding(
          lineStarts,
          offset,
          offset + collectionExpr.length,
          "allium.surface.nonCollectionIteration",
          `Surface '${surface.name}' iterates over '${collectionExpr}', which is not known to be a collection.`,
          "error",
        ),
      );
    }
  }

  return findings;
}

function findSurfaceRuleCoverageIssues(
  _text: string,
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Finding[] {
  const findings: Finding[] = [];
  const ruleSuffixes = collectRulePathSuffixes(blocks);
  const surfaces = blocks.filter((block) => block.kind === "surface");
  const pathPattern = /\b([a-z_][a-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)+)\b/g;

  for (const surface of surfaces) {
    const bindings = collectSurfaceBindingTypes(surface.body);
    for (
      let path = pathPattern.exec(surface.body);
      path;
      path = pathPattern.exec(surface.body)
    ) {
      if (isCommentLineAtIndex(surface.body, path.index)) {
        continue;
      }
      const value = path[1];
      const parts = value.split(".");
      if (!bindings.has(parts[0])) {
        continue;
      }
      const suffix = parts.slice(1).join(".");
      if (suffix.length === 0) {
        continue;
      }
      const covered = [...ruleSuffixes].some(
        (ruleSuffix) =>
          suffix === ruleSuffix ||
          suffix.endsWith(`.${ruleSuffix}`) ||
          ruleSuffix.endsWith(`.${suffix}`),
      );
      if (covered) {
        continue;
      }
      const offset = surface.bodyStartOffset + path.index;
      findings.push(
        rangeFinding(
          lineStarts,
          offset,
          offset + value.length,
          "allium.surface.unusedPath",
          `Surface '${surface.name}' path '${value}' is not observed in rule field references.`,
          "info",
        ),
      );
    }
  }

  return findings;
}

function findSurfaceImpossibleWhenIssues(
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Finding[] {
  const findings: Finding[] = [];
  const surfaces = blocks.filter((block) => block.kind === "surface");
  const whenPattern = /\bwhen\s+(.+)$/;
  const comparisonPattern =
    /([A-Za-z_][A-Za-z0-9_.]*)\s*=\s*("[^"]*"|[a-z_][a-z0-9_]*|-?\d+(?:\.\d+)?)/g;

  for (const surface of surfaces) {
    let cursor = 0;
    while (cursor < surface.body.length) {
      const lineEnd = surface.body.indexOf("\n", cursor);
      const end = lineEnd >= 0 ? lineEnd : surface.body.length;
      const line = surface.body.slice(cursor, end);
      const whenMatch = line.match(whenPattern);
      if (!whenMatch) {
        cursor = end + 1;
        continue;
      }

      const condition = whenMatch[1];
      const equalsByExpr = new Map<string, Set<string>>();
      comparisonPattern.lastIndex = 0;
      for (
        let cmp = comparisonPattern.exec(condition);
        cmp;
        cmp = comparisonPattern.exec(condition)
      ) {
        const expr = cmp[1];
        const value = cmp[2];
        const set = equalsByExpr.get(expr) ?? new Set<string>();
        set.add(value);
        equalsByExpr.set(expr, set);
      }

      const contradictory = [...equalsByExpr.values()].some(
        (set) => set.size > 1,
      );
      if (contradictory) {
        const offset =
          surface.bodyStartOffset + cursor + line.indexOf(whenMatch[0]);
        findings.push(
          rangeFinding(
            lineStarts,
            offset,
            offset + whenMatch[0].length,
            "allium.surface.impossibleWhen",
            `Surface '${surface.name}' has a 'when' condition that appears contradictory and may never be true.`,
            "warning",
          ),
        );
      }

      cursor = end + 1;
    }
  }
  return findings;
}

function findSurfaceNamedBlockUniquenessIssues(
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Finding[] {
  const findings: Finding[] = [];
  const surfaces = blocks.filter((block) => block.kind === "surface");
  for (const surface of surfaces) {
    findings.push(
      ...findDuplicateNamedSurfaceBlocks(
        surface,
        lineStarts,
        "requires",
        "allium.surface.duplicateRequiresBlock",
      ),
    );
    findings.push(
      ...findDuplicateNamedSurfaceBlocks(
        surface,
        lineStarts,
        "provides",
        "allium.surface.duplicateProvidesBlock",
      ),
    );
  }
  return findings;
}

function findSurfaceRequiresDeferredHintIssues(
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
  text: string,
): Finding[] {
  const findings: Finding[] = [];
  const deferredNames = new Set<string>();
  const deferredPattern = /^\s*deferred\s+([A-Za-z_][A-Za-z0-9_.]*)\b/gm;
  for (
    let deferred = deferredPattern.exec(text);
    deferred;
    deferred = deferredPattern.exec(text)
  ) {
    deferredNames.add(deferred[1]);
  }

  const surfaces = blocks.filter((block) => block.kind === "surface");
  const requiresPattern = /^\s*requires\s+([A-Za-z_][A-Za-z0-9_]*)\s*:/gm;
  for (const surface of surfaces) {
    for (
      let match = requiresPattern.exec(surface.body);
      match;
      match = requiresPattern.exec(surface.body)
    ) {
      const requiresName = match[1];
      const hasDeferredHint = [...deferredNames].some(
        (name) => name === requiresName || name.endsWith(`.${requiresName}`),
      );
      if (hasDeferredHint) {
        continue;
      }
      const offset =
        surface.bodyStartOffset + match.index + match[0].indexOf(requiresName);
      findings.push(
        rangeFinding(
          lineStarts,
          offset,
          offset + requiresName.length,
          "allium.surface.requiresWithoutDeferred",
          `Named requires block '${requiresName}' in surface '${surface.name}' has no matching deferred specification hint.`,
          "warning",
        ),
      );
    }
  }

  return findings;
}

function findSurfaceProvidesTriggerIssues(
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
  text: string,
): Finding[] {
  const findings: Finding[] = [];
  const knownExternalTriggers = collectExternalStimulusTriggers(text);
  const surfaces = blocks.filter((block) => block.kind === "surface");
  for (const surface of surfaces) {
    const providesCalls = parseProvidesTriggerCalls(surface.body);
    for (const call of providesCalls) {
      if (knownExternalTriggers.has(call.name)) {
        continue;
      }
      const offset = surface.bodyStartOffset + call.offsetInBody;
      findings.push(
        rangeFinding(
          lineStarts,
          offset,
          offset + call.name.length,
          "allium.surface.undefinedProvidesTrigger",
          `Surface '${surface.name}' provides trigger '${call.name}' which is not defined as an external stimulus rule trigger.`,
          "error",
        ),
      );
    }
  }
  return findings;
}

function collectExternalStimulusTriggers(text: string): Set<string> {
  const out = new Set<string>();
  const rulePattern = /^\s*rule\s+[A-Za-z_][A-Za-z0-9_]*\s*\{([\s\S]*?)^\s*}/gm;
  for (let rule = rulePattern.exec(text); rule; rule = rulePattern.exec(text)) {
    const body = rule[1];
    const whenLine = body.match(/^\s*when\s*:\s*(.+)$/m);
    if (!whenLine) {
      continue;
    }
    const trigger = whenLine[1].trim();
    if (
      trigger.includes(":") ||
      /\b(becomes|<=|>=|<|>|and|or|if|exists)\b/.test(trigger)
    ) {
      continue;
    }
    const callMatch = trigger.match(/^([A-Za-z_][A-Za-z0-9_]*)\s*\(/);
    if (callMatch) {
      out.add(callMatch[1]);
    }
  }
  return out;
}

function parseProvidesTriggerCalls(
  body: string,
): Array<{ name: string; offsetInBody: number }> {
  const calls: Array<{ name: string; offsetInBody: number }> = [];
  const sectionPattern = /^(\s*)provides\s*:\s*$/gm;
  for (
    let section = sectionPattern.exec(body);
    section;
    section = sectionPattern.exec(body)
  ) {
    const baseIndent = (section[1] ?? "").length;
    let cursor = section.index + section[0].length + 1;
    while (cursor < body.length) {
      const lineEnd = body.indexOf("\n", cursor);
      const end = lineEnd >= 0 ? lineEnd : body.length;
      const line = body.slice(cursor, end);
      const trimmed = line.trim();
      const indent = (line.match(/^\s*/) ?? [""])[0].length;

      if (trimmed.length === 0) {
        cursor = end + 1;
        continue;
      }
      if (indent <= baseIndent) {
        break;
      }
      const callMatch = line.match(/([A-Za-z_][A-Za-z0-9_]*)\s*\(/);
      if (callMatch) {
        calls.push({
          name: callMatch[1],
          offsetInBody: cursor + line.indexOf(callMatch[1]),
        });
      }
      cursor = end + 1;
    }
  }
  return calls;
}

function findUnusedEntityIssues(text: string, lineStarts: number[]): Finding[] {
  const findings: Finding[] = [];
  const entityPattern =
    /^\s*(?:external\s+)?entity\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{/gm;
  for (
    let match = entityPattern.exec(text);
    match;
    match = entityPattern.exec(text)
  ) {
    const name = match[1];
    const usagePattern = new RegExp(`\\b${escapeRegex(name)}\\b`, "g");
    let count = 0;
    for (
      let usage = usagePattern.exec(text);
      usage;
      usage = usagePattern.exec(text)
    ) {
      if (isCommentLineAtIndex(text, usage.index)) {
        continue;
      }
      count += 1;
    }
    if (count > 1) {
      continue;
    }
    const offset = match.index + match[0].indexOf(name);
    findings.push(
      rangeFinding(
        lineStarts,
        offset,
        offset + name.length,
        "allium.entity.unused",
        `Entity '${name}' is declared but not referenced elsewhere in this specification.`,
        "warning",
      ),
    );
  }
  return findings;
}

function findUnusedNamedDefinitionIssues(
  text: string,
  lineStarts: number[],
): Finding[] {
  const findings: Finding[] = [];
  const definitions: Array<{ kind: string; name: string; offset: number }> = [];

  const valuePattern = /^\s*value\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{/gm;
  for (
    let match = valuePattern.exec(text);
    match;
    match = valuePattern.exec(text)
  ) {
    const name = match[1];
    definitions.push({
      kind: "value",
      name,
      offset: match.index + match[0].indexOf(name),
    });
  }

  const enumPattern = /^\s*enum\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{/gm;
  for (
    let match = enumPattern.exec(text);
    match;
    match = enumPattern.exec(text)
  ) {
    const name = match[1];
    definitions.push({
      kind: "enum",
      name,
      offset: match.index + match[0].indexOf(name),
    });
  }

  const defaultPattern =
    /^\s*default\s+[A-Za-z_][A-Za-z0-9_]*(?:\s+([A-Za-z_][A-Za-z0-9_]*))?\s*=/gm;
  for (
    let match = defaultPattern.exec(text);
    match;
    match = defaultPattern.exec(text)
  ) {
    const name = match[1];
    if (!name) {
      continue;
    }
    definitions.push({
      kind: "default instance",
      name,
      offset: match.index + match[0].indexOf(name),
    });
  }

  for (const definition of definitions) {
    const usagePattern = new RegExp(
      `\\b${escapeRegex(definition.name)}\\b`,
      "g",
    );
    let count = 0;
    for (
      let usage = usagePattern.exec(text);
      usage;
      usage = usagePattern.exec(text)
    ) {
      if (isCommentLineAtIndex(text, usage.index)) {
        continue;
      }
      count += 1;
    }
    if (count > 1) {
      continue;
    }
    findings.push(
      rangeFinding(
        lineStarts,
        definition.offset,
        definition.offset + definition.name.length,
        "allium.definition.unused",
        `${capitalize(definition.kind)} '${definition.name}' is declared but not referenced elsewhere.`,
        "warning",
      ),
    );
  }

  return findings;
}

function findUnusedFieldIssues(text: string, lineStarts: number[]): Finding[] {
  const findings: Finding[] = [];
  const fields = collectDeclaredEntityFields(text);
  if (fields.length === 0) {
    return findings;
  }
  for (const field of fields) {
    const usagePattern = new RegExp(`\\.${escapeRegex(field.name)}\\b`, "g");
    let count = 0;
    for (
      let usage = usagePattern.exec(text);
      usage;
      usage = usagePattern.exec(text)
    ) {
      if (isCommentLineAtIndex(text, usage.index)) {
        continue;
      }
      count += 1;
    }
    if (count > 0) {
      continue;
    }
    findings.push(
      rangeFinding(
        lineStarts,
        field.offset,
        field.offset + field.name.length,
        "allium.field.unused",
        `Field '${field.entity}.${field.name}' is declared but not referenced elsewhere.`,
        "info",
      ),
    );
  }
  return findings;
}

function findUnreachableRuleTriggerIssues(
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Finding[] {
  const findings: Finding[] = [];
  const surfaces = blocks.filter((block) => block.kind === "surface");
  const provided = new Set<string>();
  for (const surface of surfaces) {
    for (const trigger of parseProvidesTriggerCalls(surface.body)) {
      provided.add(trigger.name);
    }
  }

  const produced = new Set<string>();
  const rules = blocks.filter((block) => block.kind === "rule");
  const ensureCallPattern = /^\s*ensures\s*:\s*([A-Za-z_][A-Za-z0-9_]*)\s*\(/gm;
  for (const rule of rules) {
    for (
      let match = ensureCallPattern.exec(rule.body);
      match;
      match = ensureCallPattern.exec(rule.body)
    ) {
      produced.add(match[1]);
    }
  }

  for (const rule of rules) {
    const whenLine = rule.body.match(/^\s*when\s*:\s*(.+)$/m);
    if (!whenLine) {
      continue;
    }
    const triggerLine = whenLine[1].trim();
    if (
      triggerLine.includes(":") ||
      /\b(becomes|<=|>=|<|>|if|exists)\b/.test(triggerLine)
    ) {
      continue;
    }

    const callPattern = /([A-Za-z_][A-Za-z0-9_]*)\s*\(/g;
    for (
      let call = callPattern.exec(triggerLine);
      call;
      call = callPattern.exec(triggerLine)
    ) {
      const callName = call[1];
      if (provided.has(callName) || produced.has(callName)) {
        continue;
      }
      const callOffset =
        rule.bodyStartOffset +
        rule.body.indexOf(whenLine[0]) +
        whenLine[0].indexOf(callName);
      findings.push(
        rangeFinding(
          lineStarts,
          callOffset,
          callOffset + callName.length,
          "allium.rule.unreachableTrigger",
          `Rule '${rule.name}' listens for trigger '${callName}' but no local surface provides or rule emits it.`,
          "info",
        ),
      );
    }
  }

  return findings;
}

function findExternalEntitySourceHints(
  text: string,
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Finding[] {
  const findings: Finding[] = [];
  const hasImports = blocks.some((block) => block.kind === "use");
  if (hasImports) {
    return findings;
  }
  const ruleBlocks = blocks.filter((block) => block.kind === "rule");
  const pattern = /^\s*external\s+entity\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{/gm;
  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    const name = match[1];
    const offset = match.index + match[0].indexOf(name);
    const namePattern = new RegExp(`\\b${escapeRegex(name)}\\b`);
    const referencedInRules = ruleBlocks.some((rule) =>
      namePattern.test(rule.body),
    );
    findings.push(
      rangeFinding(
        lineStarts,
        offset,
        offset + name.length,
        "allium.externalEntity.missingSourceHint",
        `External entity '${name}' has no obvious governing specification import in this module.`,
        referencedInRules ? "info" : "warning",
      ),
    );
  }
  return findings;
}

function findDeferredLocationHints(
  text: string,
  lineStarts: number[],
): Finding[] {
  const findings: Finding[] = [];
  const pattern = /^\s*deferred\s+([A-Za-z_][A-Za-z0-9_.]*)(.*)$/gm;
  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    const suffix = (match[2] ?? "").trim();
    if (
      suffix.includes("http://") ||
      suffix.includes("https://") ||
      suffix.includes('"')
    ) {
      continue;
    }
    const name = match[1];
    const offset = match.index + match[0].indexOf(name);
    findings.push(
      rangeFinding(
        lineStarts,
        offset,
        offset + name.length,
        "allium.deferred.missingLocationHint",
        `Deferred specification '${name}' should include a location hint.`,
        "warning",
      ),
    );
  }
  return findings;
}

function findImplicitLambdaIssues(
  text: string,
  lineStarts: number[],
): Finding[] {
  const findings: Finding[] = [];
  const pattern = /\.((?:any|all|each))\(\s*([A-Za-z_][A-Za-z0-9_]*)\s*\)/g;

  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    if (isCommentLineAtIndex(text, match.index)) {
      continue;
    }
    const operator = match[1];
    const shorthand = match[2];
    const shorthandOffset = match.index + match[0].lastIndexOf(shorthand);
    findings.push(
      rangeFinding(
        lineStarts,
        shorthandOffset,
        shorthandOffset + shorthand.length,
        "allium.expression.implicitLambda",
        `Collection operator '${operator}' must use an explicit lambda (for example 'x => ...') instead of shorthand '${shorthand}'.`,
        "error",
      ),
    );
  }

  return findings;
}

function findNeverFireRuleIssues(
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Finding[] {
  const findings: Finding[] = [];
  const rules = blocks.filter((block) => block.kind === "rule");
  for (const rule of rules) {
    const requires = collectRuleClauseLines(rule.body).filter(
      (line) => line.clause === "requires",
    );
    const equalsByExpr = new Map<string, Set<string>>();
    const notEqualsByExpr = new Map<string, Set<string>>();

    for (const line of requires) {
      const match = line.text.match(
        /([A-Za-z_][A-Za-z0-9_.]*)\s*(=|!=)\s*("[^"]*"|[a-z_][a-z0-9_]*|-?\d+(?:\.\d+)?)/,
      );
      if (!match) {
        continue;
      }
      const expr = match[1];
      const operator = match[2];
      const value = match[3];
      if (operator === "=") {
        const set = equalsByExpr.get(expr) ?? new Set<string>();
        set.add(value);
        equalsByExpr.set(expr, set);
      } else {
        const set = notEqualsByExpr.get(expr) ?? new Set<string>();
        set.add(value);
        notEqualsByExpr.set(expr, set);
      }
    }

    let contradictory = false;
    for (const [expr, equals] of equalsByExpr.entries()) {
      if (equals.size > 1) {
        contradictory = true;
      }
      const notEquals = notEqualsByExpr.get(expr);
      if (notEquals && [...equals].some((value) => notEquals.has(value))) {
        contradictory = true;
      }
    }

    if (!contradictory) {
      continue;
    }
    findings.push(
      rangeFinding(
        lineStarts,
        rule.nameStartOffset,
        rule.nameStartOffset + rule.name.length,
        "allium.rule.neverFires",
        `Rule '${rule.name}' has contradictory requires constraints and may never fire.`,
        "warning",
      ),
    );
  }
  return findings;
}

function findInvalidTriggerIssues(
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Finding[] {
  const findings: Finding[] = [];
  const rules = blocks.filter((block) => block.kind === "rule");
  for (const rule of rules) {
    const whenMatch = rule.body.match(/^\s*when\s*:\s*(.+)$/m);
    if (!whenMatch) {
      continue;
    }
    const trigger = whenMatch[1].trim();
    if (isValidTriggerShape(trigger)) {
      continue;
    }
    const lineOffset = rule.bodyStartOffset + rule.body.indexOf(whenMatch[0]);
    findings.push(
      rangeFinding(
        lineStarts,
        lineOffset,
        lineOffset + whenMatch[0].length,
        "allium.rule.invalidTrigger",
        `Rule '${rule.name}' uses an unsupported trigger form in 'when:'.`,
        "error",
      ),
    );
  }
  return findings;
}

function isValidTriggerShape(trigger: string): boolean {
  const callPattern =
    /^[A-Za-z_][A-Za-z0-9_]*(?:\/[A-Za-z_][A-Za-z0-9_]*)?\s*\([^)]*\)\s*$/;
  if (callPattern.test(trigger)) {
    return true;
  }
  if (/\b(and|or)\b/.test(trigger)) {
    const parts = trigger
      .split(/\b(?:and|or)\b/)
      .map((part) => part.trim())
      .filter((part) => part.length > 0);
    if (parts.length > 1 && parts.every((part) => callPattern.test(part))) {
      return true;
    }
  }

  const typedPattern =
    /^([a-z_][a-z0-9_]*)\s*:\s*([A-Za-z_][A-Za-z0-9_]*(?:\/[A-Za-z_][A-Za-z0-9_]*)?)\.(.+)$/;
  const typedMatch = trigger.match(typedPattern);
  if (!typedMatch) {
    return false;
  }
  const tail = typedMatch[3].trim();
  if (/^created\b/.test(tail)) {
    return true;
  }
  if (/\bbecomes\b/.test(tail)) {
    return true;
  }
  if (/\btransitions_to\b/.test(tail)) {
    return true;
  }
  if (/(<=|>=|<|>)\s*now\b/.test(tail) || /\bnow\s*[-+]\s*\d/.test(tail)) {
    return true;
  }
  if (/^[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*$/.test(tail)) {
    return true;
  }
  return false;
}

function findDuplicateRuleBehaviourIssues(
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Finding[] {
  const findings: Finding[] = [];
  const rules = blocks.filter((block) => block.kind === "rule");
  const signatureMap = new Map<string, typeof rules>();

  for (const rule of rules) {
    const signature = canonicalRuleSignature(rule.body);
    const existing = signatureMap.get(signature) ?? [];
    existing.push(rule);
    signatureMap.set(signature, existing);
  }

  for (const group of signatureMap.values()) {
    if (group.length < 2) {
      continue;
    }
    for (let i = 1; i < group.length; i += 1) {
      const duplicate = group[i];
      findings.push(
        rangeFinding(
          lineStarts,
          duplicate.startOffset,
          duplicate.startOffset + duplicate.name.length,
          "allium.rule.duplicateBehavior",
          `Rule '${duplicate.name}' duplicates behavior already expressed by '${group[0].name}'.`,
          "warning",
        ),
      );
    }
  }

  for (let i = 0; i < rules.length; i += 1) {
    for (let j = i + 1; j < rules.length; j += 1) {
      const shadowed = detectShadowedPair(rules[i], rules[j]);
      if (!shadowed) {
        continue;
      }
      findings.push(
        rangeFinding(
          lineStarts,
          shadowed.shadowed.startOffset,
          shadowed.shadowed.startOffset + shadowed.shadowed.name.length,
          "allium.rule.potentialShadow",
          `Rule '${shadowed.shadowed.name}' may be shadowed by broader rule '${shadowed.broader.name}'.`,
          "info",
        ),
      );
    }
  }

  return findings;
}

function canonicalRuleSignature(ruleBody: string): string {
  const when = (ruleBody.match(/^\s*when\s*:\s*(.+)$/m)?.[1] ?? "").trim();
  const requires = collectRuleClauseLines(ruleBody)
    .filter((line) => line.clause === "requires")
    .map((line) => normalizeClauseText(line.text))
    .sort();
  const ensures = collectRuleClauseLines(ruleBody)
    .filter((line) => line.clause === "ensures")
    .map((line) => normalizeClauseText(line.text))
    .sort();

  return `when:${normalizeClauseText(when)}|requires:${requires.join(",")}|ensures:${ensures.join(",")}`;
}

function detectShadowedPair(
  left: ReturnType<typeof parseAlliumBlocks>[number],
  right: ReturnType<typeof parseAlliumBlocks>[number],
): {
  broader: ReturnType<typeof parseAlliumBlocks>[number];
  shadowed: ReturnType<typeof parseAlliumBlocks>[number];
} | null {
  const leftWhen = (left.body.match(/^\s*when\s*:\s*(.+)$/m)?.[1] ?? "").trim();
  const rightWhen = (
    right.body.match(/^\s*when\s*:\s*(.+)$/m)?.[1] ?? ""
  ).trim();
  if (normalizeClauseText(leftWhen) !== normalizeClauseText(rightWhen)) {
    return null;
  }

  const leftEnsures = new Set(
    collectRuleClauseLines(left.body)
      .filter((line) => line.clause === "ensures")
      .map((line) => normalizeClauseText(line.text)),
  );
  const rightEnsures = new Set(
    collectRuleClauseLines(right.body)
      .filter((line) => line.clause === "ensures")
      .map((line) => normalizeClauseText(line.text)),
  );
  if (
    leftEnsures.size !== rightEnsures.size ||
    [...leftEnsures].some((item) => !rightEnsures.has(item))
  ) {
    return null;
  }

  const leftRequires = new Set(
    collectRuleClauseLines(left.body)
      .filter((line) => line.clause === "requires")
      .map((line) => normalizeClauseText(line.text)),
  );
  const rightRequires = new Set(
    collectRuleClauseLines(right.body)
      .filter((line) => line.clause === "requires")
      .map((line) => normalizeClauseText(line.text)),
  );

  if (
    isSubset(leftRequires, rightRequires) &&
    leftRequires.size < rightRequires.size
  ) {
    return { broader: left, shadowed: right };
  }
  if (
    isSubset(rightRequires, leftRequires) &&
    rightRequires.size < leftRequires.size
  ) {
    return { broader: right, shadowed: left };
  }
  return null;
}

function isSubset(values: Set<string>, maybeSuperset: Set<string>): boolean {
  for (const value of values) {
    if (!maybeSuperset.has(value)) {
      return false;
    }
  }
  return true;
}

function normalizeClauseText(value: string): string {
  return value.trim().replace(/\s+/g, " ");
}

function findExpressionTypeMismatchIssues(
  lineStarts: number[],
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Finding[] {
  const findings: Finding[] = [];
  const rules = blocks.filter((block) => block.kind === "rule");
  for (const rule of rules) {
    const clauseLines = collectRuleClauseLines(rule.body);
    for (const line of clauseLines) {
      if (line.clause !== "requires" && line.clause !== "ensures") {
        continue;
      }
      if (isCommentLineAtIndex(rule.body, line.startOffset)) {
        continue;
      }

      const comparison = line.text.match(
        /("[^"]*"|[A-Za-z_][A-Za-z0-9_.]*)\s*(<=|>=|<|>)\s*("[^"]*"|[A-Za-z_][A-Za-z0-9_.]*|-?\d+(?:\.\d+)?)/,
      );
      if (comparison) {
        const lhs = comparison[1];
        const rhs = comparison[3];
        if (lhs.startsWith('"') || rhs.startsWith('"')) {
          const mismatchOffset =
            rule.bodyStartOffset +
            line.startOffset +
            line.text.indexOf(comparison[0]);
          findings.push(
            rangeFinding(
              lineStarts,
              mismatchOffset,
              mismatchOffset + comparison[0].length,
              "allium.expression.typeMismatch",
              `Comparison '${comparison[0]}' mixes string and ordered comparison operators.`,
              "error",
            ),
          );
          continue;
        }
      }

      const arithmetic = line.text.match(
        /("[^"]*"|-?\d+(?:\.\d+)?|[A-Za-z_][A-Za-z0-9_.]*)\s*([+\-*/])\s*("[^"]*"|-?\d+(?:\.\d+)?|[A-Za-z_][A-Za-z0-9_.]*)/,
      );
      if (arithmetic) {
        const lhs = arithmetic[1];
        const rhs = arithmetic[3];
        if (lhs.startsWith('"') || rhs.startsWith('"')) {
          const mismatchOffset =
            rule.bodyStartOffset +
            line.startOffset +
            line.text.indexOf(arithmetic[0]);
          findings.push(
            rangeFinding(
              lineStarts,
              mismatchOffset,
              mismatchOffset + arithmetic[0].length,
              "allium.expression.typeMismatch",
              `Arithmetic expression '${arithmetic[0]}' mixes string and numeric terms.`,
              "error",
            ),
          );
        }
      }

      const equality = line.text.match(
        /("[^"]*"|-?\d+(?:\.\d+)?)\s*(=|!=)\s*("[^"]*"|-?\d+(?:\.\d+)?)/,
      );
      if (equality) {
        const lhs = equality[1];
        const rhs = equality[3];
        const lhsString = lhs.startsWith('"');
        const rhsString = rhs.startsWith('"');
        if (lhsString !== rhsString) {
          const mismatchOffset =
            rule.bodyStartOffset +
            line.startOffset +
            line.text.indexOf(equality[0]);
          findings.push(
            rangeFinding(
              lineStarts,
              mismatchOffset,
              mismatchOffset + equality[0].length,
              "allium.expression.typeMismatch",
              `Equality expression '${equality[0]}' compares incompatible literal types.`,
              "error",
            ),
          );
        }
      }
    }
  }
  return findings;
}

function capitalize(value: string): string {
  return value[0].toUpperCase() + value.slice(1);
}

function findDerivedCircularDependencyIssues(
  text: string,
  lineStarts: number[],
): Finding[] {
  const findings: Finding[] = [];
  const entities = parseEntityDerivedDefinitions(text);
  for (const entity of entities) {
    const byName = new Map(entity.derived.map((item) => [item.name, item]));
    const graph = new Map<string, Set<string>>();
    for (const item of entity.derived) {
      const deps = new Set<string>();
      const tokenPattern = /\b([A-Za-z_][A-Za-z0-9_]*)\b/g;
      for (
        let token = tokenPattern.exec(item.expression);
        token;
        token = tokenPattern.exec(item.expression)
      ) {
        const name = token[1];
        if (name === item.name || !byName.has(name)) {
          continue;
        }
        deps.add(name);
      }
      graph.set(item.name, deps);
    }

    const visiting = new Set<string>();
    const visited = new Set<string>();
    const cycleMembers = new Set<string>();

    const dfs = (node: string): void => {
      if (visited.has(node) || cycleMembers.has(node)) {
        return;
      }
      if (visiting.has(node)) {
        cycleMembers.add(node);
        return;
      }
      visiting.add(node);
      const deps = graph.get(node) ?? new Set<string>();
      for (const dep of deps) {
        if (visiting.has(dep)) {
          cycleMembers.add(node);
          cycleMembers.add(dep);
          continue;
        }
        dfs(dep);
        if (cycleMembers.has(dep)) {
          cycleMembers.add(node);
        }
      }
      visiting.delete(node);
      visited.add(node);
    };

    for (const name of graph.keys()) {
      dfs(name);
    }

    for (const name of cycleMembers) {
      const item = byName.get(name);
      if (!item) {
        continue;
      }
      findings.push(
        rangeFinding(
          lineStarts,
          item.startOffset,
          item.startOffset + item.name.length,
          "allium.derived.circularDependency",
          `Derived value '${entity.name}.${name}' participates in a circular dependency.`,
          "error",
        ),
      );
    }
  }
  return findings;
}

function findDuplicateNamedSurfaceBlocks(
  surface: ReturnType<typeof parseAlliumBlocks>[number],
  lineStarts: number[],
  keyword: "requires" | "provides",
  code: string,
): Finding[] {
  const findings: Finding[] = [];
  const seen = new Set<string>();
  const pattern = new RegExp(
    `^\\s*${keyword}\\s+([A-Za-z_][A-Za-z0-9_]*)\\s*:`,
    "gm",
  );
  for (
    let match = pattern.exec(surface.body);
    match;
    match = pattern.exec(surface.body)
  ) {
    const name = match[1];
    if (!seen.has(name)) {
      seen.add(name);
      continue;
    }
    const offset =
      surface.bodyStartOffset + match.index + match[0].indexOf(name);
    findings.push(
      rangeFinding(
        lineStarts,
        offset,
        offset + name.length,
        code,
        `Surface '${surface.name}' has duplicate named '${keyword}' block '${name}'.`,
        "error",
      ),
    );
  }
  return findings;
}

function parseRelatedReferences(
  body: string,
): Array<{ name: string; offsetInBody: number }> {
  const refs: Array<{ name: string; offsetInBody: number }> = [];
  const relatedPattern = /^(\s*)related\s*:\s*$/gm;
  for (
    let related = relatedPattern.exec(body);
    related;
    related = relatedPattern.exec(body)
  ) {
    const baseIndent = (related[1] ?? "").length;
    const sectionStart = related.index + related[0].length + 1;
    let cursor = sectionStart;

    while (cursor < body.length) {
      const nextNewline = body.indexOf("\n", cursor);
      const lineEnd = nextNewline >= 0 ? nextNewline : body.length;
      const line = body.slice(cursor, lineEnd);
      const trimmed = line.trim();
      const indent = (line.match(/^\s*/) ?? [""])[0].length;

      if (trimmed.length === 0) {
        cursor = lineEnd + 1;
        continue;
      }
      if (indent <= baseIndent) {
        break;
      }
      if (!trimmed.startsWith("--")) {
        const clauseMatch = trimmed.match(
          /^([A-Za-z_][A-Za-z0-9_]*)(?:\s*\(.*\))?(?:\s+when\s+.*)?$/,
        );
        if (clauseMatch) {
          const nameStart = line.indexOf(clauseMatch[1]);
          refs.push({
            name: clauseMatch[1],
            offsetInBody: cursor + nameStart,
          });
        }
      }

      cursor = lineEnd + 1;
    }
  }
  return refs;
}

function parseVariantDeclarations(
  text: string,
): Array<{ name: string; base: string; startOffset: number }> {
  const out: Array<{ name: string; base: string; startOffset: number }> = [];
  const pattern =
    /^\s*variant\s+([A-Za-z_][A-Za-z0-9_]*)\s*:\s*([A-Za-z_][A-Za-z0-9_]*)\s*\{/gm;
  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    out.push({
      name: match[1],
      base: match[2],
      startOffset: match.index + match[0].indexOf(match[1]),
    });
  }
  return out;
}

function collectTypeSchemas(
  text: string,
): Map<string, Map<string, { typeName: string; isCollection: boolean }>> {
  const schemas = new Map<
    string,
    Map<string, { typeName: string; isCollection: boolean }>
  >();
  const blockPattern =
    /^\s*(?:external\s+entity|entity|value|variant)\s+([A-Za-z_][A-Za-z0-9_]*)(?:\s*:\s*[A-Za-z_][A-Za-z0-9_]*)?\s*\{/gm;
  for (
    let block = blockPattern.exec(text);
    block;
    block = blockPattern.exec(text)
  ) {
    const typeName = block[1];
    const open = text.indexOf("{", block.index);
    if (open < 0) {
      continue;
    }
    const close = findMatchingBrace(text, open);
    if (close < 0) {
      continue;
    }
    const body = text.slice(open + 1, close);
    const fields = new Map<
      string,
      { typeName: string; isCollection: boolean }
    >();
    const fieldPattern = /^\s*([A-Za-z_][A-Za-z0-9_]*)\s*:\s*(.+)\s*$/gm;
    for (
      let field = fieldPattern.exec(body);
      field;
      field = fieldPattern.exec(body)
    ) {
      const name = field[1];
      const rhs = field[2].trim();
      if (
        /^[A-Za-z_][A-Za-z0-9_]*\s+for\s+this\s+[A-Za-z_][A-Za-z0-9_]*$/.test(
          rhs,
        )
      ) {
        const relType = rhs.split(/\s+/)[0];
        fields.set(name, { typeName: relType, isCollection: true });
        continue;
      }
      if (rhs.includes(" with ")) {
        fields.set(name, { typeName, isCollection: true });
        continue;
      }
      const cleaned = rhs.replace(/\?$/, "");
      const genericMatch = cleaned.match(
        /^(List|Set|Map)<\s*([A-Za-z_][A-Za-z0-9_]*)/,
      );
      if (genericMatch) {
        fields.set(name, { typeName: genericMatch[2], isCollection: true });
        continue;
      }
      const direct = cleaned.match(/^([A-Za-z_][A-Za-z0-9_]*)$/);
      if (direct) {
        fields.set(name, { typeName: direct[1], isCollection: false });
      }
    }
    schemas.set(typeName, fields);
  }
  return schemas;
}

function collectRulePathSuffixes(
  blocks: ReturnType<typeof parseAlliumBlocks>,
): Set<string> {
  const suffixes = new Set<string>();
  const rules = blocks.filter((block) => block.kind === "rule");
  const pattern = /\b([a-z_][a-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)+)\b/g;
  for (const rule of rules) {
    for (
      let match = pattern.exec(rule.body);
      match;
      match = pattern.exec(rule.body)
    ) {
      if (isCommentLineAtIndex(rule.body, match.index)) {
        continue;
      }
      const parts = match[1].split(".");
      if (parts.length < 2) {
        continue;
      }
      suffixes.add(parts.slice(1).join("."));
    }
  }
  return suffixes;
}

function collectSurfaceBindingTypes(body: string): Map<string, string> {
  const bindings = new Map<string, string>();
  const patterns = [
    /^\s*facing\s+([A-Za-z_][A-Za-z0-9_]*)\s*:\s*([A-Za-z_][A-Za-z0-9_]*)/m,
    /^\s*context\s+([A-Za-z_][A-Za-z0-9_]*)\s*:\s*([A-Za-z_][A-Za-z0-9_]*)/m,
  ];
  for (const pattern of patterns) {
    const match = body.match(pattern);
    if (match) {
      bindings.set(match[1], match[2]);
    }
  }
  return bindings;
}

function isReachablePath(
  parts: string[],
  rootType: string,
  schemas: Map<
    string,
    Map<string, { typeName: string; isCollection: boolean }>
  >,
): boolean {
  let currentType = rootType;
  for (let i = 1; i < parts.length; i += 1) {
    const fields = schemas.get(currentType);
    if (!fields) {
      return true;
    }
    const field = fields.get(parts[i]);
    if (!field) {
      return false;
    }
    currentType = field.typeName;
  }
  return true;
}

function resolvePathType(
  parts: string[],
  bindings: Map<string, string>,
  schemas: Map<
    string,
    Map<string, { typeName: string; isCollection: boolean }>
  >,
): { typeName: string; baseType: string; isCollection: boolean } | null {
  const root = parts[0];
  const rootType = bindings.get(root);
  if (!rootType) {
    return null;
  }
  let currentType = rootType;
  let current: { typeName: string; baseType: string; isCollection: boolean } = {
    typeName: rootType,
    baseType: rootType,
    isCollection: false,
  };
  for (let i = 1; i < parts.length; i += 1) {
    const fields = schemas.get(currentType);
    if (!fields) {
      return null;
    }
    const field = fields.get(parts[i]);
    if (!field) {
      return null;
    }
    current = {
      typeName: field.typeName,
      baseType: field.typeName,
      isCollection: field.isCollection,
    };
    currentType = field.typeName;
  }
  return current;
}

function parseVariantFieldDefinitions(text: string): Array<{
  name: string;
  base: string;
  fields: string[];
}> {
  const out: Array<{ name: string; base: string; fields: string[] }> = [];
  const pattern =
    /^\s*variant\s+([A-Za-z_][A-Za-z0-9_]*)\s*:\s*([A-Za-z_][A-Za-z0-9_]*)\s*\{/gm;
  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    const open = text.indexOf("{", match.index);
    if (open < 0) {
      continue;
    }
    const close = findMatchingBrace(text, open);
    if (close < 0) {
      continue;
    }
    const body = text.slice(open + 1, close);
    const fields: string[] = [];
    const fieldPattern = /^\s*([A-Za-z_][A-Za-z0-9_]*)\s*:/gm;
    for (
      let field = fieldPattern.exec(body);
      field;
      field = fieldPattern.exec(body)
    ) {
      fields.push(field[1]);
    }
    out.push({ name: match[1], base: match[2], fields });
  }
  return out;
}

function collectDiscriminatorFieldsByEntity(text: string): Map<string, string> {
  const out = new Map<string, string>();
  const entities = parseEntityBlocks(text);
  for (const entity of entities) {
    for (const field of entity.pipeFields) {
      if (!field.hasCapitalizedName || !field.allNamesCapitalized) {
        continue;
      }
      out.set(entity.name, field.fieldName);
    }
  }
  return out;
}

function parseEntityDerivedDefinitions(text: string): Array<{
  name: string;
  derived: Array<{ name: string; expression: string; startOffset: number }>;
}> {
  const entities: Array<{
    name: string;
    derived: Array<{ name: string; expression: string; startOffset: number }>;
  }> = [];
  const pattern =
    /^\s*(?:external\s+)?entity\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{/gm;
  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    const open = text.indexOf("{", match.index);
    if (open < 0) {
      continue;
    }
    const close = findMatchingBrace(text, open);
    if (close < 0) {
      continue;
    }
    const body = text.slice(open + 1, close);
    const derived: Array<{
      name: string;
      expression: string;
      startOffset: number;
    }> = [];
    let cursor = 0;
    while (cursor < body.length) {
      const lineEnd = body.indexOf("\n", cursor);
      const end = lineEnd >= 0 ? lineEnd : body.length;
      const line = body.slice(cursor, end);
      const fieldMatch = line.match(
        /^\s*([A-Za-z_][A-Za-z0-9_]*)\s*:\s*(.+)\s*$/,
      );
      if (fieldMatch) {
        const fieldName = fieldMatch[1];
        const rhs = fieldMatch[2].trim();
        if (looksLikeDerivedExpression(rhs)) {
          derived.push({
            name: fieldName,
            expression: rhs,
            startOffset: open + 1 + cursor + line.indexOf(fieldName),
          });
        }
      }
      cursor = end + 1;
    }
    entities.push({ name: match[1], derived });
  }
  return entities;
}

function looksLikeDerivedExpression(rhs: string): boolean {
  if (/^[A-Z][A-Za-z0-9_]*(?:\??|<[A-Za-z0-9_<>, ?/|]+>)?$/.test(rhs)) {
    return false;
  }
  if (/^[a-z_][a-z0-9_]*(?:\s*\|\s*[a-z_][a-z0-9_]*)+$/.test(rhs)) {
    return false;
  }
  if (
    /^[A-Za-z_][A-Za-z0-9_]*\s+for\s+this\s+[A-Za-z_][A-Za-z0-9_]*$/.test(rhs)
  ) {
    return false;
  }
  return /[.()><=+\-*/]|(\bwith\b)|(\bcount\b)|(\bany\b)|(\ball\b)|(\bnow\b)/.test(
    rhs,
  );
}

function collectDeclaredTypeNames(text: string): string[] {
  const out = new Set<string>();
  const patterns = [
    /^\s*(?:external\s+)?entity\s+([A-Za-z_][A-Za-z0-9_]*)\b/gm,
    /^\s*value\s+([A-Za-z_][A-Za-z0-9_]*)\b/gm,
    /^\s*variant\s+([A-Za-z_][A-Za-z0-9_]*)\b/gm,
    /^\s*enum\s+([A-Za-z_][A-Za-z0-9_]*)\b/gm,
    /^\s*actor\s+([A-Za-z_][A-Za-z0-9_]*)\b/gm,
  ];
  for (const pattern of patterns) {
    for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
      out.add(match[1]);
    }
  }
  return [...out];
}

function collectFieldTypeSites(
  text: string,
): Array<{ typeExpression: string; startOffset: number }> {
  const out: Array<{ typeExpression: string; startOffset: number }> = [];
  const blockPattern =
    /^\s*(?:external\s+entity|entity|value|variant)\s+[A-Za-z_][A-Za-z0-9_]*(?:\s*:\s*[A-Za-z_][A-Za-z0-9_]*)?\s*\{/gm;
  for (
    let block = blockPattern.exec(text);
    block;
    block = blockPattern.exec(text)
  ) {
    const open = text.indexOf("{", block.index);
    if (open < 0) {
      continue;
    }
    const close = findMatchingBrace(text, open);
    if (close < 0) {
      continue;
    }
    const body = text.slice(open + 1, close);
    const fieldPattern = /^\s*[A-Za-z_][A-Za-z0-9_]*\s*:\s*([^=\n]+)$/gm;
    for (
      let field = fieldPattern.exec(body);
      field;
      field = fieldPattern.exec(body)
    ) {
      const typeExpression = field[1].replace(/\s--.*$/, "").trim();
      out.push({
        typeExpression,
        startOffset: open + 1 + field.index + field[0].indexOf(typeExpression),
      });
    }
  }
  return out;
}

function collectRelationshipTypeSites(
  text: string,
): Array<{ targetType: string; startOffset: number }> {
  const out: Array<{ targetType: string; startOffset: number }> = [];
  const blockPattern =
    /^\s*(?:external\s+entity|entity|value|variant)\s+[A-Za-z_][A-Za-z0-9_]*(?:\s*:\s*[A-Za-z_][A-Za-z0-9_]*)?\s*\{/gm;
  for (
    let block = blockPattern.exec(text);
    block;
    block = blockPattern.exec(text)
  ) {
    const open = text.indexOf("{", block.index);
    if (open < 0) {
      continue;
    }
    const close = findMatchingBrace(text, open);
    if (close < 0) {
      continue;
    }
    const body = text.slice(open + 1, close);
    const relationshipPattern =
      /^\s*[A-Za-z_][A-Za-z0-9_]*\s*:\s*([A-Za-z_][A-Za-z0-9_]*(?:\/[A-Za-z_][A-Za-z0-9_]*)?)\s+for\s+this\s+[A-Za-z_][A-Za-z0-9_]*\s*$/gm;
    for (
      let rel = relationshipPattern.exec(body);
      rel;
      rel = relationshipPattern.exec(body)
    ) {
      const targetType = rel[1];
      out.push({
        targetType,
        startOffset: open + 1 + rel.index + rel[0].indexOf(targetType),
      });
    }
  }
  return out;
}

function looksLikePluralTypeName(typeName: string): boolean {
  if (!/^[A-Z][A-Za-z0-9_]*$/.test(typeName)) {
    return false;
  }
  if (/(ss|us|is)$/.test(typeName)) {
    return false;
  }
  return typeName.endsWith("s");
}

function collectEntityStatusEnums(text: string): Map<string, Set<string>> {
  const out = new Map<string, Set<string>>();
  const entityPattern =
    /^\s*(?:external\s+)?entity\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{/gm;
  for (
    let entity = entityPattern.exec(text);
    entity;
    entity = entityPattern.exec(text)
  ) {
    const open = text.indexOf("{", entity.index);
    if (open < 0) {
      continue;
    }
    const close = findMatchingBrace(text, open);
    if (close < 0) {
      continue;
    }
    const body = text.slice(open + 1, close);
    const statusField = body.match(
      /^\s*status\s*:\s*([a-z_][a-z0-9_]*(?:\s*\|\s*[a-z_][a-z0-9_]*)+)\s*$/m,
    );
    if (!statusField) {
      continue;
    }
    const values = statusField[1]
      .split("|")
      .map((v) => v.trim())
      .filter((v) => v.length > 0);
    out.set(entity[1], new Set(values));
  }
  return out;
}

function validateTypeNameReference(
  typeName: string,
  offset: number,
  lineStarts: number[],
  declaredTypes: Set<string>,
  aliases: Set<string>,
  undefinedTypeCode: string,
  undefinedAliasCode: string,
): Finding[] {
  if (typeName.includes("/")) {
    const alias = typeName.split("/")[0];
    if (aliases.has(alias)) {
      return [];
    }
    return [
      rangeFinding(
        lineStarts,
        offset,
        offset + typeName.length,
        undefinedAliasCode,
        `Type reference '${typeName}' uses unknown import alias '${alias}'.`,
        "error",
      ),
    ];
  }
  if (/^[a-z]/.test(typeName) || declaredTypes.has(typeName)) {
    return [];
  }
  return [
    rangeFinding(
      lineStarts,
      offset,
      offset + typeName.length,
      undefinedTypeCode,
      `Type reference '${typeName}' is not declared locally or imported.`,
      "error",
    ),
  ];
}

function parseEntityBlocks(text: string): Array<{
  name: string;
  pipeFields: Array<{
    fieldName: string;
    names: string[];
    rawNames: string;
    allNamesCapitalized: boolean;
    hasCapitalizedName: boolean;
    startOffset: number;
  }>;
}> {
  const entities: Array<{
    name: string;
    pipeFields: Array<{
      fieldName: string;
      names: string[];
      rawNames: string;
      allNamesCapitalized: boolean;
      hasCapitalizedName: boolean;
      startOffset: number;
    }>;
  }> = [];
  const pattern =
    /^\s*(?:external\s+)?entity\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{/gm;
  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    const open = text.indexOf("{", match.index);
    if (open < 0) {
      continue;
    }
    const close = findMatchingBrace(text, open);
    if (close < 0) {
      continue;
    }
    const body = text.slice(open + 1, close);
    const pipeFields: Array<{
      fieldName: string;
      names: string[];
      rawNames: string;
      allNamesCapitalized: boolean;
      hasCapitalizedName: boolean;
      startOffset: number;
    }> = [];
    const fieldPattern =
      /^\s*([A-Za-z_][A-Za-z0-9_]*)\s*:\s*([A-Za-z_][A-Za-z0-9_]*(?:\s*\|\s*[A-Za-z_][A-Za-z0-9_]*)+)\s*$/gm;
    for (
      let field = fieldPattern.exec(body);
      field;
      field = fieldPattern.exec(body)
    ) {
      const rawNames = field[2];
      const names = rawNames.split("|").map((v) => v.trim());
      const hasCapitalizedName = names.some((n) => /^[A-Z]/.test(n));
      const allNamesCapitalized = names.every((n) => /^[A-Z]/.test(n));
      pipeFields.push({
        fieldName: field[1],
        names,
        rawNames,
        hasCapitalizedName,
        allNamesCapitalized,
        startOffset: open + 1 + field.index + field[0].indexOf(rawNames),
      });
    }
    entities.push({ name: match[1], pipeFields });
  }
  return entities;
}

function collectDeclaredEntityFields(
  text: string,
): Array<{ entity: string; name: string; offset: number }> {
  const out: Array<{ entity: string; name: string; offset: number }> = [];
  const entityPattern =
    /^\s*(?:external\s+)?entity\s+([A-Za-z_][A-Za-z0-9_]*)\s*\{/gm;
  for (
    let entity = entityPattern.exec(text);
    entity;
    entity = entityPattern.exec(text)
  ) {
    const entityName = entity[1];
    const open = text.indexOf("{", entity.index);
    if (open < 0) {
      continue;
    }
    const close = findMatchingBrace(text, open);
    if (close < 0) {
      continue;
    }
    const body = text.slice(open + 1, close);
    // Collect transitions block ranges to exclude from field scanning.
    // `terminal:` inside transitions blocks is a keyword, not a field.
    const transitionsRanges: Array<[number, number]> = [];
    const transPattern = /\btransitions\s+\w+\s*\{/g;
    for (
      let tm = transPattern.exec(body);
      tm;
      tm = transPattern.exec(body)
    ) {
      const tOpen = body.indexOf("{", tm.index);
      if (tOpen < 0) continue;
      const tClose = findMatchingBrace(text, open + 1 + tOpen);
      if (tClose < 0) continue;
      transitionsRanges.push([tOpen, tClose - (open + 1)]);
    }
    const fieldPattern = /^\s*([A-Za-z_][A-Za-z0-9_]*)\s*:\s*(.+)$/gm;
    for (
      let field = fieldPattern.exec(body);
      field;
      field = fieldPattern.exec(body)
    ) {
      const name = field[1];
      const rhs = field[2].trim();
      if (rhs.length === 0) {
        continue;
      }
      if (
        transitionsRanges.some(
          ([s, e]) => field!.index >= s && field!.index <= e,
        )
      ) {
        continue;
      }
      out.push({
        entity: entityName,
        name,
        offset: open + 1 + field.index + field[0].indexOf(name),
      });
    }
  }
  return out;
}

function applySuppressions(
  findings: Finding[],
  text: string,
  lineStarts: number[],
): Finding[] {
  const directives = collectSuppressions(text, lineStarts);
  return findings.filter((finding) => {
    const line = finding.start.line;
    const lineSuppressed = directives.get(line);
    const prevLineSuppressed = directives.get(line - 1);
    const active = lineSuppressed ?? prevLineSuppressed;
    if (!active) {
      return true;
    }
    return !(active.has("all") || active.has(finding.code));
  });
}

function collectSuppressions(
  text: string,
  lineStarts: number[],
): Map<number, Set<string>> {
  const suppressionByLine = new Map<number, Set<string>>();
  const pattern = /^[^\S\n]*--\s*allium-ignore\s+([A-Za-z0-9._,\- \t]+)$/gm;
  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    const line = offsetToPosition(lineStarts, match.index).line;
    const codes = match[1]
      .split(",")
      .map((value) => value.trim())
      .filter(Boolean);
    suppressionByLine.set(line, new Set(codes));
  }
  return suppressionByLine;
}

function isCommentLineAtIndex(text: string, index: number): boolean {
  const lineStart = text.lastIndexOf("\n", index) + 1;
  const lineEnd = text.indexOf("\n", index);
  const line = text.slice(lineStart, lineEnd >= 0 ? lineEnd : text.length);
  return /^\s*--/.test(line);
}

function escapeRegex(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
