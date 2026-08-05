//! Witness matrix: a systematic sweep over how a cross-module transition
//! witness can be expressed, so a false lifecycle report on a *valid* witness
//! surfaces regardless of which binding-type source or module layout is used.
//!
//! Every scenario is a complete, valid lifecycle: entity `Job` is created in
//! `pending`, a rule witnesses `pending -> done`, and `done` is terminal. The
//! only thing that varies is *how the witnessing rule's binding is typed to the
//! entity* (the binding-type source) and whether the spec is one file or split
//! across a `use` edge. All of them should be completely clean.
//!
//! Two properties per scenario:
//!   1. the single-file form (a valid witness) reports nothing;
//!   2. the split form reports exactly what the single-file form does
//!      (the merged-single-file oracle from #66/#74).
//!
//! Discovery mode: this collects every anomaly and reports them together, so a
//! sweep shows the whole failing set at once rather than the first cell.

use std::fs;
use std::path::Path;
use std::process::Command;

fn allium() -> Command {
    Command::new(env!("CARGO_BIN_EXE_allium"))
}

struct TempDir {
    path: std::path::PathBuf,
}
impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("allium-wm-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }
    fn write(&self, name: &str, content: &str) {
        fs::write(self.path.join(name), content).unwrap();
    }
    fn file(&self, name: &str) -> String {
        self.path.join(name).to_string_lossy().into_owned()
    }
    fn path(&self) -> &Path {
        &self.path
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn split_json_docs(s: &str) -> Vec<String> {
    let mut docs = Vec::new();
    let (mut depth, mut start, mut in_str, mut esc) = (0i32, None, false, false);
    for (i, ch) in s.char_indices() {
        if in_str {
            if esc {
                esc = false;
            } else if ch == '\\' {
                esc = true;
            } else if ch == '"' {
                in_str = false;
            }
            continue;
        }
        match ch {
            '"' => in_str = true,
            '{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(s0) = start {
                        docs.push(s[s0..=i].to_string());
                    }
                    start = None;
                }
            }
            _ => {}
        }
    }
    docs
}

/// A canonical report set, independent of which file a report lands in: the
/// diagnostic codes and finding types (with the entity/status they name, via
/// the message, which is filename- and line-free).
fn report_set(stdout: &str) -> Vec<String> {
    let mut rows = Vec::new();
    for doc in split_json_docs(stdout) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&doc) else {
            continue;
        };
        if let Some(arr) = v["diagnostics"].as_array() {
            for d in arr {
                if let (Some(c), Some(m)) = (d["code"].as_str(), d["message"].as_str()) {
                    rows.push(format!("D {c} :: {m}"));
                }
            }
        }
        if let Some(arr) = v["findings"].as_array() {
            for f in arr {
                rows.push(format!(
                    "F {} :: {}",
                    f["type"].as_str().unwrap_or(""),
                    f["summary"].as_str().unwrap_or("")
                ));
            }
        }
    }
    rows.sort();
    rows
}

