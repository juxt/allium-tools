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
use std::sync::atomic::{AtomicU64, Ordering};

fn allium() -> Command {
    Command::new(env!("CARGO_BIN_EXE_allium"))
}

// Unique per TempDir, so tests running in parallel never share a path. Keying on
// the process id alone let concurrent tests clobber each other's `single`/`pair`
// directories, which showed up as flaky split-invariance failures in the full run.
static TEMPDIR_SEQ: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: std::path::PathBuf,
}
impl TempDir {
    fn new(name: &str) -> Self {
        let seq = TEMPDIR_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir()
            .join(format!("allium-wm-{name}-{}-{seq}", std::process::id()));
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
            "importer_facing",
            "",
            "surface WDesk {\n    facing b: dom/Job\n    provides:\n        Ready(b)\n            when b.status = pending\n}\n\nrule Witness {\n    when: Ready(z)\n    requires: z.status = pending\n    ensures: z.status = done\n}\n",
            "surface WDesk {\n    facing b: Job\n    provides:\n        Ready(b)\n            when b.status = pending\n}\n\nrule Witness {\n    when: Ready(z)\n    requires: z.status = pending\n    ensures: z.status = done\n}\n",
        ),
        (
            "importer_inline",
            "",
            "surface WDesk {\n    provides:\n        Ready(b: dom/Job)\n            when b.status = pending\n}\n\nrule Witness {\n    when: Ready(z)\n    requires: z.status = pending\n    ensures: z.status = done\n}\n",
            "surface WDesk {\n    provides:\n        Ready(b: Job)\n            when b.status = pending\n}\n\nrule Witness {\n    when: Ready(z)\n    requires: z.status = pending\n    ensures: z.status = done\n}\n",
        ),
        (
            "importer_inline_where",
            "",
            "surface WDesk {\n    provides:\n        Ready(b: dom/Job where status = pending)\n            when b.status = pending\n}\n\nrule Witness {\n    when: Ready(z)\n    requires: z.status = pending\n    ensures: z.status = done\n}\n",
            "surface WDesk {\n    provides:\n        Ready(b: Job where status = pending)\n            when b.status = pending\n}\n\nrule Witness {\n    when: Ready(z)\n    requires: z.status = pending\n    ensures: z.status = done\n}\n",
        ),
        (
            "importer_inline_with",
            "",
            "surface WDesk {\n    provides:\n        Ready(b: dom/Job with status = pending)\n            when b.status = pending\n}\n\nrule Witness {\n    when: Ready(z)\n    requires: z.status = pending\n    ensures: z.status = done\n}\n",
            "surface WDesk {\n    provides:\n        Ready(b: Job with status = pending)\n            when b.status = pending\n}\n\nrule Witness {\n    when: Ready(z)\n    requires: z.status = pending\n    ensures: z.status = done\n}\n",
        ),
        (
            "importer_context_with",
            "",
            "surface WDesk {\n    context b: dom/Job with status = pending\n    provides:\n        Ready(b)\n            when b.status = pending\n}\n\nrule Witness {\n    when: Ready(z)\n    requires: z.status = pending\n    ensures: z.status = done\n}\n",
            "surface WDesk {\n    context b: Job with status = pending\n    provides:\n        Ready(b)\n            when b.status = pending\n}\n\nrule Witness {\n    when: Ready(z)\n    requires: z.status = pending\n    ensures: z.status = done\n}\n",
        ),
        (
            "importer_facing_with",
            "",
            "surface WDesk {\n    facing b: dom/Job with status = pending\n    provides:\n        Ready(b)\n            when b.status = pending\n}\n\nrule Witness {\n    when: Ready(z)\n    requires: z.status = pending\n    ensures: z.status = done\n}\n",
            "surface WDesk {\n    facing b: Job with status = pending\n    provides:\n        Ready(b)\n            when b.status = pending\n}\n\nrule Witness {\n    when: Ready(z)\n    requires: z.status = pending\n    ensures: z.status = done\n}\n",
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
            "emission_command_typed",
            "surface Kicker {\n    provides:\n        Kick(j: Job)\n}\n\nrule Announce {\n    when: Kick(j)\n    ensures: Ready(job: j)\n}\n",
            "rule Witness {\n    when: dom/Ready(b)\n    requires: b.status = pending\n    ensures: b.status = done\n}\n",
            "rule Witness {\n    when: Ready(b)\n    requires: b.status = pending\n    ensures: b.status = done\n}\n",
        ),
        (
            "emission_in_branch",
            "surface Kicker {\n    provides:\n        Kick(j: Job, flag)\n}\n\nrule Announce {\n    when: Kick(j, flag)\n    if flag:\n        ensures: Ready(job: j)\n    else:\n        ensures: Ready(job: j)\n}\n",
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

    // Importer creates the imported entity inside an if/else branch. The domain
    // does not create it, so the importer's creation is the only assignment of
    // `pending`.
    let ci_domain = "entity Job {\n    status: pending | done\n    transitions status { pending -> done  terminal: done }\n}\n\nrule Advance {\n    when: t: Job.status becomes pending\n    ensures: t.status = done\n}\n";
    out.push(Scenario {
        name: "created_in_branch",
        single: format!(
            "-- allium: 3\n\n{ci_domain}\nrule Make {{\n    when: Go(flag)\n    if flag:\n        ensures: Job.created(status: pending)\n    else:\n        ensures: Job.created(status: pending)\n}}\n\nsurface Intake {{\n    provides:\n        Go(flag)\n}}\n"
        ),
        domain: format!("-- allium: 3\n\n{ci_domain}"),
        consumer: "-- allium: 3\n\nuse \"./domain.allium\" as dom\n\nrule Make {\n    when: Go(flag)\n    if flag:\n        ensures: dom/Job.created(status: pending)\n    else:\n        ensures: dom/Job.created(status: pending)\n}\n\nsurface Intake {\n    provides:\n        Go(flag)\n}\n".to_string(),
    });

    // Temporal trigger (`m: E.due_at <= now`) as the witnessing form. It needs a
    // Timestamp field, so it doesn't fit DOMAIN_BASE. The imported entity's
    // transition is witnessed by a time-based, not event-based, trigger.
    let tt_domain = "entity Job {\n    status: pending | done\n    due_at: Timestamp\n    transitions status { pending -> done  terminal: done }\n}\n\nrule CreateJob {\n    when: JobRequested()\n    ensures: Job.created(status: pending)\n}\n\nsurface JobIntake {\n    provides:\n        JobRequested()\n}\n";
    out.push(Scenario {
        name: "temporal_trigger",
        single: format!(
            "-- allium: 3\n\n{tt_domain}\nrule Witness {{\n    when: m: Job.due_at <= now\n    requires: m.status = pending\n    ensures: m.status = done\n}}\n"
        ),
        domain: format!("-- allium: 3\n\n{tt_domain}"),
        consumer: "-- allium: 3\n\nuse \"./domain.allium\" as dom\n\nrule Witness {\n    when: m: dom/Job.due_at <= now\n    requires: m.status = pending\n    ensures: m.status = done\n}\n".to_string(),
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

// ---------------------------------------------------------------------------
// Name-existence sweep: with a valid alias, a qualified reference to a name the
// aliased module does not declare (`dom/Ghost`) should be diagnosed. #72 does
// this for provides triggers and #47 for default fields; every other qualified
// entity/type reference site is the audit's next layer. Needs the domain in the
// check set, so it runs as pairs.
// ---------------------------------------------------------------------------

const NE_DOMAIN: &str = "-- allium: 3\n\nentity Job {\n    status: pending | done\n    transitions status { pending -> done  terminal: done }\n}\n\nrule CreateJob {\n    when: JobRequested()\n    ensures: Job.created(status: pending)\n}\n\nsurface JobIntake {\n    provides:\n        JobRequested()\n}\n";

fn pair_diagnostics_mentioning(consumer: &str, needle: &str) -> Vec<String> {
    let dir = TempDir::new("nameexist");
    dir.write("domain.allium", NE_DOMAIN);
    dir.write("consumer.allium", consumer);
    let out = run("check", &[dir.path().to_str().unwrap()]);
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
fn name_existence_sweep() {
    // `dom` is valid; `Ghost` is not an entity/type the domain declares.
    let head = "-- allium: 3\n\nuse \"./domain.allium\" as dom\n\n";
    let sites: Vec<(&str, String)> = vec![
        ("surface_context", format!("{head}surface S {{\n    context t: dom/Ghost\n    provides:\n        Ev(t)\n}}\n")),
        ("field_type", format!("{head}entity W {{\n    j: dom/Ghost\n}}\n")),
        ("ensures_created", format!("{head}rule R {{\n    when: Go()\n    ensures: dom/Ghost.created(status: pending)\n}}\n")),
        ("transition_subject", format!("{head}rule R {{\n    when: t: dom/Ghost.status becomes pending\n    ensures: t.status = done\n}}\n")),
        ("inline_provides_param", format!("{head}surface S {{\n    provides:\n        Ev(b: dom/Ghost)\n}}\n")),
    ];

    let mut uncaught: Vec<String> = Vec::new();
    for (site, snippet) in sites {
        if pair_diagnostics_mentioning(&snippet, "Ghost").is_empty() {
            uncaught.push(format!("[{site}] NO diagnostic names the nonexistent 'dom/Ghost'\n--- consumer ---\n{snippet}"));
        }
    }

    if !uncaught.is_empty() {
        panic!(
            "\n==== NAME EXISTENCE: {} sites do not diagnose a nonexistent qualified name ====\n\n{}\n",
            uncaught.len(),
            uncaught.join("\n\n")
        );
    }
}

// ---------------------------------------------------------------------------
// Generative split-invariance. The hand-written scenarios above enumerate the
// binding-type sources at fixed names; this generates random entity/state names
// and picks a witnessing form each seed, so the property (a valid witness is
// clean single-file, and the split reports exactly the same) is exercised over a
// far wider surface than the fixed cells. A false lifecycle report that only
// shows up for some name or form combination surfaces here.
// ---------------------------------------------------------------------------

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1))
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

struct SplitCase {
    single: String,
    domain: String,
    consumer: String,
    form: &'static str,
}

fn gen_split_case(seed: u64) -> SplitCase {
    let mut rng = Rng::new(seed);
    let e = format!("Ent{}", rng.below(1000));
    let s0 = format!("s{}a", rng.below(100));
    let s1 = format!("s{}b", rng.below(100));
    let form = rng.below(6);
    let temporal = form == 5;
    let field = if temporal { "    due_at: Timestamp\n" } else { "" };
    let entity = format!(
        "entity {e} {{\n    status: {s0} | {s1}\n{field}    transitions status {{ {s0} -> {s1}  terminal: {s1} }}\n}}\n"
    );
    let create = format!("rule Create{e} {{\n    when: {e}Req()\n    ensures: {e}.created(status: {s0})\n}}\n");
    let intake = format!("surface {e}Intake {{\n    provides:\n        {e}Req()\n}}\n");
    let domain_body = format!("{entity}\n{create}\n{intake}");

    // A type refinement is a no-op on which entity a binding refers to, so the
    // context/facing/inline forms carry a random one to also exercise refinement
    // unwrapping (the #76 family) generatively.
    let refine = match rng.below(3) {
        1 => format!(" where status = {s0}"),
        2 => format!(" with status = {s0}"),
        _ => String::new(),
    };
    let witness = |q: &str| -> String {
        match form {
            0 => format!("rule W {{\n    when: b: {q}{e}.status becomes {s0}\n    ensures: b.status = {s1}\n}}\n"),
            1 => format!("rule W {{\n    when: b: {q}{e}.status transitions_to {s0}\n    ensures: b.status = {s1}\n}}\n"),
            2 => format!("surface WDesk {{\n    context b: {q}{e}{refine}\n    provides:\n        Ready(b)\n            when b.status = {s0}\n}}\n\nrule W {{\n    when: Ready(z)\n    requires: z.status = {s0}\n    ensures: z.status = {s1}\n}}\n"),
            3 => format!("surface WDesk {{\n    facing b: {q}{e}{refine}\n    provides:\n        Ready(b)\n            when b.status = {s0}\n}}\n\nrule W {{\n    when: Ready(z)\n    requires: z.status = {s0}\n    ensures: z.status = {s1}\n}}\n"),
            4 => format!("surface WDesk {{\n    provides:\n        Ready(b: {q}{e}{refine})\n            when b.status = {s0}\n}}\n\nrule W {{\n    when: Ready(z)\n    requires: z.status = {s0}\n    ensures: z.status = {s1}\n}}\n"),
            _ => format!("rule W {{\n    when: m: {q}{e}.due_at <= now\n    requires: m.status = {s0}\n    ensures: m.status = {s1}\n}}\n"),
        }
    };
    let form_name = ["becomes", "transitions_to", "context", "facing", "inline", "temporal"][form as usize];

    SplitCase {
        single: format!("-- allium: 3\n\n{domain_body}\n{}", witness("")),
        domain: format!("-- allium: 3\n\n{domain_body}"),
        consumer: format!("-- allium: 3\n\nuse \"./domain.allium\" as dom\n\n{}", witness("dom/")),
        form: form_name,
    }
}

#[test]
fn generative_split_invariance() {
    let mut anomalies: Vec<String> = Vec::new();
    for seed in 0..90u64 {
        let c = gen_split_case(seed);
        let single = reports_of_file(&c.single);
        let split = reports_of_pair(&c.domain, &c.consumer);
        if !single.is_empty() {
            anomalies.push(format!(
                "[seed {seed} form={}] generated single-file witness is not clean: {single:?}\n{}",
                c.form, c.single
            ));
        } else if single != split {
            anomalies.push(format!(
                "[seed {seed} form={}] SPLIT != SINGLE\n  single: {single:?}\n  split:  {split:?}\n--- consumer ---\n{}",
                c.form, c.consumer
            ));
        }
    }
    assert!(
        anomalies.is_empty(),
        "\n==== GENERATIVE SPLIT-INVARIANCE: {} anomalies ====\n\n{}\n",
        anomalies.len(),
        anomalies.join("\n\n")
    );
}

// ---------------------------------------------------------------------------
// Multi-entity split-invariance. Several entities, each created in the domain and
// advanced by a witnessing rule in the consumer, exercise the reverse channel
// aggregating contributions for more than one entity at once. A per-entity key
// mix-up (crediting entity A's transition to entity B, say) shows up as a
// spurious lifecycle report on the split that the single file does not have.
// ---------------------------------------------------------------------------

fn gen_multi_split_case(seed: u64) -> (String, String, String) {
    let mut rng = Rng::new(seed);
    let n = 2 + rng.below(3); // 2..=4 entities
    let mut domain_body = String::new();
    let mut single_witness = String::new();
    let mut consumer_witness = String::new();
    for i in 0..n {
        let e = format!("Ent{i}x{}", rng.below(100));
        let s0 = format!("s{}a", rng.below(100));
        let s1 = format!("s{}b", rng.below(100));
        let trig = if rng.below(2) == 0 { "becomes" } else { "transitions_to" };
        domain_body.push_str(&format!(
            "entity {e} {{\n    status: {s0} | {s1}\n    transitions status {{ {s0} -> {s1}  terminal: {s1} }}\n}}\n\nrule Create{e} {{\n    when: {e}Req()\n    ensures: {e}.created(status: {s0})\n}}\n\nsurface {e}Intake {{\n    provides:\n        {e}Req()\n}}\n\n"
        ));
        single_witness.push_str(&format!(
            "rule W{i} {{\n    when: b: {e}.status {trig} {s0}\n    ensures: b.status = {s1}\n}}\n\n"
        ));
        consumer_witness.push_str(&format!(
            "rule W{i} {{\n    when: b: dom/{e}.status {trig} {s0}\n    ensures: b.status = {s1}\n}}\n\n"
        ));
    }
    let single = format!("-- allium: 3\n\n{domain_body}{single_witness}");
    let domain = format!("-- allium: 3\n\n{domain_body}");
    let consumer = format!("-- allium: 3\n\nuse \"./domain.allium\" as dom\n\n{consumer_witness}");
    (single, domain, consumer)
}

#[test]
fn generative_multi_entity_split_invariance() {
    let mut anomalies: Vec<String> = Vec::new();
    for seed in 0..70u64 {
        let (single_src, domain, consumer) = gen_multi_split_case(seed);
        let single = reports_of_file(&single_src);
        let split = reports_of_pair(&domain, &consumer);
        if !single.is_empty() {
            anomalies.push(format!("[seed {seed}] multi-entity single-file not clean: {single:?}\n{single_src}"));
        } else if single != split {
            anomalies.push(format!(
                "[seed {seed}] SPLIT != SINGLE\n  single: {single:?}\n  split:  {split:?}\n--- consumer ---\n{consumer}"
            ));
        }
    }
    assert!(
        anomalies.is_empty(),
        "\n==== MULTI-ENTITY SPLIT-INVARIANCE: {} anomalies ====\n\n{}\n",
        anomalies.len(),
        anomalies.join("\n\n")
    );
}
