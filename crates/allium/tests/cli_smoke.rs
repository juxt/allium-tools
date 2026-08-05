//! End-to-end smoke tests for the `check`/`analyse`/`parse` commands: run the
//! real binary on a written-out spec and assert the exit-code and output-shape
//! contract. These guard the CLI surface itself (JSON envelope, exit codes),
//! which the in-process analyser tests never exercise.

use std::fs;
use std::process::Command;

fn allium() -> Command {
    Command::new(env!("CARGO_BIN_EXE_allium"))
}

/// A throwaway spec file under the OS temp dir, removed on drop.
struct SpecFile {
    path: std::path::PathBuf,
}
impl SpecFile {
    fn new(tag: &str, content: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "allium-smoke-{tag}-{}.allium",
            std::process::id()
        ));
        fs::write(&path, content).unwrap();
        Self { path }
    }
    fn arg(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}
impl Drop for SpecFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

const VALID: &str = "-- allium: 3\n\n\
    entity Job {\n    status: pending | done\n    transitions status { pending -> done  terminal: done }\n}\n\n\
    rule CreateJob {\n    when: JobRequested()\n    ensures: Job.created(status: pending)\n}\n\n\
    rule Finish {\n    when: j: Job.status becomes pending\n    ensures: j.status = done\n}\n\n\
    surface JobIntake {\n    provides:\n        JobRequested()\n}\n";

// References an undeclared entity `Ghost`, which is an error-severity diagnostic.
const BROKEN: &str = "-- allium: 3\n\n\
    rule R {\n    when: Go()\n    ensures: Ghost.created(status: pending)\n}\n";

#[test]
fn check_valid_spec_exits_zero_with_empty_reports() {
    let spec = SpecFile::new("valid", VALID);
    let out = allium().arg("check").arg(spec.arg()).output().expect("spawn allium");
    assert!(out.status.success(), "expected exit 0, got {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("\"command\": \"check\""), "missing command envelope: {stdout}");
    assert!(stdout.contains("\"diagnostics\": []"), "expected no diagnostics: {stdout}");
    assert!(stdout.contains("\"findings\": []"), "expected no findings: {stdout}");
}

#[test]
fn check_spec_with_error_exits_one_and_names_the_offender() {
    let spec = SpecFile::new("broken", BROKEN);
    let out = allium().arg("check").arg(spec.arg()).output().expect("spawn allium");
    assert_eq!(out.status.code(), Some(1), "an error-severity diagnostic should exit 1");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("allium.rule.undefinedTypeReference"), "expected the type-ref error: {stdout}");
    assert!(stdout.contains("\"severity\": \"error\""), "expected an error severity: {stdout}");
    assert!(stdout.contains("Ghost"), "diagnostic should name the offending reference: {stdout}");
}

#[test]
fn analyse_valid_spec_emits_the_json_envelope() {
    let spec = SpecFile::new("analyse", VALID);
    let out = allium().arg("analyse").arg(spec.arg()).output().expect("spawn allium");
    assert!(out.status.success(), "expected exit 0, got {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    for key in ["\"command\"", "\"diagnostics\"", "\"findings\""] {
        assert!(stdout.contains(key), "analyse output missing {key}: {stdout}");
    }
    // Output must be a single well-formed JSON document.
    serde_json::from_str::<serde_json::Value>(stdout.trim())
        .expect("analyse stdout should be valid JSON");
}

#[test]
fn parse_valid_spec_exits_zero() {
    let spec = SpecFile::new("parse", VALID);
    let out = allium().arg("parse").arg(spec.arg()).output().expect("spawn allium");
    assert!(out.status.success(), "expected exit 0 on a well-formed spec, got {:?}", out.status);
}
