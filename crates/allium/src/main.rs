use allium_parser::diagnostic::Severity;
use allium_parser::lexer::SourceMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        eprintln!("Usage: allium check <file.allium>...");
        eprintln!("       allium check <directory>");
        eprintln!("       allium parse <file.allium> [--json]");
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

fn cmd_parse(args: &[String]) -> ExitCode {
    let json_mode = args.iter().any(|a| a == "--json");
    let files: Vec<&str> = args.iter().map(|s| s.as_str()).filter(|s| *s != "--json").collect();

    if files.is_empty() {
        eprintln!("Usage: allium parse <file.allium> [--json]");
        return ExitCode::from(2);
    }

    let source = match std::fs::read_to_string(files[0]) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: {e}", files[0]);
            return ExitCode::from(1);
        }
    };

    let result = allium_parser::parse(&source);

    if json_mode {
        // Minimal JSON output: just diagnostics for now.
        // Full AST serialisation can come later with serde.
        println!("{{");
        println!("  \"file\": {:?},", files[0]);
        println!("  \"version\": {:?},", result.module.version);
        println!("  \"declarations\": {},", result.module.declarations.len());
        println!("  \"diagnostics\": [");
        let source_map = SourceMap::new(&source);
        for (i, d) in result.diagnostics.iter().enumerate() {
            let (line, col) = source_map.line_col(d.span.start);
            let severity = match d.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            };
            let comma = if i + 1 < result.diagnostics.len() { "," } else { "" };
            println!(
                "    {{\"line\": {}, \"col\": {}, \"severity\": \"{severity}\", \"message\": {:?}}}{}",
                line + 1,
                col + 1,
                d.message,
                comma,
            );
        }
        println!("  ]");
        println!("}}");
    } else {
        println!("Parsed: {}", files[0]);
        if let Some(v) = result.module.version {
            println!("Version: {v}");
        }
        println!("Declarations: {}", result.module.declarations.len());
        for d in &result.module.declarations {
            println!("  {}", describe_decl(d));
        }
        if !result.diagnostics.is_empty() {
            let source_map = SourceMap::new(&source);
            println!("Diagnostics:");
            for d in &result.diagnostics {
                let (line, col) = source_map.line_col(d.span.start);
                let severity = match d.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                };
                println!("  {}:{}: {severity}: {}", line + 1, col + 1, d.message);
                print_source_snippet(&source_map, &source, line, col);
            }
        }
    }

    if result.diagnostics.iter().any(|d| d.severity == Severity::Error) {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn describe_decl(decl: &allium_parser::ast::Decl) -> String {
    use allium_parser::ast::*;
    match decl {
        Decl::ModuleDecl(m) => format!("module {}", m.name.name),
        Decl::Use(u) => {
            let alias = u.alias.as_ref().map(|a| format!(" as {}", a.name)).unwrap_or_default();
            format!("use {:?}{alias}", u.path.parts.iter().map(|p| match p {
                StringPart::Text(t) => t.as_str(),
                _ => "{...}",
            }).collect::<String>())
        }
        Decl::Block(b) => {
            let name = b.name.as_ref().map(|n| n.name.as_str()).unwrap_or("(anonymous)");
            format!("{:?} {name} ({} items)", b.kind, b.items.len())
        }
        Decl::Default(d) => format!("default {}", d.name.name),
        Decl::Variant(v) => format!("variant {} : {:?}", v.name.name, v.base),
        Decl::Deferred(_) => "deferred ...".to_string(),
        Decl::OpenQuestion(q) => {
            let text: String = q.text.parts.iter().map(|p| match p {
                StringPart::Text(t) => t.clone(),
                StringPart::Interpolation(id) => format!("{{{}}}", id.name),
            }).collect();
            format!("open question \"{text}\"")
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
