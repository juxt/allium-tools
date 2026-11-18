mod domain_model;
mod test_plan;

use allium_parser::diagnostic::Severity;
use allium_parser::lexer::SourceMap;
use allium_parser::{AmbiguousImports, ReverseContributions};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const HELP: &str = "\
allium - validate, parse and analyse Allium specification files

Usage: allium <command> [arguments]
       allium [-h | --help] [-V | --version]

Commands:
  check    Validate spec files and report structural diagnostics (JSON)
  analyse  Analyse process completeness: data flow, reachability, conflicts (JSON)
  parse    Parse a spec file and print the AST as JSON
  plan     Derive test obligations from a spec
  diff     Compare the test obligations of two specs or plans
  model    Extract the domain model as structured data
  help     Print help for the CLI or for a specific command

Options:
  -h, --help     Show this help message and exit
  -V, --version  Print version information and exit

Run `allium help <command>` or `allium <command> --help` for per-command help.
";

const CHECK_HELP: &str = "\
allium check - validate spec files and report structural diagnostics

Usage: allium check <path>...

Each <path> is a .allium file or a directory. Directories are searched
recursively for .allium files. Outputs JSON with a diagnostics array
containing line-level structural warnings and errors.

Exit codes:
  0  No errors or warnings
  1  One or more errors or warnings were reported
  2  No inputs provided, or no .allium files could be resolved
";

const ANALYSE_HELP: &str = "\
allium analyse - analyse process completeness

Usage: allium analyse <path>...

Runs structural checks (same as `check`) plus process-level analysis:
data flow tracing, edge reachability, deadlock detection, conflict
detection, and invariant verification. Outputs JSON with both a
diagnostics array and a findings array.

Exit codes:
  0  No findings
  1  One or more findings were produced
  2  No inputs provided, or no .allium files could be resolved
";

const PARSE_HELP: &str = "\
allium parse - parse a spec file and print the AST as JSON

Usage: allium parse <file.allium>

Prints a JSON document describing the parsed module and any diagnostics
produced during parsing. Exits non-zero if the spec carries an
error-severity diagnostic (e.g. it does not parse).
";

const PLAN_HELP: &str = "\
allium plan - derive test obligations from a spec

Usage: allium plan [--format json|tsv] <file.allium>

Prints the test obligations implied by the spec, including invariants,
rule pre- and post-conditions, and transitions.

--format json (the default) prints one JSON document:
  {\"version\": n, \"obligations\": [...], \"diagnostics\": [...]}
Each obligation carries `id`, `category`, `description`,
`source_construct` and `source_span` {`start`, `end`} (byte offsets),
plus `expression`, `detail` and `dependencies` where they apply. The
`diagnostics` array mirrors `check`.

--format tsv prints one obligation per line, with five tab-separated
columns and no header:
  id  category  source_construct  description  expression
`expression` is empty where the obligation has none. A tab, newline,
carriage return or backslash inside a value is written as \\t, \\n, \\r
or \\\\. Diagnostics go to stderr as `file:line:col: severity: message
[code]`.

Exits non-zero if the spec carries an error-severity diagnostic, so an
empty obligation set from a spec that does not parse is not mistaken for
a valid spec that legitimately has none.
";

const DIFF_HELP: &str = "\
allium diff - compare the test obligations of two specs or plans

Usage: allium diff [--format tsv|json] <old> <new>

Each input is a spec, or the output of `allium plan` in either format.
The kind is read from the content: a JSON object is a plan, a file
ending in `.allium` or starting with `-- allium:` is a spec, and lines of
five tab-separated columns are a plan in TSV form. A spec is planned
before it is compared. `-` reads one of the inputs from stdin.

--format tsv (the default) prints one line per difference, sorted by id:
  A  id           added
  D  id           removed
  M  id           content changed
  R  old  new     the same content under a different id
--format json prints
  {\"added\": [id], \"removed\": [id], \"changed\": [id],
   \"moved\": [{\"from\": id, \"to\": id}]}

Content is everything in an obligation except its `id` and
`source_span`. When either input is a TSV plan, content is its four
columns after the id. Positional ids such as `rule-failure.<Rule>.<n>`
are renumbered when clauses are added or removed, so an obligation whose
content now sits under another id is reported as moved rather than as
changed.

Example, comparing a spec with its last committed version:
  allium diff <(git show HEAD:spec.allium) spec.allium

Exit codes:
  0  No differences
  1  Differences were found
  2  Usage error, an unreadable input, an input that is neither a spec nor
     a plan, or a spec with an error-severity diagnostic
";

const MODEL_HELP: &str = "\
allium model - extract the domain model as structured data

Usage: allium model <file.allium>

Prints a JSON document describing entities, value types and generators
derived from the spec. The output carries a `diagnostics` array mirroring
`check`, and exits non-zero if the spec carries an error-severity
diagnostic.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        eprintln!("allium: missing command");
        eprintln!("Run `allium --help` for usage.");
        return ExitCode::from(2);
    }

    match args[0].as_str() {
        "--help" | "-h" => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        "--version" | "-V" => {
            println!(
                "allium {} (language versions: 1, 2, 3)",
                env!("CARGO_PKG_VERSION")
            );
            return ExitCode::SUCCESS;
        }
        "help" => return cmd_help(&args[1..]),
        _ => {}
    }

    let subcommand = args[0].as_str();
    let rest = &args[1..];

    match subcommand {
        "check" | "analyse" | "parse" | "plan" | "diff" | "model" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                print!("{}", subcommand_help(subcommand));
                return ExitCode::SUCCESS;
            }
            match subcommand {
                "check" => cmd_check(rest),
                "analyse" => cmd_analyse(rest),
                "parse" => cmd_parse(rest),
                "plan" => cmd_plan(rest),
                "diff" => cmd_diff(rest),
                "model" => cmd_model(rest),
                _ => unreachable!(),
            }
        }
        other => {
            eprintln!("allium: unknown command `{other}`");
            eprintln!("Run `allium --help` for available commands.");
            ExitCode::from(2)
        }
    }
}

