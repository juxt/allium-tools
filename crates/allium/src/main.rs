mod domain_model;
mod test_plan;

use allium_parser::diagnostic::Severity;
use allium_parser::lexer::SourceMap;
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
produced during parsing.
";

const PLAN_HELP: &str = "\
allium plan - derive test obligations from a spec

Usage: allium plan <file.allium>

Prints a JSON document describing the test plan implied by the spec,
including invariants, rule pre- and post-conditions, and transitions.
";

const MODEL_HELP: &str = "\
allium model - extract the domain model as structured data

Usage: allium model <file.allium>

Prints a JSON document describing entities, value types and generators
derived from the spec.
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
        "check" | "analyse" | "parse" | "plan" | "model" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                print!("{}", subcommand_help(subcommand));
                return ExitCode::SUCCESS;
            }
            match subcommand {
                "check" => cmd_check(rest),
                "analyse" => cmd_analyse(rest),
                "parse" => cmd_parse(rest),
                "plan" => cmd_plan(rest),
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
        Some("check") | Some("analyse") | Some("parse") | Some("plan") | Some("model") => {
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

/// Shared loop for commands that process multiple .allium files.
fn run_multi_file(
    command: &str,
    args: &[String],
    analyse_file: impl Fn(&Path, &str, &allium_parser::ParseResult, &SourceMap) -> FileResult,
) -> ExitCode {
    let files = resolve_files(args);
    if files.is_empty() {
        eprintln!("No .allium files found.");
        return ExitCode::from(2);
    }

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
        let source_map = SourceMap::new(&source);
        let file_result = analyse_file(path, &source, &result, &source_map);

        if file_result.has_issues {
            any_issues = true;
        }

        let output = serde_json::json!({
            "command": command,
            "spec_file": path.display().to_string(),
            "diagnostics": file_result.diagnostics,
            "findings": file_result.findings,
        });

        println!("{}", serde_json::to_string_pretty(&output).unwrap());
    }

    if any_issues { ExitCode::from(1) } else { ExitCode::SUCCESS }
}

fn cmd_check(args: &[String]) -> ExitCode {
    run_multi_file("check", args, |path, source, result, source_map| {
        let analysis = allium_parser::analyze(&result.module, source);
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
    run_multi_file("analyse", args, |path, source, result, source_map| {
        let analyse_result = allium_parser::analyse(&result.module, source);
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

/// Shared handler for commands that take a single .allium file, parse it, and
/// print a JSON-serialisable result.
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
    let output = transform(&result.module, &source);
    println!("{}", serde_json::to_string_pretty(&output).unwrap());
    ExitCode::SUCCESS
}

fn cmd_parse(args: &[String]) -> ExitCode {
    // Parse is slightly different: it serialises the full ParseResult, not a
    // transform of the module. Keep it inline rather than forcing it through
    // run_single_file.
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
    ExitCode::SUCCESS
}

fn cmd_plan(args: &[String]) -> ExitCode {
    run_single_file("allium plan <file.allium>", args, |module, source| {
        let plan = test_plan::generate_test_plan(module, source);
        serde_json::to_value(plan).unwrap()
    })
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
