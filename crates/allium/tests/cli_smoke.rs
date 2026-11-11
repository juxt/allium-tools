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

fn plan_json(spec: &SpecFile) -> serde_json::Value {
    let out = allium().arg("plan").arg(spec.arg()).output().expect("spawn allium");
    serde_json::from_slice(&out.stdout).expect("plan stdout should be valid JSON")
}

#[test]
fn plan_against_reports_added_removed_changed_and_moved_ids() {
    let spec = SpecFile::new("plan-against", VALID);
    let mut old = plan_json(&spec);
    let obligations = old["obligations"].as_array_mut().unwrap();
    let find = |obs: &mut Vec<serde_json::Value>, id: &str| {
        obs.iter().position(|o| o["id"] == id).unwrap()
    };
    // Only in the new plan: added.
    let i = find(obligations, "surface-provides.JobIntake");
    obligations.remove(i);
    // Same id, different content: changed.
    let i = find(obligations, "rule-success.Finish");
    obligations[i]["description"] = "an older description".into();
    // Same content under another positional id: moved.
    let i = find(obligations, "rule-entity-creation.CreateJob.1");
    obligations[i]["id"] = "rule-entity-creation.CreateJob.2".into();
    // Only the span differs: not changed.
    let i = find(obligations, "entity-fields.Job");
    obligations[i]["source_span"] = serde_json::json!({"start": 0, "end": 1});
    // Only in the old plan, with content no new obligation has: removed.
    let i = find(obligations, "surface-actor.JobIntake");
    let mut gone = obligations[i].clone();
    gone["id"] = "surface-actor.Gone".into();
    gone["source_construct"] = "Gone".into();
    obligations.push(gone);
    let against = SpecFile::new("plan-against-old", &old.to_string());

    for args in [
        [spec.arg(), "--against".into(), against.arg()],
        ["--against".into(), against.arg(), spec.arg()],
    ] {
        let out = allium().arg("plan").args(&args).output().expect("spawn allium");
        assert!(out.status.success(), "expected exit 0, got {:?}", out.status);
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "added": ["surface-provides.JobIntake"],
                "removed": ["surface-actor.Gone"],
                "changed": ["rule-success.Finish"],
                "moved": [{
                    "from": "rule-entity-creation.CreateJob.2",
                    "to": "rule-entity-creation.CreateJob.1"
                }],
                "diagnostics": []
            })
        );
    }
}

#[test]
fn plan_against_an_unreadable_or_invalid_plan_exits_one() {
    let spec = SpecFile::new("plan-against-bad", VALID);
    let not_json = SpecFile::new("plan-against-not-json", "not json");
    let no_obligations = SpecFile::new("plan-against-no-obligations", "{\"version\": 3}");
    for against in [
        "/nonexistent/plan.json".to_string(),
        not_json.arg(),
        no_obligations.arg(),
    ] {
        let out = allium()
            .args(["plan", &spec.arg(), "--against", &against])
            .output()
            .expect("spawn allium");
        assert_eq!(out.status.code(), Some(1), "--against {against}");
        assert!(out.stdout.is_empty(), "no plan output expected for --against {against}");
        assert!(!out.stderr.is_empty(), "expected a message on stderr for --against {against}");
    }
}

#[test]
fn plan_against_without_a_path_is_a_usage_error() {
    let spec = SpecFile::new("plan-against-missing", VALID);
    let out = allium().args(["plan", &spec.arg(), "--against"]).output().expect("spawn allium");
    assert_eq!(out.status.code(), Some(2));
}