fn cmd_help(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        None => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Some("check") | Some("analyse") | Some("parse") | Some("plan") | Some("diff") | Some("model") => {
            print!("{}", subcommand_help(args[0].as_str()));
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("allium: unknown command `{other}`");
            eprintln!("Run `allium --help` for available commands.");
            ExitCode::from(2)
        }
    }
}

fn subcommand_help(name: &str) -> &'static str {
    match name {
        "check" => CHECK_HELP,
        "analyse" => ANALYSE_HELP,
        "parse" => PARSE_HELP,
        "plan" => PLAN_HELP,
        "diff" => DIFF_HELP,
        "model" => MODEL_HELP,
        _ => HELP,
    }
}

// ---------------------------------------------------------------------------
// Multi-file commands: check, analyse
// ---------------------------------------------------------------------------

/// Per-file analysis result fed back from the closure to the multi-file loop.
struct FileResult {
    diagnostics: Vec<serde_json::Value>,
    findings: Vec<serde_json::Value>,
    has_issues: bool,
}

/// A parsed file ready for analysis.
struct ParsedFile {
    path: PathBuf,
    source: String,
    result: allium_parser::ParseResult,
}

/// Cross-module context computed once and passed to each file's analysis.
struct CrossModuleContext {
    /// Per-file: declaration names referenced by other modules.
    external_refs: HashMap<PathBuf, HashSet<String>>,
    /// Per-file: use path strings that resolved to files in the check set.
    resolved_use_paths: HashMap<PathBuf, HashSet<String>>,
    /// Per-file: `use` alias → trigger names the aliased module provides or
    /// emits. Aliases whose targets are outside the check set are absent.
    imported_triggers: HashMap<PathBuf, HashMap<String, HashSet<String>>>,
    /// Per-file: `use` alias → (imported entity/value type → declared field
    /// names). Lets a qualified `default alias/Type` literal be validated
    /// against the imported schema. Aliases whose targets are outside the check
    /// set are absent.
    imported_entity_fields: HashMap<PathBuf, HashMap<String, HashMap<String, HashSet<String>>>>,
    /// Per-file: `use` alias → every trigger name the aliased module references.
    /// Lets a qualified `provides: alias/Trigger` entry be validated (#72).
    /// Aliases whose targets are outside the check set are absent.
    imported_referenced_triggers: HashMap<PathBuf, HashMap<String, HashSet<String>>>,
    /// Per-file: names and triggers that more than one imported module could
    /// resolve, so unqualified references to them are ambiguous (issue #15).
    ambiguous_imports: HashMap<PathBuf, AmbiguousImports>,
    /// Per-imported-file: contributions its importers make back to it (qualified
    /// creations, provides and witnessed transitions). Keyed by the imported
    /// (target) module, aggregated across all its importers in the check set.
    reverse_contributions: HashMap<PathBuf, ReverseContributions>,
    /// Per-file: imported entity name → its declared status values, flattened
    /// across every `use` alias whose target is in the check set. Lets the
    /// importer's conflict pass attribute a rule to an imported entity.
    imported_entity_statuses: HashMap<PathBuf, HashMap<String, HashSet<String>>>,
    /// Per-file: use path strings that resolve neither to a file in the check
    /// set nor to a file on disk — broken imports, as opposed to out-of-set
    /// ones. References through their aliases are diagnosed per reference.
    missing_use_paths: HashMap<PathBuf, HashSet<String>>,
}