fn run(cmd: &str, args: &[&str]) -> String {
    let out = allium().arg(cmd).args(args).output().expect("spawn allium");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn reports_of_file(content: &str) -> Vec<String> {
    let dir = TempDir::new("single");
    dir.write("spec.allium", content);
    let mut all = report_set(&run("check", &[&dir.file("spec.allium")]));
    all.extend(report_set(&run("analyse", &[&dir.file("spec.allium")])));
    all.sort();
    all.dedup();
    all
}

fn reports_of_pair(domain: &str, consumer: &str) -> Vec<String> {
    let dir = TempDir::new("pair");
    dir.write("domain.allium", domain);
    dir.write("consumer.allium", consumer);
    let mut all = report_set(&run("check", &[dir.path().to_str().unwrap()]));
    all.extend(report_set(&run("analyse", &[dir.path().to_str().unwrap()])));
    all.sort();
    all.dedup();
    all
}

// ---------------------------------------------------------------------------
// Scenario generation: one valid witness of Job: pending -> done, expressed
// through each binding-type source, as a single file and as a domain+consumer
// split.
// ---------------------------------------------------------------------------

/// The domain always present: entity, creation rule, and a surface providing
/// the creation trigger (so the creation trigger is never unreachable).
const DOMAIN_BASE: &str = "entity Job {\n    status: pending | done\n    transitions status { pending -> done  terminal: done }\n}\n\nrule CreateJob {\n    when: JobRequested()\n    ensures: Job.created(status: pending)\n}\n\nsurface JobIntake {\n    provides:\n        JobRequested()\n}\n";

struct Scenario {
    name: &'static str,
    single: String,
    domain: String,
    consumer: String,
}

fn scenarios() -> Vec<Scenario> {
    // (domain-side extra, consumer body qualified via `dom/`, consumer body local)
    // Each consumer body witnesses pending -> done.
    let cases: Vec<(&'static str, &str, &str, &str)> = vec![
        (
            "sub_requires",
            "surface JobDesk {\n    provides:\n        Ready(x: Job)\n            when x.status = pending\n}\n",
            "rule Witness {\n    when: dom/Ready(b)\n    requires: b.status = pending\n    ensures: b.status = done\n}\n",
            "rule Witness {\n    when: Ready(b)\n    requires: b.status = pending\n    ensures: b.status = done\n}\n",
        ),
        (
            "becomes",
            "",
            "rule Witness {\n    when: b: dom/Job.status becomes pending\n    ensures: b.status = done\n}\n",
            "rule Witness {\n    when: b: Job.status becomes pending\n    ensures: b.status = done\n}\n",
        ),
        (
            "transitions_to",
            "",
            "rule Witness {\n    when: b: dom/Job.status transitions_to pending\n    ensures: b.status = done\n}\n",
            "rule Witness {\n    when: b: Job.status transitions_to pending\n    ensures: b.status = done\n}\n",
        ),
        (
            "importer_context",
            "",
            "surface WDesk {\n    context b: dom/Job\n    provides:\n        Ready(b)\n            when b.status = pending\n}\n\nrule Witness {\n    when: Ready(z)\n    requires: z.status = pending\n    ensures: z.status = done\n}\n",
            "surface WDesk {\n    context b: Job\n    provides:\n        Ready(b)\n            when b.status = pending\n}\n\nrule Witness {\n    when: Ready(z)\n    requires: z.status = pending\n    ensures: z.status = done\n}\n",
        ),
        (
            "importer_context_where_eq",
            "",
            "surface WDesk {\n    context b: dom/Job where status = pending\n    provides:\n        Ready(b)\n            when b.status = pending\n}\n\nrule Witness {\n    when: Ready(z)\n    requires: z.status = pending\n    ensures: z.status = done\n}\n",
            "surface WDesk {\n    context b: Job where status = pending\n    provides:\n        Ready(b)\n            when b.status = pending\n}\n\nrule Witness {\n    when: Ready(z)\n    requires: z.status = pending\n    ensures: z.status = done\n}\n",
        ),
        (
            "importer_context_where_in",
            "",
            "surface WDesk {\n    context b: dom/Job where status in {pending, done}\n    provides:\n        Ready(b)\n            when b.status = pending\n}\n\nrule Witness {\n    when: Ready(z)\n    requires: z.status = pending\n    ensures: z.status = done\n}\n",
            "surface WDesk {\n    context b: Job where status in {pending, done}\n    provides:\n        Ready(b)\n            when b.status = pending\n}\n\nrule Witness {\n    when: Ready(z)\n    requires: z.status = pending\n    ensures: z.status = done\n}\n",
        ),
        (
            "importer_inline",
            "",
            "surface WDesk {\n    provides:\n        Ready(b: dom/Job)\n            when b.status = pending\n}\n\nrule Witness {\n    when: Ready(z)\n    requires: z.status = pending\n    ensures: z.status = done\n}\n",
            "surface WDesk {\n    provides:\n        Ready(b: Job)\n            when b.status = pending\n}\n\nrule Witness {\n    when: Ready(z)\n    requires: z.status = pending\n    ensures: z.status = done\n}\n",
        ),
        (
            "rule_emission",
            "rule Announce {\n    when: j: Job.status becomes pending\n    ensures: Ready(job: j)\n}\n",
            "rule Witness {\n    when: dom/Ready(b)\n    requires: b.status = pending\n    ensures: b.status = done\n}\n",
            "rule Witness {\n    when: Ready(b)\n    requires: b.status = pending\n    ensures: b.status = done\n}\n",
        ),
        (
            "rule_emission_transitions_to",
            "rule Announce {\n    when: j: Job.status transitions_to pending\n    ensures: Ready(job: j)\n}\n",
            "rule Witness {\n    when: dom/Ready(b)\n    requires: b.status = pending\n    ensures: b.status = done\n}\n",
            "rule Witness {\n    when: Ready(b)\n    requires: b.status = pending\n    ensures: b.status = done\n}\n",
        ),
        (
            "branch_target",
            "surface JobDesk {\n    provides:\n        Ready(x: Job, flag)\n            when x.status = pending\n}\n",
            "rule Witness {\n    when: dom/Ready(b, ok)\n    requires: b.status = pending\n    if ok:\n        ensures: b.status = done\n    else:\n        ensures: b.status = done\n}\n",
            "rule Witness {\n    when: Ready(b, ok)\n    requires: b.status = pending\n    if ok:\n        ensures: b.status = done\n    else:\n        ensures: b.status = done\n}\n",
        ),
    ];

    let mut out: Vec<Scenario> = cases
        .into_iter()
        .map(|(name, domain_extra, consumer_qual, consumer_local)| {
            let single = format!("-- allium: 3\n\n{DOMAIN_BASE}\n{domain_extra}\n{consumer_local}");
            let domain = format!("-- allium: 3\n\n{DOMAIN_BASE}\n{domain_extra}");
            let consumer =
                format!("-- allium: 3\n\nuse \"./domain.allium\" as dom\n\n{consumer_qual}");
            Scenario { name, single, domain, consumer }
        })
        .collect();

    // Multi-hop lifecycle (pending -> active -> done) witnessed across a module,
    // one becomes-triggered rule per hop. Needs its own 3-state entity, so it
    // does not use DOMAIN_BASE.
    let mh_entity = "entity Job {\n    status: pending | active | done\n    transitions status { pending -> active  active -> done  terminal: done }\n}\n\nrule CreateJob {\n    when: JobRequested()\n    ensures: Job.created(status: pending)\n}\n\nsurface JobIntake {\n    provides:\n        JobRequested()\n}\n";
    out.push(Scenario {
        name: "multi_hop",
        single: format!(
            "-- allium: 3\n\n{mh_entity}\nrule W1 {{\n    when: a: Job.status becomes pending\n    ensures: a.status = active\n}}\n\nrule W2 {{\n    when: c: Job.status becomes active\n    ensures: c.status = done\n}}\n"
        ),
        domain: format!("-- allium: 3\n\n{mh_entity}"),
        consumer: "-- allium: 3\n\nuse \"./domain.allium\" as dom\n\nrule W1 {\n    when: a: dom/Job.status becomes pending\n    ensures: a.status = active\n}\n\nrule W2 {\n    when: c: dom/Job.status becomes active\n    ensures: c.status = done\n}\n".to_string(),
    });

    out
}

#[test]
fn witness_matrix_sweep() {
    let mut anomalies: Vec<String> = Vec::new();

    for sc in scenarios() {
        let single = reports_of_file(&sc.single);
        let split = reports_of_pair(&sc.domain, &sc.consumer);

        if !single.is_empty() {
            anomalies.push(format!(
                "[{}] SINGLE-FILE not clean (valid witness should report nothing):\n    {}\n--- spec ---\n{}",
                sc.name,
                single.join("\n    "),
                sc.single
            ));
        }
        if split != single {
            anomalies.push(format!(
                "[{}] SPLIT != SINGLE (split-invariance violation):\n  single: {:?}\n  split:  {:?}",
                sc.name, single, split
            ));
        }
    }

    if !anomalies.is_empty() {
        panic!(
            "\n==== WITNESS MATRIX: {} anomalies ====\n\n{}\n",
            anomalies.len(),
            anomalies.join("\n\n")
        );
    }
}

// ---------------------------------------------------------------------------
// Alias-anchoring sweep: at every site a qualified reference `alias/Name` can
// appear, a qualifier that matches no `use` alias is a locally-knowable typo
// and should be diagnosed at the reference. This is the "sites audit" as a
// test: #72 covers `provides:`, #78 asks for `when:`; the rest are unaudited.
//
// Each snippet declares `dom` and then misspells one qualifier as `nosuch`.
// A site is caught if some diagnostic names `nosuch`.
// ---------------------------------------------------------------------------

fn diagnostics_mentioning(content: &str, needle: &str) -> Vec<String> {
    let dir = TempDir::new("alias");
    dir.write("spec.allium", content);
    let out = run("check", &[&dir.file("spec.allium")]);
    let mut hits = Vec::new();
    for doc in split_json_docs(&out) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&doc) else {
            continue;
        };
        if let Some(arr) = v["diagnostics"].as_array() {
            for d in arr {
                let msg = d["message"].as_str().unwrap_or("");
                if msg.contains(needle) {
                    hits.push(format!("{} :: {msg}", d["code"].as_str().unwrap_or("")));
                }
            }
        }
    }
    hits
}

