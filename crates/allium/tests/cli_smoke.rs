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

// Does not parse: an entity block left open at end of file.
const UNPARSEABLE: &str = "-- allium: 3\n\nentity Broken {\n";

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

// Regression for #80: the single-file commands (`plan`, `model`, `parse`)
// must not launder a parse failure into a plausible-looking empty success.
// A spec that does not parse exits non-zero and surfaces the diagnostic in
// its JSON output, so a consumer cannot mistake "unparseable garbage" for
// "valid spec with nothing to report".

#[test]
fn single_file_commands_exit_nonzero_on_unparseable_spec() {
    for command in ["plan", "model", "parse"] {
        let spec = SpecFile::new(&format!("unparseable-{command}"), UNPARSEABLE);
        let out = allium().arg(command).arg(spec.arg()).output().expect("spawn allium");
        assert_eq!(
            out.status.code(),
            Some(1),
            "{command} on an unparseable spec should exit 1, got {:?}",
            out.status
        );
    }
}

#[test]
fn plan_surfaces_parse_diagnostics_in_its_json() {
    let spec = SpecFile::new("plan-unparseable", UNPARSEABLE);
    let out = allium().arg("plan").arg(spec.arg()).output().expect("spawn allium");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("plan stdout should be valid JSON");
    let diags = json["diagnostics"].as_array().expect("plan output should carry a diagnostics array");
    assert!(
        diags.iter().any(|d| d["severity"] == "error"),
        "plan should surface the parse error in its diagnostics: {stdout}"
    );
    // The empty obligation set must be accompanied by the error, not stand alone.
    assert_eq!(json["obligations"].as_array().map(Vec::len), Some(0));
}