/// Shared loop for commands that process multiple .allium files.
///
/// Parses all files first, builds the cross-module reference map, then
/// analyses each file with knowledge of which declarations are referenced
/// externally and which use paths resolved.
fn run_multi_file(
    command: &str,
    args: &[String],
    analyse_file: impl Fn(&Path, &str, &allium_parser::ParseResult, &SourceMap, &HashSet<String>, &HashSet<String>, &HashMap<String, HashSet<String>>, &HashMap<String, HashMap<String, HashSet<String>>>, &AmbiguousImports, &ReverseContributions, &HashMap<String, HashSet<String>>, &HashMap<String, HashSet<String>>, &HashSet<String>) -> FileResult,
) -> ExitCode {
    let files = resolve_files(args);
    if files.is_empty() {
        eprintln!("No .allium files found.");
        return ExitCode::from(2);
    }

    // Pass 1: parse all files.
    let mut parsed: Vec<ParsedFile> = Vec::new();
    let mut any_issues = false;

    for path in &files {
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                any_issues = true;
                continue;
            }
        };
        let result = allium_parser::parse(&source);
        parsed.push(ParsedFile {
            path: path.clone(),
            source,
            result,
        });
    }

    // Pass 2: build cross-module context.
    let ctx = build_cross_module_context(&parsed);

    // Pass 3: analyse each file with cross-module context.
    let no_ambiguity = AmbiguousImports::default();
    let no_reverse = ReverseContributions::default();
    for pf in &parsed {
        let source_map = SourceMap::new(&pf.source);
        let key = canonical_key(&pf.path);
        let refs = ctx.external_refs.get(&key).cloned().unwrap_or_default();
        let use_paths = ctx.resolved_use_paths.get(&key).cloned().unwrap_or_default();
        let imports = ctx.imported_triggers.get(&key).cloned().unwrap_or_default();
        let imported_fields = ctx.imported_entity_fields.get(&key).cloned().unwrap_or_default();
        let ambiguous = ctx.ambiguous_imports.get(&key).unwrap_or(&no_ambiguity);
        let reverse = ctx.reverse_contributions.get(&key).unwrap_or(&no_reverse);
        let referenced = ctx.imported_referenced_triggers.get(&key).cloned().unwrap_or_default();
        let imported_statuses = ctx.imported_entity_statuses.get(&key).cloned().unwrap_or_default();
        let missing = ctx.missing_use_paths.get(&key).cloned().unwrap_or_default();
        let file_result = analyse_file(&pf.path, &pf.source, &pf.result, &source_map, &refs, &use_paths, &imports, &imported_fields, ambiguous, reverse, &referenced, &imported_statuses, &missing);

        if file_result.has_issues {
            any_issues = true;
        }

        let output = serde_json::json!({
            "command": command,
            "spec_file": pf.path.display().to_string(),
            "diagnostics": file_result.diagnostics,
            "findings": file_result.findings,
        });

        println!("{}", serde_json::to_string_pretty(&output).unwrap());
    }

    if any_issues { ExitCode::from(1) } else { ExitCode::SUCCESS }
}