fn plan_with(args: &[&str], stdin: Option<&str>) -> std::process::Output {
    use std::io::Write;
    let mut child = allium()
        .arg("plan")
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn allium");
    child.stdin.take().unwrap().write_all(stdin.unwrap_or("").as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

fn ids_of(json: &serde_json::Value) -> Vec<&str> {
    json["obligations"].as_array().unwrap().iter().map(|o| o["id"].as_str().unwrap()).collect()
}

#[test]
fn plan_selectors_select_the_union_and_keep_the_shape() {
    let spec = SpecFile::new("plan-select", VALID);
    let out = plan_with(
        &[
            &spec.arg(),
            "--id", "rule-success.Finish",
            "--id-prefix", "transition-",
            "--construct", "JobIntake",
            "--category", "entity_fields",
        ],
        None,
    );
    assert!(out.status.success(), "expected exit 0, got {:?}", out.status);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let mut ids = ids_of(&json);
    ids.sort();
    assert_eq!(
        ids,
        [
            "entity-fields.Job",
            "rule-success.Finish",
            "surface-actor.JobIntake",
            "surface-provides.JobIntake",
            "transition-edge.Job.pending.done",
            "transition-rejected.Job.status",
            "transition-terminal.Job.status",
        ]
    );
    assert!(json.get("version").is_some() && json["diagnostics"] == serde_json::json!([]));
}

#[test]
fn plan_ids_from_reads_a_file_or_stdin() {
    let spec = SpecFile::new("plan-ids-from", VALID);
    let list = "  rule-success.Finish \n\nentity-fields.Job\n";
    let ids_file = SpecFile::new("plan-ids-from-list", list);
    for (path, stdin) in [(ids_file.arg(), None), ("-".to_string(), Some(list))] {
        let out = plan_with(&[&spec.arg(), "--ids-from", &path], stdin);
        assert!(out.status.success(), "expected exit 0, got {:?}", out.status);
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(ids_of(&json), ["entity-fields.Job", "rule-success.Finish"]);
    }
}

#[test]
fn plan_names_every_unknown_selector_value() {
    let spec = SpecFile::new("plan-unknown", VALID);
    let out = plan_with(
        &[
            &spec.arg(),
            "--id", "no-such-id",
            "--construct", "Ghost",
            "--category", "bogus",
            "--category", "invariant",
            "--id-prefix", "zzz",
        ],
        None,
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&out.stderr);
    // `invariant` is a category name, but no obligation of this plan carries it.
    for value in ["no-such-id", "Ghost", "bogus", "invariant"] {
        assert!(stderr.contains(value), "stderr should name {value}: {stderr}");
    }
    assert!(!stderr.contains("zzz"), "a prefix that matches nothing is no error: {stderr}");
}

#[test]
fn plan_covered_by_reports_covered_uncovered_and_unknown_ids() {
    let spec = SpecFile::new("plan-covered", VALID);
    let list = "rule-success.Finish\n  ghost.id\n\nrule-success.Finish\n";
    let ids_file = SpecFile::new("plan-covered-list", list);
    for (path, stdin) in [(ids_file.arg(), None), ("-".to_string(), Some(list))] {
        let out = plan_with(&[&spec.arg(), "--covered-by", &path], stdin);
        assert!(out.status.success(), "expected exit 0, got {:?}", out.status);
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "covered": ["rule-success.Finish"],
                "uncovered": [
                    "entity-fields.Job",
                    "rule-entity-creation.CreateJob.1",
                    "rule-success.CreateJob",
                    "surface-actor.JobIntake",
                    "surface-provides.JobIntake",
                    "transition-edge.Job.pending.done",
                    "transition-rejected.Job.status",
                    "transition-terminal.Job.status"
                ],
                "unknown": ["ghost.id"],
                "diagnostics": []
            })
        );
    }
    // A selector limits `uncovered` to the selected obligations.
    let out = plan_with(&[&spec.arg(), "--covered-by", &ids_file.arg(), "--construct", "CreateJob"], None);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["covered"], serde_json::json!([]));
    assert_eq!(
        json["uncovered"],
        serde_json::json!(["rule-entity-creation.CreateJob.1", "rule-success.CreateJob"])
    );
    assert_eq!(json["unknown"], serde_json::json!(["ghost.id"]));
}

#[test]
fn plan_selection_filters_both_plans_of_a_comparison() {
    let spec = SpecFile::new("plan-select-against", VALID);
    let mut old = plan_json(&spec);
    let obligations = old["obligations"].as_array_mut().unwrap();
    obligations.retain(|o| o["id"] != "surface-provides.JobIntake");
    for o in obligations.iter_mut() {
        if o["id"] == "rule-success.Finish" {
            o["description"] = "an older description".into();
        }
        if o["id"] == "entity-fields.Job" {
            o["description"] = "an older description".into();
        }
    }
    let against = SpecFile::new("plan-select-against-old", &old.to_string());
    let out = plan_with(&[&spec.arg(), "--against", &against.arg(), "--id-prefix", "rule-"], None);
    assert!(out.status.success(), "expected exit 0, got {:?}", out.status);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "added": [], "removed": [], "changed": ["rule-success.Finish"], "moved": [], "diagnostics": []
        })
    );
}

