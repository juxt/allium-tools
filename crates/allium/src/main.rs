use allium_parser::diagnostic::Severity;
use allium_parser::lexer::SourceMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        eprintln!("Usage: allium check <file.allium>...");
        eprintln!("       allium check <directory>");
        eprintln!("       allium parse <file.allium>");
        return ExitCode::from(2);
    }

    match args[0].as_str() {
        "check" => cmd_check(&args[1..]),
        "parse" => cmd_parse(&args[1..]),
        other => {
            eprintln!("Unknown command: {other}");
            eprintln!("Available commands: check, parse");
            ExitCode::from(2)
        }
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
        let source_map = SourceMap::new(&source);

        for d in &result.diagnostics {
            let (line, col) = source_map.line_col(d.span.start);
            let severity = match d.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            };
            println!(
                "{}:{}:{}: {severity}: {}",
                path.display(),
                line + 1,
                col + 1,
                d.message,
            );
            print_source_snippet(&source_map, &source, line, col);

            match d.severity {
                Severity::Error => total_errors += 1,
                Severity::Warning => total_warnings += 1,
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