/// Build both cross-module maps by examining all parsed files.
fn build_cross_module_context(parsed: &[ParsedFile]) -> CrossModuleContext {
    // Set of canonical paths in the check set, for resolving use targets.
    let check_set: HashSet<PathBuf> = parsed
        .iter()
        .map(|pf| canonical_key(&pf.path))
        .collect();

    // Pre-compute each file's declared names so we can match unqualified
    // references against imported module declarations.
    let declared: HashMap<PathBuf, HashSet<String>> = parsed
        .iter()
        .map(|pf| {
            (
                canonical_key(&pf.path),
                allium_parser::collect_declared_names(&pf.result.module),
            )
        })
        .collect();

    // Pre-compute each file's provided/emitted trigger names so importing
    // files can resolve `alias/Trigger` subscriptions across the check set.
    let trigger_outputs: HashMap<PathBuf, HashSet<String>> = parsed
        .iter()
        .map(|pf| {
            (
                canonical_key(&pf.path),
                allium_parser::collect_trigger_outputs(&pf.result.module),
            )
        })
        .collect();

    // Pre-compute each file's entity/value type → field-name schema so importing
    // files can validate `default alias/Type = { ... }` literals against the
    // imported type's declared fields.
    let entity_field_outputs: HashMap<PathBuf, HashMap<String, HashSet<String>>> = parsed
        .iter()
        .map(|pf| {
            (
                canonical_key(&pf.path),
                allium_parser::collect_entity_field_schemas(&pf.result.module),
            )
        })
        .collect();

    // Each file's entity → status-values schema, so an importer's conflict pass
    // can attribute a rule to an imported entity by the statuses it references.
    let entity_status_outputs: HashMap<PathBuf, HashMap<String, HashSet<String>>> = parsed
        .iter()
        .map(|pf| {
            (
                canonical_key(&pf.path),
                allium_parser::collect_entity_status_schemas(&pf.result.module),
            )
        })
        .collect();

    // Pre-compute every trigger name each file references (provides, emits or
    // listens for), so an importing file can validate a qualified `provides:`
    // entry against the aliased module (#72).
    let referenced_triggers: HashMap<PathBuf, HashSet<String>> = parsed
        .iter()
        .map(|pf| {
            (
                canonical_key(&pf.path),
                allium_parser::collect_referenced_trigger_names(&pf.result.module),
            )
        })
        .collect();

    // Imported module AST by canonical key, so an importer's contributions can
    // be resolved against the module it imports (types, status values).
    let module_by_key: HashMap<PathBuf, &allium_parser::Module> = parsed
        .iter()
        .map(|pf| (canonical_key(&pf.path), &pf.result.module))
        .collect();

    let mut external_refs: HashMap<PathBuf, HashSet<String>> = HashMap::new();
    let mut resolved_use_paths: HashMap<PathBuf, HashSet<String>> = HashMap::new();
    let mut imported_triggers: HashMap<PathBuf, HashMap<String, HashSet<String>>> = HashMap::new();
    let mut imported_referenced_triggers: HashMap<PathBuf, HashMap<String, HashSet<String>>> =
        HashMap::new();
    let mut reverse_contributions: HashMap<PathBuf, ReverseContributions> = HashMap::new();
    let mut imported_entity_fields: HashMap<
        PathBuf,
        HashMap<String, HashMap<String, HashSet<String>>>,
    > = HashMap::new();
    let mut imported_entity_statuses: HashMap<PathBuf, HashMap<String, HashSet<String>>> =
        HashMap::new();
    let mut ambiguous_imports: HashMap<PathBuf, AmbiguousImports> = HashMap::new();
    let mut missing_use_paths: HashMap<PathBuf, HashSet<String>> = HashMap::new();

    for pf in parsed {
        // For bare filenames (no directory component), parent() returns "".
        // Joining "" with a relative use path still works because canonicalize
        // resolves it against the process working directory.
        let base = pf.path.parent().unwrap_or(Path::new("."));
        let file_key = canonical_key(&pf.path);

        // Collect use declarations: alias → use-path string, and resolve each
        // against the check set. Also build alias → canonical target key, and
        // note paths that resolve neither in the check set nor on disk — a
        // broken import, distinct from an on-disk file merely outside a
        // narrower check set (a single-file check of one member of a pair).
        let mut aliases: HashMap<&str, String> = HashMap::new();
        let mut alias_targets: HashMap<&str, PathBuf> = HashMap::new();
        let mut resolved_for_file: HashSet<String> = HashSet::new();
        let mut missing_for_file: HashSet<String> = HashSet::new();

        for d in &pf.result.module.declarations {
            if let allium_parser::ast::Decl::Use(u) = d {
                let path_text = u.path.text();
                let target = base.join(&path_text);
                let target_key = canonical_key(&target);

                if check_set.contains(&target_key) {
                    resolved_for_file.insert(path_text.clone());
                } else if path_text.ends_with(".allium") && !target.exists() {
                    // Only local file references (ending .allium) can be
                    // broken. Registry coordinates (`github.com/org/spec/sha`)
                    // are immutable remote references that are not expected on
                    // disk; they stay out-of-set, never broken.
                    //
                    // This `exists()` makes `check` output depend on the
                    // filesystem, not just its input files: a wrong-case local
                    // path reads as present on a case-insensitive filesystem
                    // (macOS, Windows) and broken on a case-sensitive one
                    // (Linux CI), so the same spec can diagnose differently by
                    // platform. The divergence runs in the safe direction: CI
                    // is the stricter of the two, so nothing dead slips the
                    // gate that a developer saw as clean.
                    missing_for_file.insert(path_text.clone());
                }

                if let Some(alias) = &u.alias {
                    alias_targets.insert(alias.name.as_str(), target_key);
                    aliases.insert(alias.name.as_str(), path_text);
                }
            }
        }
        missing_use_paths.insert(file_key.clone(), missing_for_file);

        // 0. Reverse contributions — for each alias resolving to a module in the
        //    check set, the contributions this importer makes back to it
        //    (qualified creations, provides, witnessed transitions), aggregated
        //    against the imported (target) module.
        for (alias, target_key) in &alias_targets {
            if let Some(target_module) = module_by_key.get(target_key) {
                let contrib = allium_parser::collect_reverse_contributions(
                    &pf.result.module,
                    alias,
                    target_module,
                );
                if !contrib.is_empty() {
                    reverse_contributions
                        .entry(target_key.clone())
                        .or_default()
                        .merge(contrib);
                }
            }
        }

        // 1. Qualified references (core/InputEvent) and alias-member
        //    references (core.EntityMap) — both produce (qualifier, name)
        //    pairs from collect_qualified_references.
        let qrefs = allium_parser::collect_qualified_references(&pf.result.module);
        for (qualifier, name) in &qrefs {
            if let Some(target_key) = alias_targets.get(qualifier.as_str()) {
                external_refs.entry(target_key.clone()).or_default().insert(name.clone());
            }
        }

        // 2. Unqualified references — any uppercase ident used in this file
        //    that matches a declaration in an imported module. This covers
        //    bare entity names in rule bodies and type names in contract
        //    signatures. Subtract locally-declared names first so that a
        //    local declaration doesn't spuriously suppress a same-named
        //    declaration in an imported module.
        let all_idents = allium_parser::collect_all_referenced_idents(&pf.result.module);
        let local_names = allium_parser::collect_declared_names(&pf.result.module);
        let imported_idents: HashSet<&String> = all_idents.difference(&local_names).collect();
        for target_key in alias_targets.values() {
            if let Some(target_decls) = declared.get(target_key) {
                for name in &imported_idents {
                    if target_decls.contains(name.as_str()) {
                        external_refs.entry(target_key.clone()).or_default().insert((*name).clone());
                    }
                }
            }
        }

        // 3. Imported triggers — for each alias whose target is in the check
        //    set, the trigger names the target module provides or emits.
        let mut imported_for_file: HashMap<String, HashSet<String>> = HashMap::new();
        for (alias, target_key) in &alias_targets {
            if let Some(triggers) = trigger_outputs.get(target_key) {
                imported_for_file.insert((*alias).to_string(), triggers.clone());
            }
        }
        imported_triggers.insert(file_key.clone(), imported_for_file);

        // 3a. Referenced triggers — for each alias whose target is in the check
        //     set, every trigger name that module references. Lets a qualified
        //     `provides: alias/Trigger` entry be validated against it (#72).
        let mut referenced_for_file: HashMap<String, HashSet<String>> = HashMap::new();
        for (alias, target_key) in &alias_targets {
            if let Some(triggers) = referenced_triggers.get(target_key) {
                referenced_for_file.insert((*alias).to_string(), triggers.clone());
            }
        }
        imported_referenced_triggers.insert(file_key.clone(), referenced_for_file);

        // 3b. Imported entity field schemas — for each alias whose target is in
        //     the check set, that module's entity/value type → field names.
        let mut imported_fields_for_file: HashMap<String, HashMap<String, HashSet<String>>> =
            HashMap::new();
        for (alias, target_key) in &alias_targets {
            if let Some(fields) = entity_field_outputs.get(target_key) {
                imported_fields_for_file.insert((*alias).to_string(), fields.clone());
            }
        }
        imported_entity_fields.insert(file_key.clone(), imported_fields_for_file);

        // 3c. Imported entity status vocabularies, flattened across aliases to
        //     entity name → status values, so the importer's conflict pass can
        //     attribute a rule to an imported entity by the statuses it uses.
        let mut imported_statuses_for_file: HashMap<String, HashSet<String>> = HashMap::new();
        for target_key in alias_targets.values() {
            if let Some(statuses) = entity_status_outputs.get(target_key) {
                for (entity, values) in statuses {
                    imported_statuses_for_file
                        .entry(entity.clone())
                        .or_default()
                        .extend(values.iter().cloned());
                }
            }
        }
        imported_entity_statuses.insert(file_key.clone(), imported_statuses_for_file);

        // 4. Ambiguous imports — names declared, and triggers provided or
        //    emitted, by more than one distinct imported file. Keyed by
        //    distinct target so that two aliases for the same file are not
        //    counted as ambiguous (issue #15).
        let mut name_sources: HashMap<&String, BTreeMap<&PathBuf, &str>> = HashMap::new();
        let mut trigger_sources: HashMap<&String, BTreeMap<&PathBuf, &str>> = HashMap::new();
        for (alias, target_key) in &alias_targets {
            if let Some(decls) = declared.get(target_key) {
                for name in decls {
                    let by_target = name_sources.entry(name).or_default();
                    let entry = by_target.entry(target_key).or_insert(alias);
                    if *alias < *entry {
                        *entry = alias;
                    }
                }
            }
            if let Some(triggers) = trigger_outputs.get(target_key) {
                for name in triggers {
                    let by_target = trigger_sources.entry(name).or_default();
                    let entry = by_target.entry(target_key).or_insert(alias);
                    if *alias < *entry {
                        *entry = alias;
                    }
                }
            }
        }
        let collapse = |sources: HashMap<&String, BTreeMap<&PathBuf, &str>>| -> HashMap<String, Vec<String>> {
            sources
                .into_iter()
                .filter(|(_, by_target)| by_target.len() >= 2)
                .map(|(name, by_target)| {
                    let mut aliases: Vec<String> =
                        by_target.values().map(|a| a.to_string()).collect();
                    aliases.sort();
                    (name.clone(), aliases)
                })
                .collect()
        };
        ambiguous_imports.insert(
            file_key.clone(),
            AmbiguousImports {
                names: collapse(name_sources),
                triggers: collapse(trigger_sources),
            },
        );

        resolved_use_paths.insert(file_key, resolved_for_file);
    }

    CrossModuleContext {
        external_refs,
        resolved_use_paths,
        imported_triggers,
        imported_entity_fields,
        imported_referenced_triggers,
        ambiguous_imports,
        reverse_contributions,
        imported_entity_statuses,
        missing_use_paths,
    }
}