#[test]
fn plan_selection_and_coverage_usage_errors_exit_two() {
    let spec = SpecFile::new("plan-select-usage", VALID);
    let ids = SpecFile::new("plan-select-usage-ids", "rule-success.Finish\n");
    let s = spec.arg();
    let i = ids.arg();
    for args in [
        vec![s.as_str(), "--id"],
        vec![s.as_str(), "--covered-by", i.as_str(), "--against", i.as_str()],
        vec![s.as_str(), "--covered-by", i.as_str(), "--covered-by", i.as_str()],
        vec![s.as_str(), "--covered-by", "-", "--ids-from", "-"],
    ] {
        let out = plan_with(&args, None);
        assert_eq!(out.status.code(), Some(2), "args {args:?}");
        assert!(out.stdout.is_empty(), "args {args:?}");
    }
}

#[test]
fn plan_unreadable_id_lists_exit_one() {
    let spec = SpecFile::new("plan-select-unreadable", VALID);
    for option in ["--covered-by", "--ids-from"] {
        let out = plan_with(&[&spec.arg(), option, "/nonexistent/ids.txt"], None);
        assert_eq!(out.status.code(), Some(1), "{option}");
        assert!(out.stdout.is_empty(), "{option}");
        assert!(!out.stderr.is_empty(), "{option}");
    }
}

#[test]
fn plan_summary_counts_the_selected_obligations_per_category() {
    let spec = SpecFile::new("plan-summary", VALID);
    let out = plan_with(&[&spec.arg(), "--summary"], None);
    assert!(out.status.success(), "expected exit 0, got {:?}", out.status);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "total": 9,
            "by_category": {
                "entity_fields": 1,
                "rule_entity_creation": 1,
                "rule_success": 2,
                "surface_actor": 1,
                "surface_provides": 1,
                "transition_edge": 1,
                "transition_rejected": 1,
                "transition_terminal": 1
            },
            "diagnostics": []
        })
    );
    let out = plan_with(&["--summary", &spec.arg(), "--id-prefix", "rule-"], None);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["total"], 3);
    assert_eq!(json["by_category"], serde_json::json!({"rule_entity_creation": 1, "rule_success": 2}));
}

#[test]
fn plan_compact_keeps_the_shape_with_three_keys_per_obligation() {
    let spec = SpecFile::new("plan-compact", VALID);
    let out = plan_with(&[&spec.arg(), "--compact"], None);
    assert!(out.status.success(), "expected exit 0, got {:?}", out.status);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let mut keys: Vec<&String> = json.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["diagnostics", "obligations", "version"]);
    let full = plan_json(&spec);
    assert_eq!(json["obligations"].as_array().unwrap().len(), full["obligations"].as_array().unwrap().len());
    for o in json["obligations"].as_array().unwrap() {
        let mut keys: Vec<&String> = o.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, ["category", "id", "source_construct"]);
    }
    let out = plan_with(&[&spec.arg(), "--compact", "--construct", "Finish"], None);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        json["obligations"],
        serde_json::json!([{"id": "rule-success.Finish", "category": "rule_success", "source_construct": "Finish"}])
    );
}

#[test]
fn plan_summary_and_compact_usage_errors_exit_two() {
    let spec = SpecFile::new("plan-output-usage", VALID);
    let ids = SpecFile::new("plan-output-usage-ids", "rule-success.Finish\n");
    let s = spec.arg();
    let i = ids.arg();
    for args in [
        vec![s.as_str(), "--summary", "--compact"],
        vec![s.as_str(), "--summary", "--against", i.as_str()],
        vec![s.as_str(), "--summary", "--covered-by", i.as_str()],
        vec![s.as_str(), "--compact", "--against", i.as_str()],
        vec![s.as_str(), "--compact", "--covered-by", i.as_str()],
    ] {
        let out = plan_with(&args, None);
        assert_eq!(out.status.code(), Some(2), "args {args:?}");
        assert!(out.stdout.is_empty(), "args {args:?}");
    }
}

#[test]
fn plan_help_describes_the_output_shape() {
    let out = allium().args(["plan", "--help"]).output().expect("spawn allium");
    let help = String::from_utf8_lossy(&out.stdout);
    for word in ["version", "obligations", "diagnostics", "source_construct", "source_span", "--summary", "--compact"] {
        assert!(help.contains(word), "plan --help should mention {word}: {help}");
    }
}
