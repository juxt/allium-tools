//! `allium plan --format tsv` and `allium diff`: run the real binary and
//! assert the output formats and exit codes.

use std::fs;
use std::process::{Command, Output, Stdio};
use std::io::Write;

fn allium() -> Command {
    Command::new(env!("CARGO_BIN_EXE_allium"))
}

/// A throwaway file under the OS temp dir, removed on drop.
struct TempFile(std::path::PathBuf);
impl TempFile {
    fn new(name: &str, content: &str) -> Self {
        let path = std::env::temp_dir().join(format!("allium-plan-diff-{}-{name}", std::process::id()));
        fs::write(&path, content).unwrap();
        Self(path)
    }
    fn arg(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn run(args: &[&str]) -> Output {
    allium().args(args).output().expect("spawn allium")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn spec(requires: &[&str]) -> String {
    let clauses: String = requires.iter().map(|r| format!("    requires: {r}\n")).collect();
    format!(
        "-- allium: 3\n\n\
         entity Order {{\n    status: pending | paid\n    total: Integer\n}}\n\n\
         rule Pay {{\n    when: PayOrder(order)\n{clauses}    ensures: order.status = paid\n}}\n\n\
         surface Shop {{\n    provides:\n        PayOrder(order)\n}}\n"
    )
}

const PENDING: &str = "order.status = pending";
const POSITIVE: &str = "order.total > 0";

#[test]
fn plan_tsv_prints_five_columns_per_obligation() {
    let s = TempFile::new("tsv.allium", &spec(&[PENDING, POSITIVE]));
    let out = run(&["plan", "--format", "tsv", &s.arg()]);
    assert!(out.status.success(), "{:?}", out.status);
    let text = stdout(&out);
    for line in text.lines() {
        assert_eq!(line.split('\t').count(), 5, "line should have five columns: {line:?}");
    }
    assert!(
        text.lines().any(|l| l == format!(
            "rule-failure.Pay.2\trule_failure\tPay\tVerify rule Pay is rejected when requires clause fails\t{POSITIVE}"
        )),
        "{text}"
    );
}

#[test]
fn plan_tsv_and_json_list_the_same_obligations() {
    let s = TempFile::new("same.allium", &spec(&[PENDING, POSITIVE]));
    let json: serde_json::Value = serde_json::from_str(&stdout(&run(&["plan", &s.arg()]))).unwrap();
    let json_ids: Vec<&str> =
        json["obligations"].as_array().unwrap().iter().map(|o| o["id"].as_str().unwrap()).collect();
    let tsv = stdout(&run(&["plan", "--format=tsv", &s.arg()]));
    let tsv_ids: Vec<&str> = tsv.lines().map(|l| l.split('\t').next().unwrap()).collect();
    assert_eq!(json_ids, tsv_ids);
}

#[test]
fn plan_json_carries_the_requires_clause_as_expression() {
    let s = TempFile::new("expr.allium", &spec(&[PENDING]));
    let json: serde_json::Value = serde_json::from_str(&stdout(&run(&["plan", &s.arg()]))).unwrap();
    let failure = json["obligations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["id"] == "rule-failure.Pay.1")
        .expect("rule-failure.Pay.1");
    assert_eq!(failure["expression"], PENDING);
}

#[test]
fn plan_expression_ignores_comments_and_layout() {
    let s = TempFile::new("multiline.allium", &spec(&["order.status = pending -- open orders only\n        and order.total > 0"]));
    let text = stdout(&run(&["plan", "--format", "tsv", &s.arg()]));
    let line = text.lines().find(|l| l.starts_with("rule-failure.Pay.1\t")).expect(&text);
    assert!(line.ends_with("\torder.status = pending and order.total > 0"), "{line}");
}

#[test]
fn diff_ignores_a_comment_edit_or_reindent() {
    let old = TempFile::new("cmt-old.allium", &spec(&["order.status = pending -- open\n        and order.total > 0"]));
    let new = TempFile::new("cmt-new.allium", &spec(&["order.status = pending -- only open orders\n            and order.total > 0"]));
    let out = run(&["diff", &old.arg(), &new.arg()]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
}

#[test]
fn plan_tsv_escapes_newlines_in_values() {
    let s = TempFile::new("two-creations.allium", "-- allium: 3\n\n\
        entity A {\n    n: Integer\n}\n\nentity B {\n    n: Integer\n}\n\n\
        rule Make {\n    when: Go()\n    ensures:\n        A.created(n: 1)\n        B.created(n: 2)\n}\n\n\
        surface S {\n    provides:\n        Go()\n}\n");
    let text = stdout(&run(&["plan", "--format", "tsv", &s.arg()]));
    let line = text.lines().find(|l| l.starts_with("rule-entity-creation.Make.1\t")).expect(&text);
    assert!(line.ends_with("\tA.created(n: 1)\\nB.created(n: 2)"), "{line}");
}

#[test]
fn plan_tsv_reports_diagnostics_on_stderr_and_exits_one() {
    let s = TempFile::new("broken.allium", "-- allium: 3\n\nentity Broken {\n");
    let out = run(&["plan", "--format", "tsv", &s.arg()]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(": error: "), "{stderr}");
}

#[test]
fn plan_rejects_an_unknown_format_or_option() {
    let s = TempFile::new("opts.allium", &spec(&[PENDING]));
    assert_eq!(run(&["plan", "--format", "xml", &s.arg()]).status.code(), Some(2));
    assert_eq!(run(&["plan", "--format"]).status.code(), Some(2));
    assert_eq!(run(&["plan", "--summary", &s.arg()]).status.code(), Some(2));
}

#[test]
fn diff_of_identical_specs_prints_nothing_and_exits_zero() {
    let a = TempFile::new("same-a.allium", &spec(&[PENDING, POSITIVE]));
    let b = TempFile::new("same-b.allium", &spec(&[PENDING, POSITIVE]));
    let out = run(&["diff", &a.arg(), &b.arg()]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "");
}

#[test]
fn diff_reports_renumbered_clauses_as_moved() {
    let old = TempFile::new("ins-old.allium", &spec(&[PENDING, POSITIVE]));
    let new = TempFile::new("ins-new.allium", &spec(&["order.total < 1000", PENDING, POSITIVE]));
    let out = run(&["diff", &old.arg(), &new.arg()]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stdout(&out),
        "R\trule-failure.Pay.1\trule-failure.Pay.2\n\
         A\trule-failure.Pay.1\n\
         R\trule-failure.Pay.2\trule-failure.Pay.3\n"
    );
}

#[test]
fn diff_reports_a_removed_clause_and_the_one_that_moved_up() {
    let old = TempFile::new("del-old.allium", &spec(&[PENDING, POSITIVE]));
    let new = TempFile::new("del-new.allium", &spec(&[POSITIVE]));
    let out = run(&["diff", &old.arg(), &new.arg()]);
    assert_eq!(stdout(&out), "D\trule-failure.Pay.1\nR\trule-failure.Pay.2\trule-failure.Pay.1\n");
}

#[test]
fn diff_reports_an_edited_clause_as_changed() {
    let old = TempFile::new("edit-old.allium", &spec(&[PENDING, POSITIVE]));
    let new = TempFile::new("edit-new.allium", &spec(&[PENDING, "order.total > 10"]));
    let out = run(&["diff", &old.arg(), &new.arg()]);
    assert_eq!(stdout(&out), "M\trule-failure.Pay.2\n");
}

#[test]
fn diff_json_groups_changes_by_kind() {
    let old = TempFile::new("json-old.allium", &spec(&[PENDING, POSITIVE]));
    let new = TempFile::new("json-new.allium", &spec(&[POSITIVE]));
    let out = run(&["diff", "--format", "json", &old.arg(), &new.arg()]);
    let json: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "added": [],
            "removed": ["rule-failure.Pay.1"],
            "changed": [],
            "moved": [{"from": "rule-failure.Pay.2", "to": "rule-failure.Pay.1"}],
        })
    );
}

#[test]
fn diff_reads_plans_in_either_format_and_from_stdin() {
    let old_spec = TempFile::new("fmt-old.allium", &spec(&[PENDING, POSITIVE]));
    let new_spec = TempFile::new("fmt-new.allium", &spec(&[POSITIVE]));
    let old_json = TempFile::new("fmt-old.json", &stdout(&run(&["plan", &old_spec.arg()])));
    let new_tsv = TempFile::new("fmt-new.tsv", &stdout(&run(&["plan", "--format", "tsv", &new_spec.arg()])));
    let expected = "D\trule-failure.Pay.1\nR\trule-failure.Pay.2\trule-failure.Pay.1\n";

    assert_eq!(stdout(&run(&["diff", &old_json.arg(), &new_tsv.arg()])), expected);

    let mut child = allium()
        .args(["diff", "-", &new_spec.arg()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(spec(&[PENDING, POSITIVE]).as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(stdout(&out), expected, "a spec on stdin is recognised by its header");
}

#[test]
fn diff_exits_two_on_usage_or_input_errors() {
    let good = TempFile::new("err-good.allium", &spec(&[PENDING]));
    let junk = TempFile::new("err-junk.txt", "not a plan\n");
    let not_plan = TempFile::new("err-not-plan.json", "{\"diagnostics\": []}");
    let broken = TempFile::new("err-broken.allium", "-- allium: 3\n\nentity Broken {\n");

    assert_eq!(run(&["diff", &good.arg()]).status.code(), Some(2));
    assert_eq!(run(&["diff", "-", "-"]).status.code(), Some(2));
    assert_eq!(run(&["diff", "--format", "xml", &good.arg(), &good.arg()]).status.code(), Some(2));
    assert_eq!(run(&["diff", "/nonexistent/x.allium", &good.arg()]).status.code(), Some(2));
    for bad in [&junk, &not_plan, &broken] {
        let out = run(&["diff", &bad.arg(), &good.arg()]);
        assert_eq!(out.status.code(), Some(2), "{}", bad.arg());
        assert!(!out.stderr.is_empty());
    }
}