/// Normalise a path for use as a map key. Uses canonicalize when possible,
/// falls back to the raw path. This assumes that all files in a single check
/// invocation will either all canonicalise successfully or all fail — if some
/// canonicalise and others don't (e.g. broken symlinks), the same underlying
/// file could produce different keys and cross-module lookups would miss.
fn canonical_key(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn cmd_check(args: &[String]) -> ExitCode {
    run_multi_file("check", args, |path, source, result, source_map, external_refs, resolved_use_paths, imported_triggers, imported_entity_fields, ambiguous_imports, reverse, referenced_triggers, _imported_entity_statuses, missing_use_paths| {
        // `check` emits diagnostics only, not findings, so it needs no imported
        // status vocabulary (conflicts are findings, surfaced by `analyse`).
        let analysis = allium_parser::analyze_with_cross_module(&result.module, source, external_refs, resolved_use_paths, imported_triggers, imported_entity_fields, ambiguous_imports, reverse, referenced_triggers, missing_use_paths);
        let diagnostics: Vec<serde_json::Value> = result
            .diagnostics
            .iter()
            .chain(analysis.iter())
            .map(|d| diagnostic_to_json(d, path, source_map))
            .collect();
        let has_issues = diagnostics.iter().any(|d| {
            d["severity"] == "error" || d["severity"] == "warning"
        });
        FileResult { diagnostics, findings: vec![], has_issues }
    })
}

fn cmd_analyse(args: &[String]) -> ExitCode {
    run_multi_file("analyse", args, |path, source, result, source_map, external_refs, resolved_use_paths, imported_triggers, imported_entity_fields, ambiguous_imports, reverse, referenced_triggers, imported_entity_statuses, missing_use_paths| {
        let analyse_result = allium_parser::analyse_with_cross_module(&result.module, source, external_refs, resolved_use_paths, imported_triggers, imported_entity_fields, ambiguous_imports, reverse, referenced_triggers, imported_entity_statuses, missing_use_paths);
        let diagnostics: Vec<serde_json::Value> = result
            .diagnostics
            .iter()
            .chain(analyse_result.diagnostics.iter())
            .map(|d| diagnostic_to_json(d, path, source_map))
            .collect();
        let has_issues = !analyse_result.findings.is_empty();
        FileResult { diagnostics, findings: analyse_result.findings, has_issues }
    })
}

// ---------------------------------------------------------------------------
// Single-file commands: parse, plan, model
// ---------------------------------------------------------------------------

/// Non-zero exit when the parse carries an error-severity diagnostic.
///
/// The parser is error-tolerant: a spec that does not parse still yields a
/// (partial, possibly empty) `Module` plus diagnostics rather than failing
/// outright. Every single-file command routes its exit code through here so
/// that "the spec did not parse" cannot be laundered into a plausible-looking
/// empty success. See `docs/versioning.md` and issue #80.
fn single_file_exit(diagnostics: &[allium_parser::Diagnostic]) -> ExitCode {
    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// Shared handler for commands that take a single .allium file, parse it, and
/// print a JSON-serialisable result.
///
/// The transformed output is an object; parse diagnostics are merged into it
/// under a `diagnostics` key (mirroring `check`) so a consumer can tell an
/// empty-but-valid result apart from a parse failure without a second
/// invocation. The exit code reflects error-severity diagnostics.
fn run_single_file(
    usage: &str,
    args: &[String],
    transform: impl FnOnce(&allium_parser::Module, &str) -> serde_json::Value,
) -> ExitCode {
    if args.len() != 1 {
        eprintln!("Usage: {usage}");
        return ExitCode::from(2);
    }

    let path = Path::new(&args[0]);
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            return ExitCode::from(1);
        }
    };

    let result = allium_parser::parse(&source);
    let source_map = SourceMap::new(&source);
    let diagnostics: Vec<serde_json::Value> = result
        .diagnostics
        .iter()
        .map(|d| diagnostic_to_json(d, path, &source_map))
        .collect();

    let mut output = transform(&result.module, &source);
    if let serde_json::Value::Object(map) = &mut output {
        map.insert("diagnostics".to_string(), serde_json::Value::Array(diagnostics));
    }
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    single_file_exit(&result.diagnostics)
}