#[test]
fn alias_anchoring_sweep() {
    let head = "-- allium: 3\n\nuse \"./domain.allium\" as dom\n\n";
    // (site name, snippet after the `use dom` header, with one `nosuch/` typo)
    let sites: Vec<(&str, String)> = vec![
        ("when_subscription", format!("{head}rule R {{\n    when: nosuch/Ready(x)\n    ensures: Done()\n}}\n")),
        ("when_transition_trigger", format!("{head}rule R {{\n    when: t: nosuch/Job.status becomes pending\n    ensures: t.status = done\n}}\n")),
        ("provides", format!("{head}surface S {{\n    provides:\n        nosuch/Ready()\n}}\n")),
        ("surface_context", format!("{head}surface S {{\n    context b: nosuch/Job\n    provides:\n        Ready(b)\n}}\n")),
        ("inline_provides_param", format!("{head}surface S {{\n    provides:\n        Ready(b: nosuch/Job)\n}}\n")),
        ("field_type", format!("{head}entity Wrapper {{\n    j: nosuch/Job\n}}\n")),
        ("ensures_created", format!("{head}rule R {{\n    when: Go()\n    ensures: nosuch/Job.created(status: pending)\n}}\n")),
        ("default_type", format!("{head}default nosuch/Config c = {{ enabled: true }}\n")),
        ("requires_entity", format!("{head}rule R {{\n    when: Go()\n    requires: nosuch/Job.status = pending\n    ensures: Done()\n}}\n")),
        ("ensures_status", format!("{head}rule R {{\n    when: Go()\n    ensures: nosuch/Job.status = done\n}}\n")),
        ("invariant_ref", format!("{head}invariant I {{\n    nosuch/Job.status = done\n}}\n")),
        ("contract_fulfils", format!("{head}surface S {{\n    facing u: User\n    contracts:\n        fulfils nosuch/MyContract\n}}\n")),
        ("field_type_in_value", format!("{head}value Wrapper {{\n    j: nosuch/Job\n}}\n")),
    ];

    let mut uncaught: Vec<String> = Vec::new();
    for (site, snippet) in sites {
        let hits = diagnostics_mentioning(&snippet, "nosuch");
        if hits.is_empty() {
            uncaught.push(format!("[{site}] NO diagnostic names the undeclared alias 'nosuch'\n--- spec ---\n{snippet}"));
        }
    }

    if !uncaught.is_empty() {
        panic!(
            "\n==== ALIAS ANCHORING: {} sites do not diagnose an undeclared qualifier ====\n\n{}\n",
            uncaught.len(),
            uncaught.join("\n\n")
        );
    }
}