#[test]
fn plan_valid_spec_exits_zero_with_empty_diagnostics() {
    let spec = SpecFile::new("plan-valid", VALID);
    let out = allium().arg("plan").arg(spec.arg()).output().expect("spawn allium");
    assert!(out.status.success(), "a valid spec should exit 0, got {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("plan stdout should be valid JSON");
    assert_eq!(
        json["diagnostics"].as_array().map(Vec::len),
        Some(0),
        "a valid spec should carry an empty diagnostics array: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// v4 diagnostic format — the unified, LLM-facing shape. These lock the schema
// (code / lowercase severity / resolved location / structural fix), the
// de-duplication, the retained gate keywords, and the absence of em-dashes, so
// a later refactor cannot silently regress the format an LLM consumer relies on.
// ---------------------------------------------------------------------------

const V4_UNDECLARED: &str = "-- allium: 4\n\
    component C\n  observable state x : Number\n  invariant i means y >= 0\nend\n";

const V4_CONTRADICTION: &str = "-- allium: 4\n\
    component C\n  observable state x : Number\n\
    invariant lo means x >= 10\n  invariant hi means x <= 5\nend\n";

// `bal` is undeclared and referenced twice in one invariant (same item span),
// which previously produced two identical warnings.
const V4_DUP: &str = "-- allium: 4\n\
    component C\n  entity E\n  observable state x(E) : Number\n\
    invariant i means every p :: every q :: bal(q) = bal(p)\nend\n";

fn one_diag(stdout: &str) -> serde_json::Value {
    let json: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("v4 output should be valid JSON");
    let diags = json["diagnostics"].as_array().expect("diagnostics array");
    assert!(!diags.is_empty(), "expected at least one diagnostic: {stdout}");
    diags[0].clone()
}

#[test]
fn v4_check_diagnostic_uses_unified_shape() {
    let spec = SpecFile::new("v4-undeclared", V4_UNDECLARED);
    let out = allium().arg("check").arg(spec.arg()).output().expect("spawn allium");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let d = one_diag(&stdout);
    assert_eq!(d["code"], "undeclared-name", "stable code expected: {stdout}");
    assert_eq!(d["severity"], "warning", "severity must be lowercase: {stdout}");
    assert!(d["location"]["file"].is_string(), "location.file expected: {stdout}");
    assert!(d["location"]["line"].is_number(), "location.line expected: {stdout}");
    assert!(d["location"]["col"].is_number(), "location.col expected: {stdout}");
    assert!(d["fix"].is_string(), "a structural fix string expected: {stdout}");
    assert!(d.get("span").is_none(), "raw byte span must not appear: {stdout}");
    assert!(
        d["message"].as_str().unwrap().contains("not a declared name"),
        "message names the problem: {stdout}"
    );
}

#[test]
fn v4_contradiction_carries_code_fix_and_keeps_gate_keyword() {
    let spec = SpecFile::new("v4-contradiction", V4_CONTRADICTION);
    let out = allium().arg("analyse").arg(spec.arg()).output().expect("spawn allium");
    assert_eq!(out.status.code(), Some(1), "a contradiction must fail the gate (exit 1)");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).expect("valid JSON");
    let diags = json["diagnostics"].as_array().expect("diagnostics array");
    let c = diags
        .iter()
        .find(|d| d["code"] == "contradictory-invariants")
        .unwrap_or_else(|| panic!("expected a contradictory-invariants diagnostic: {stdout}"));
    assert!(c["fix"].is_string(), "contradiction should carry a structural fix: {stdout}");
    assert!(
        c["message"].as_str().unwrap().contains("CONTRADICTORY"),
        "gate keyword CONTRADICTORY must stay in the message: {stdout}"
    );
    // Component `C` is on line 2 (line 1 is the version marker). A resolved location, not the old
    // {0,0} stub that reported line 1 for every component-level contradiction.
    assert_eq!(
        c["location"]["line"].as_u64(),
        Some(2),
        "contradiction should resolve to the component's real line, not the 0/0 stub: {stdout}"
    );
}

#[test]
fn v4_identical_diagnostics_are_deduplicated() {
    let spec = SpecFile::new("v4-dup", V4_DUP);
    let out = allium().arg("check").arg(spec.arg()).output().expect("spawn allium");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).expect("valid JSON");
    let diags = json["diagnostics"].as_array().expect("diagnostics array");
    let bal = diags
        .iter()
        .filter(|d| d["message"].as_str().map(|m| m.contains("`bal`")).unwrap_or(false))
        .count();
    assert_eq!(bal, 1, "the duplicate undeclared-name warning should be de-duplicated: {stdout}");
}

#[test]
fn v4_messages_have_no_em_dashes() {
    for (tag, src, cmd) in [
        ("undeclared", V4_UNDECLARED, "check"),
        ("contradiction", V4_CONTRADICTION, "analyse"),
    ] {
        let spec = SpecFile::new(&format!("v4-emdash-{tag}"), src);
        let out = allium().arg(cmd).arg(spec.arg()).output().expect("spawn allium");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let json: serde_json::Value = serde_json::from_str(stdout.trim()).expect("valid JSON");
        for d in json["diagnostics"].as_array().unwrap_or(&vec![]) {
            for key in ["message", "fix"] {
                if let Some(s) = d[key].as_str() {
                    assert!(!s.contains('\u{2014}'), "em-dash in {key}: {s}");
                }
            }
        }
    }
}

// A malformed predicate body (here a dangling operator) must NOT pass silently: it constrains
// nothing, so a clean check would mean nothing. It is surfaced with the `malformed-predicate` code.
const V4_MALFORMED: &str = "-- allium: 4\n\
    component C\n  observable state emi : Number\n  invariant i means emi +\nend\n";

#[test]
fn v4_malformed_predicate_is_not_silently_accepted() {
    let spec = SpecFile::new("v4-malformed", V4_MALFORMED);
    let out = allium().arg("check").arg(spec.arg()).output().expect("spawn allium");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).expect("valid JSON");
    let diags = json["diagnostics"].as_array().expect("diagnostics array");
    assert!(
        diags.iter().any(|d| d["code"] == "malformed-predicate"),
        "a dangling-operator predicate must be flagged, not accepted silently: {stdout}"
    );
}

// A state whose entity sort is undeclared (`observable state x(Ghost)` with no `entity Ghost`) is
// meaningless; it must be flagged, not accepted silently.
const V4_UNDECLARED_ENTITY: &str = "-- allium: 4\n\
    component C\n  observable state x(Ghost) : Number\n  invariant i means every p :: x(p) >= 0\nend\n";

#[test]
fn v4_undeclared_entity_sort_is_flagged() {
    let spec = SpecFile::new("v4-undeclared-entity", V4_UNDECLARED_ENTITY);
    let out = allium().arg("check").arg(spec.arg()).output().expect("spawn allium");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).expect("valid JSON");
    assert!(
        json["diagnostics"].as_array().unwrap().iter().any(|d| d["code"] == "undeclared-entity"),
        "a state over an undeclared entity sort must be flagged: {stdout}"
    );
}