fn cmd_parse(args: &[String]) -> ExitCode {
    // Parse is slightly different: it serialises the full ParseResult, which
    // already carries diagnostics. It still shares the single-file exit
    // contract so a spec that does not parse exits non-zero.
    if args.len() != 1 {
        eprintln!("Usage: allium parse <file.allium>");
        return ExitCode::from(2);
    }

    let path = Path::new(&args[0]);
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            return ExitCode::from(1);
        }
    };

    let result = allium_parser::parse(&source);
    println!("{}", serde_json::to_string_pretty(&result).unwrap());
    single_file_exit(&result.diagnostics)
}

fn cmd_plan(args: &[String]) -> ExitCode {
    let usage = "allium plan [--format json|tsv] <file.allium>";
    let Some((format, files)) = parse_format(args, OutputFormat::Json) else {
        eprintln!("Usage: {usage}");
        return ExitCode::from(2);
    };
    if format == OutputFormat::Json {
        return run_single_file(usage, &files, |module, source| {
            let plan = test_plan::generate_test_plan(module, source);
            serde_json::to_value(plan).unwrap()
        });
    }

    if files.len() != 1 {
        eprintln!("Usage: {usage}");
        return ExitCode::from(2);
    }
    let path = Path::new(&files[0]);
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            return ExitCode::from(1);
        }
    };
    let result = allium_parser::parse(&source);
    let plan = test_plan::generate_test_plan(&result.module, &source);
    let mut out = String::new();
    for o in &plan.obligations {
        let o = serde_json::to_value(o).unwrap();
        let row = obligation_row(&o);
        out.push_str(&tsv_escape(&json_str(&o, "id")));
        for value in &row {
            out.push('\t');
            out.push_str(&tsv_escape(value));
        }
        out.push('\n');
    }
    print!("{out}");
    let source_map = SourceMap::new(&source);
    for d in &result.diagnostics {
        eprintln!("{}", diagnostic_line(d, path, &source_map));
    }
    single_file_exit(&result.diagnostics)
}

