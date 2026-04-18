mod generators;
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
  check    Validate spec files and report diagnostics
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
allium check - validate spec files and report diagnostics

Usage: allium check <path>...

Each <path> is a .allium file or a directory. Directories are searched
recursively for .allium files. Findings are printed one per line as:
  <path>:<line>:<column>: <severity> <code> <message>

Exit codes:
  0  No errors or warnings
  1  One or more errors or warnings were reported
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
        "check" | "parse" | "plan" | "model" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                print!("{}", subcommand_help(subcommand));
                return ExitCode::SUCCESS;
            }
            match subcommand {
                "check" => cmd_check(rest),
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
        Some("check") | Some("parse") | Some("plan") | Some("model") => {
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
        "parse" => PARSE_HELP,
        "plan" => PLAN_HELP,
        "model" => MODEL_HELP,
        _ => HELP,
    }
}

fn cmd_parse(args: &[String]) -> ExitCode {
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
    match serde_json::to_string_pretty(&result) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Failed to serialise AST: {e}");
            ExitCode::from(1)
        }
    }
}

fn cmd_check(args: &[String]) -> ExitCode {
    let files = resolve_files(args);
    if files.is_empty() {
        eprintln!("No .allium files found.");
        return ExitCode::from(2);
    }

    let mut total_errors = 0u32;
    let mut total_warnings = 0u32;

    for path in &files {
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                total_errors += 1;
                continue;
            }
        };

        let result = allium_parser::parse(&source);
        let analysis_diagnostics = allium_parser::analyze(&result.module, &source);
        let source_map = SourceMap::new(&source);

        let all_diagnostics: Vec<&allium_parser::Diagnostic> = result
            .diagnostics
            .iter()
            .chain(analysis_diagnostics.iter())
            .collect();

        for d in all_diagnostics {
            let (line, col) = source_map.line_col(d.span.start);
            let severity = match d.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
                Severity::Info => "info",
            };
            if let Some(code) = d.code {
                println!(
                    "{}:{}:{}: {severity} {} {}",
                    path.display(),
                    line + 1,
                    col + 1,
                    code,
                    d.message,
                );
            } else {
                println!(
                    "{}:{}:{}: {severity}: {}",
                    path.display(),
                    line + 1,
                    col + 1,
                    d.message,
                );
            }
            print_source_snippet(&source_map, &source, line, col);

            match d.severity {
                Severity::Error => total_errors += 1,
                Severity::Warning => total_warnings += 1,
                Severity::Info => {}
            }
        }
    }

    let file_count = files.len();
    if total_errors == 0 && total_warnings == 0 {
        eprintln!("{file_count} file(s) checked, no issues found.");
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "{file_count} file(s) checked, {total_errors} error(s), {total_warnings} warning(s)."
        );
        ExitCode::from(1)
    }
}

fn cmd_plan(args: &[String]) -> ExitCode {
    if args.len() != 1 {
        eprintln!("Usage: allium plan <file.allium>");
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
    let plan = test_plan::generate_test_plan(&result.module, &source);
    match serde_json::to_string_pretty(&plan) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Failed to serialise test plan: {e}");
            ExitCode::from(1)
        }
    }
}

fn cmd_model(args: &[String]) -> ExitCode {
    if args.len() != 1 {
        eprintln!("Usage: allium model <file.allium>");
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
    let spec = generators::generate_generators(&result.module, &source);
    match serde_json::to_string_pretty(&spec) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Failed to serialise model: {e}");
            ExitCode::from(1)
        }
    }
}

fn print_source_snippet(source_map: &SourceMap, source: &str, line: u32, col: u32) {
    let line_text = source_map.line_text(source, line);
    let line_num = format!("{}", line + 1);
    let gutter = line_num.len();
    println!("  {} | {}", line_num, line_text);
    println!("  {} | {}^", " ".repeat(gutter), " ".repeat(col as usize));
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