#[derive(Clone, Copy, PartialEq)]
enum OutputFormat {
    Json,
    Tsv,
}

/// Splits `--format <json|tsv>` (or `--format=<value>`) from the positional
/// arguments. None on an unknown format, a missing value or an unknown option.
fn parse_format(args: &[String], default: OutputFormat) -> Option<(OutputFormat, Vec<String>)> {
    let mut format = default;
    let mut positional = Vec::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let value = if arg == "--format" {
            rest.next()?.as_str()
        } else if let Some(value) = arg.strip_prefix("--format=") {
            value
        } else if arg.starts_with('-') && arg != "-" {
            return None;
        } else {
            positional.push(arg.clone());
            continue;
        };
        format = match value {
            "json" => OutputFormat::Json,
            "tsv" => OutputFormat::Tsv,
            _ => return None,
        };
    }
    Some((format, positional))
}

fn json_str(o: &serde_json::Value, key: &str) -> String {
    o.get(key).and_then(|v| v.as_str()).unwrap_or_default().to_string()
}

/// The TSV columns of an obligation after its id.
fn obligation_row(o: &serde_json::Value) -> [String; 4] {
    ["category", "source_construct", "description", "expression"].map(|key| json_str(o, key))
}

fn tsv_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

fn tsv_unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

fn diagnostic_line(d: &allium_parser::Diagnostic, path: &Path, source_map: &SourceMap) -> String {
    let json = diagnostic_to_json(d, path, source_map);
    format!(
        "{}:{}:{}: {}: {} [{}]",
        path.display(),
        json["location"]["line"],
        json["location"]["col"],
        json_str(&json, "severity"),
        json_str(&json, "message"),
        json_str(&json, "code"),
    )
}

// ---------------------------------------------------------------------------
// diff
// ---------------------------------------------------------------------------

/// One obligation as `diff` compares it. `full` is the obligation without its
/// id and source span; a TSV plan carries only `row`.
struct PlanEntry {
    id: String,
    full: Option<String>,
    row: [String; 4],
}

fn cmd_diff(args: &[String]) -> ExitCode {
    let usage = "allium diff [--format tsv|json] <old> <new>";
    let Some((format, inputs)) = parse_format(args, OutputFormat::Tsv) else {
        eprintln!("Usage: {usage}");
        return ExitCode::from(2);
    };
    if inputs.len() != 2 || inputs.iter().all(|i| i == "-") {
        eprintln!("Usage: {usage}");
        return ExitCode::from(2);
    }
    let (old, new) = match (load_plan_entries(&inputs[0]), load_plan_entries(&inputs[1])) {
        (Ok(old), Ok(new)) => (old, new),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };

    let use_full = old.iter().chain(&new).all(|e| e.full.is_some());
    let content = |e: &PlanEntry| match (&e.full, use_full) {
        (Some(full), true) => full.clone(),
        _ => e.row.join("\t"),
    };
    let changes = diff_plans(&old, &new, content);

    match format {
        OutputFormat::Tsv => {
            for change in &changes {
                match change {
                    PlanChange::Added(id) => println!("A\t{id}"),
                    PlanChange::Removed(id) => println!("D\t{id}"),
                    PlanChange::Changed(id) => println!("M\t{id}"),
                    PlanChange::Moved(from, to) => println!("R\t{from}\t{to}"),
                }
            }
        }
        OutputFormat::Json => {
            let pick = |f: fn(&PlanChange) -> Option<serde_json::Value>| {
                changes.iter().filter_map(f).collect::<Vec<_>>()
            };
            let output = serde_json::json!({
                "added": pick(|c| match c { PlanChange::Added(id) => Some(id.as_str().into()), _ => None }),
                "removed": pick(|c| match c { PlanChange::Removed(id) => Some(id.as_str().into()), _ => None }),
                "changed": pick(|c| match c { PlanChange::Changed(id) => Some(id.as_str().into()), _ => None }),
                "moved": pick(|c| match c {
                    PlanChange::Moved(from, to) => Some(serde_json::json!({"from": from, "to": to})),
                    _ => None,
                }),
            });
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        }
    }
    if changes.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

#[derive(Debug, PartialEq)]
enum PlanChange {
    Added(String),
    Removed(String),
    Changed(String),
    Moved(String, String),
}

impl PlanChange {
    fn sort_id(&self) -> &str {
        match self {
            PlanChange::Added(id) | PlanChange::Removed(id) | PlanChange::Changed(id) => id,
            PlanChange::Moved(from, _) => from,
        }
    }
}

/// Pairs obligations in four passes: same id and content (unchanged), same
/// content under another id (moved, paired in plan order), same id with
/// other content (changed), and the rest (added or removed).
fn diff_plans(old: &[PlanEntry], new: &[PlanEntry], content: impl Fn(&PlanEntry) -> String) -> Vec<PlanChange> {
    let new_content: HashMap<&str, String> = new.iter().map(|e| (e.id.as_str(), content(e))).collect();
    let old_content: HashMap<&str, String> = old.iter().map(|e| (e.id.as_str(), content(e))).collect();
    let unchanged = |id: &str, c: &String| new_content.get(id) == Some(c);
    let mut old_left: Vec<(&str, String)> = old
        .iter()
        .map(|e| (e.id.as_str(), content(e)))
        .filter(|(id, c)| !unchanged(id, c))
        .collect();
    let mut new_left: Vec<(&str, String)> = new
        .iter()
        .map(|e| (e.id.as_str(), content(e)))
        .filter(|(id, c)| old_content.get(id) != Some(c))
        .collect();

    let mut changes = Vec::new();
    old_left.retain(|(old_id, c)| match new_left.iter().position(|(_, n)| n == c) {
        Some(i) => {
            changes.push(PlanChange::Moved(old_id.to_string(), new_left.remove(i).0.to_string()));
            false
        }
        None => true,
    });
    let new_ids: HashSet<&str> = new_left.iter().map(|(id, _)| *id).collect();
    let old_ids: HashSet<&str> = old_left.iter().map(|(id, _)| *id).collect();
    for (id, _) in &old_left {
        changes.push(if new_ids.contains(id) {
            PlanChange::Changed(id.to_string())
        } else {
            PlanChange::Removed(id.to_string())
        });
    }
    for (id, _) in &new_left {
        if !old_ids.contains(id) {
            changes.push(PlanChange::Added(id.to_string()));
        }
    }
    changes.sort_by(|a, b| a.sort_id().cmp(b.sort_id()));
    changes
}

/// Reads one `diff` input and works out what it is: a JSON plan, a spec, or
/// a TSV plan.
fn load_plan_entries(input: &str) -> Result<Vec<PlanEntry>, String> {
    let text = if input == "-" {
        std::io::read_to_string(std::io::stdin())
    } else {
        std::fs::read_to_string(input)
    }
    .map_err(|e| format!("{input}: {e}"))?;
    let trimmed = text.trim_start();

    let obligations: Vec<serde_json::Value> = if trimmed.starts_with('{') {
        let plan: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("{input}: invalid JSON: {e}"))?;
        plan.get("obligations")
            .and_then(|o| o.as_array())
            .cloned()
            .ok_or_else(|| format!("{input}: JSON without an `obligations` array is not a plan"))?
    } else if input.ends_with(".allium") || trimmed.starts_with("-- allium:") {
        let result = allium_parser::parse(&text);
        let path = Path::new(input);
        let source_map = SourceMap::new(&text);
        let errors: Vec<String> = result
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| diagnostic_line(d, path, &source_map))
            .collect();
        if !errors.is_empty() {
            return Err(errors.join("\n"));
        }
        let plan = test_plan::generate_test_plan(&result.module, &text);
        plan.obligations.iter().map(|o| serde_json::to_value(o).unwrap()).collect()
    } else {
        return text
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| {
                let cols: Vec<String> = line.split('\t').map(tsv_unescape).collect();
                match <[String; 5]>::try_from(cols) {
                    Ok([id, category, construct, description, expression]) => Ok(PlanEntry {
                        id,
                        full: None,
                        row: [category, construct, description, expression],
                    }),
                    Err(_) => Err(format!(
                        "{input}: not a spec, a JSON plan or a TSV plan (expected five tab-separated columns)"
                    )),
                }
            })
            .collect();
    };

    obligations
        .iter()
        .map(|o| {
            let id = o.get("id").and_then(|v| v.as_str()).ok_or_else(|| format!("{input}: an obligation has no `id`"))?;
            let mut full = o.as_object().cloned().unwrap_or_default();
            full.remove("id");
            full.remove("source_span");
            Ok(PlanEntry {
                id: id.to_string(),
                full: Some(serde_json::Value::Object(full).to_string()),
                row: obligation_row(o),
            })
        })
        .collect()
}

fn cmd_model(args: &[String]) -> ExitCode {
    run_single_file("allium model <file.allium>", args, |module, source| {
        let model = domain_model::extract_domain_model(module, source);
        serde_json::to_value(model).unwrap()
    })
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn diagnostic_to_json(
    d: &allium_parser::Diagnostic,
    path: &Path,
    source_map: &SourceMap,
) -> serde_json::Value {
    let (line, col) = source_map.line_col(d.span.start);
    let severity = match d.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
    };
    serde_json::json!({
        "code": d.code,
        "severity": severity,
        "message": d.message,
        "location": {
            "file": path.display().to_string(),
            "line": line + 1,
            "col": col + 1,
        }
    })
}

fn resolve_files(args: &[String]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for arg in args {
        let path = Path::new(arg);
        if path.is_dir() {
            collect_allium_files(path, &mut files);
        } else if path.extension().is_some_and(|e| e == "allium") {
            files.push(path.to_path_buf());
        } else {
            // Try as-is (might be a glob pattern the shell expanded)
            files.push(path.to_path_buf());
        }
    }
    files
}

fn collect_allium_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_allium_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "allium") {
            out.push(path);
        }
    }
}
