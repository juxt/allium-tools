//! Cross-module reverse aggregation: an importer's qualified contributions to
//! an imported module's entities and triggers must be credited when both are
//! in the check set. Covers issues #62 (qualified creation), #63 (qualified
//! `provides:`), #64 (a transition witnessed across an import), and #65 (an
//! importer-owned trigger whose binding is typed to the imported entity by a
//! qualified surface context).
//!
//! The oracle throughout is the merged single-file control: the two-file pair
//! must produce exactly what the equivalent one-file spec produces. Analysing
//! the imported module alone must keep its local warnings — crediting requires
//! a real `use` import edge, not arbitrary co-supply.

use std::fs;
use std::path::Path;
use std::process::Command;

fn allium() -> Command {
    Command::new(env!("CARGO_BIN_EXE_allium"))
}

struct Diag {
    code: String,
    message: String,
}

struct Finding {
    kind: String,
    summary: String,
}

fn parse_diagnostics(stdout: &str) -> Vec<Diag> {
    let mut diags = Vec::new();
    for doc in split_json_docs(stdout) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&doc) {
            if let Some(arr) = v["diagnostics"].as_array() {
                for d in arr {
                    if let (Some(c), Some(m)) = (d["code"].as_str(), d["message"].as_str()) {
                        diags.push(Diag { code: c.to_string(), message: m.to_string() });
                    }
                }
            }
        }
    }
    diags
}

fn parse_findings(stdout: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    for doc in split_json_docs(stdout) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&doc) {
            if let Some(arr) = v["findings"].as_array() {
                for f in arr {
                    let kind = f["type"].as_str().unwrap_or_default().to_string();
                    let summary = f["summary"].as_str().unwrap_or_default().to_string();
                    findings.push(Finding { kind, summary });
                }
            }
        }
    }
    findings
}

/// Split concatenated pretty-printed JSON objects.
fn split_json_docs(s: &str) -> Vec<String> {
    let mut docs = Vec::new();
    let mut depth = 0i32;
    let mut start = None;
    let mut in_str = false;
    let mut escaped = false;
    for (i, ch) in s.char_indices() {
        if in_str {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
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
                    if let Some(s_idx) = start {
                        docs.push(s[s_idx..=i].to_string());
                    }
                    start = None;
                }
            }
            _ => {}
        }
    }
    docs
}

struct TempDir {
    path: std::path::PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("allium-xmod-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn write(&self, name: &str, content: &str) {
        fs::write(self.path.join(name), content).unwrap();
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn file(&self, name: &str) -> String {
        self.path.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn run(cmd: &str, args: &[&str]) -> (bool, String) {
    let output = allium().arg(cmd).args(args).output().expect("spawn allium");
    (output.status.success(), String::from_utf8_lossy(&output.stdout).into_owned())
}

// ===========================================================================
// Repro sources, transcribed from the issues.
// ===========================================================================

// --- #62: qualified creation ------------------------------------------------

const TICKET_62: &str = r#"-- allium: 3

entity Ticket {
    status: open | closed
}

rule CloseOpenTicket {
    when: CloseTicket(ticket)
    requires: ticket.status = open
    ensures: ticket.status = closed
}

surface TicketWorklist {
    context ticket: Ticket

    exposes:
        ticket.status

    provides:
        CloseTicket(ticket)
            when ticket.status = open
}
"#;

const CONSOLE_62: &str = r#"-- allium: 3

use "./ticket.allium" as tickets

rule CreateTicket {
    when: OpenTicketRequested()

    ensures: tickets/Ticket.created(
        status: open
    )
}

surface TicketIntake {
    provides:
        OpenTicketRequested()
}
"#;

const MERGED_62: &str = r#"-- allium: 3

entity Ticket {
    status: open | closed
}

rule CloseOpenTicket {
    when: CloseTicket(ticket)
    requires: ticket.status = open
    ensures: ticket.status = closed
}

surface TicketWorklist {
    context ticket: Ticket

    exposes:
        ticket.status

    provides:
        CloseTicket(ticket)
            when ticket.status = open
}

rule CreateTicket {
    when: OpenTicketRequested()
    ensures: Ticket.created(status: open)
}

surface TicketIntake {
    provides:
        OpenTicketRequested()
}
"#;

// --- #63: qualified provides ------------------------------------------------

const TICKET_63: &str = r#"-- allium: 3

entity Ticket {
    status: open | closed

    transitions status {
        open -> closed
        terminal: closed
    }
}

rule CreateTicket {
    when: OpenTicket()

    ensures: Ticket.created(
        status: open
    )
}

rule CloseOpenTicket {
    when: CloseTicket(ticket)

    requires: ticket.status = open

    ensures: ticket.status = closed
}
"#;

const CONSOLE_63: &str = r#"-- allium: 3

use "./ticket.allium" as tickets

surface TicketIntake {
    provides:
        tickets/OpenTicket()
        tickets/CloseTicket(ticket)
}
"#;

const MERGED_63: &str = r#"-- allium: 3

entity Ticket {
    status: open | closed

    transitions status {
        open -> closed
        terminal: closed
    }
}

rule CreateTicket {
    when: OpenTicket()
    ensures: Ticket.created(status: open)
}

rule CloseOpenTicket {
    when: CloseTicket(ticket)
    requires: ticket.status = open
    ensures: ticket.status = closed
}

surface TicketIntake {
    provides:
        OpenTicket()
        CloseTicket(ticket)
}
"#;

// --- #64: transition witnessed across an import -----------------------------

const TICKET_64: &str = r#"-- allium: 3

entity Ticket {
    status: closed | archived

    transitions status {
        closed -> archived
        terminal: archived
    }
}

rule CreateTicket {
    when: CreateTicketRequested()

    ensures: Ticket.created(
        status: closed
    )
}

surface TicketDesk {
    provides:
        CreateTicketRequested()
        ArchiveTicketRequested(ticket: Ticket)
            when ticket.status = closed
}
"#;

const CONSOLE_64: &str = r#"-- allium: 3

use "./ticket.allium" as tickets

rule ArchiveClosedTicket {
    when: tickets/ArchiveTicketRequested(ticket)

    requires: ticket.status = closed

    ensures: ticket.status = archived
}
"#;

const MERGED_64: &str = r#"-- allium: 3

entity Ticket {
    status: closed | archived

    transitions status {
        closed -> archived
        terminal: archived
    }
}

rule CreateTicket {
    when: CreateTicketRequested()
    ensures: Ticket.created(status: closed)
}

rule ArchiveClosedTicket {
    when: ArchiveTicketRequested(ticket)
    requires: ticket.status = closed
    ensures: ticket.status = archived
}

surface TicketDesk {
    provides:
        CreateTicketRequested()
        ArchiveTicketRequested(ticket: Ticket)
            when ticket.status = closed
}
"#;

// ===========================================================================
// #62 — qualified imported-entity creation credited to status lifecycle
// ===========================================================================

#[test]
fn t62_qualified_creation_credits_status_in_pair() {
    let dir = TempDir::new("62-pair");
    dir.write("ticket.allium", TICKET_62);
    dir.write("operator-console.allium", CONSOLE_62);

    let (ok, stdout) = run("check", &[dir.path().to_str().unwrap()]);
    let unreachable: Vec<_> = parse_diagnostics(&stdout)
        .into_iter()
        .filter(|d| d.code == "allium.status.unreachableValue" && d.message.contains("open"))
        .collect();

    assert!(
        unreachable.is_empty(),
        "console creates Ticket with status open — 'open' must be reachable in the pair.\nDiagnostics: {:?}",
        unreachable.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
    assert!(ok, "check on the pair should exit 0 once the creation is credited.\n{stdout}");
}

#[test]
fn t62_status_still_unreachable_when_ticket_checked_alone() {
    let dir = TempDir::new("62-alone");
    dir.write("ticket.allium", TICKET_62);

    let (_ok, stdout) = run("check", &[&dir.file("ticket.allium")]);
    assert!(
        parse_diagnostics(&stdout)
            .iter()
            .any(|d| d.code == "allium.status.unreachableValue" && d.message.contains("open")),
        "checked alone, ticket.allium has no assignment of 'open' and must still warn.\n{stdout}"
    );
}

#[test]
fn t62_merged_control_is_clean() {
    let dir = TempDir::new("62-merged");
    dir.write("merged.allium", MERGED_62);

    let (ok, stdout) = run("check", &[&dir.file("merged.allium")]);
    assert!(
        !parse_diagnostics(&stdout)
            .iter()
            .any(|d| d.code == "allium.status.unreachableValue"),
        "merged control must be clean (the oracle).\n{stdout}"
    );
    assert!(ok, "merged control check should exit 0.\n{stdout}");
}

// ===========================================================================
// #63 — qualified `provides: alias/Trigger` credited to trigger reachability
// ===========================================================================

#[test]
fn t63_qualified_provides_credits_triggers_in_pair() {
    let dir = TempDir::new("63-pair");
    dir.write("ticket.allium", TICKET_63);
    dir.write("operator-console.allium", CONSOLE_63);

    let (ok, stdout) = run("analyse", &[dir.path().to_str().unwrap()]);

    let unreachable: Vec<_> = parse_diagnostics(&stdout)
        .into_iter()
        .filter(|d| d.code == "allium.rule.unreachableTrigger")
        .collect();
    assert!(
        unreachable.is_empty(),
        "the console provides both triggers by qualified name — no rule should be unreachable.\nDiagnostics: {:?}",
        unreachable.iter().map(|d| &d.message).collect::<Vec<_>>()
    );

    let findings: Vec<_> = parse_findings(&stdout)
        .into_iter()
        .filter(|f| f.kind == "unreachable_trigger")
        .collect();
    assert!(
        findings.is_empty(),
        "no unreachable_trigger findings expected in the pair.\nFindings: {:?}",
        findings.iter().map(|f| &f.summary).collect::<Vec<_>>()
    );
    assert!(ok, "analyse on the pair should exit 0 once the provides are credited.\n{stdout}");
}

#[test]
fn t63_triggers_unreachable_when_ticket_analysed_alone() {
    let dir = TempDir::new("63-alone");
    dir.write("ticket.allium", TICKET_63);

    let (_ok, stdout) = run("analyse", &[&dir.file("ticket.allium")]);
    assert!(
        parse_diagnostics(&stdout)
            .iter()
            .any(|d| d.code == "allium.rule.unreachableTrigger"),
        "analysed alone, ticket.allium's rules have no local provider and must warn.\n{stdout}"
    );
}

#[test]
fn t63_merged_control_is_clean() {
    let dir = TempDir::new("63-merged");
    dir.write("merged.allium", MERGED_63);

    let (ok, stdout) = run("analyse", &[&dir.file("merged.allium")]);
    assert!(
        !parse_diagnostics(&stdout)
            .iter()
            .any(|d| d.code == "allium.rule.unreachableTrigger"),
        "merged control must be clean (the oracle).\n{stdout}"
    );
    assert!(
        !parse_findings(&stdout).iter().any(|f| f.kind == "unreachable_trigger"),
        "merged control must have no unreachable_trigger findings.\n{stdout}"
    );
    assert!(ok, "merged control analyse should exit 0.\n{stdout}");
}

// The same witness expressed with the `becomes` transition-trigger form, which
// #64 records as failing identically. Same semantics, different trigger syntax.
const CONSOLE_64_BECOMES: &str = r#"-- allium: 3

use "./ticket.allium" as tickets

rule ArchiveClosedTicket {
    when: t: tickets/Ticket.status becomes closed
    ensures: t.status = archived
}
"#;

// ===========================================================================
// #64 — a declared transition witnessed by an importing rule
// ===========================================================================

#[test]
fn t64_qualified_witness_credits_transition_no_deadlock() {
    let dir = TempDir::new("64-pair");
    dir.write("ticket.allium", TICKET_64);
    dir.write("operator-console.allium", CONSOLE_64);

    let (analyse_ok, analyse_out) = run("analyse", &[dir.path().to_str().unwrap()]);
    assert!(
        !parse_findings(&analyse_out).iter().any(|f| f.kind == "deadlock"),
        "the console witnesses closed -> archived — no deadlock should be reported.\nFindings: {:?}",
        parse_findings(&analyse_out).iter().map(|f| f.summary.clone()).collect::<Vec<_>>()
    );
    assert!(analyse_ok, "analyse on the pair should exit 0.\n{analyse_out}");

    let (check_ok, check_out) = run("check", &[dir.path().to_str().unwrap()]);
    let diags = parse_diagnostics(&check_out);
    assert!(
        !diags.iter().any(|d| d.code == "allium.status.noExit" && d.message.contains("closed")),
        "closed has a witnessed exit to archived — no noExit expected.\n{check_out}"
    );
    assert!(
        !diags.iter().any(|d| d.code == "allium.status.unreachableValue" && d.message.contains("archived")),
        "archived is assigned by the witnessing rule — not unreachable.\n{check_out}"
    );
    assert!(check_ok, "check on the pair should exit 0.\n{check_out}");
}

#[test]
fn t64_becomes_form_witness_also_credits_transition() {
    let dir = TempDir::new("64-becomes");
    dir.write("ticket.allium", TICKET_64);
    dir.write("operator-console.allium", CONSOLE_64_BECOMES);

    let (analyse_ok, analyse_out) = run("analyse", &[dir.path().to_str().unwrap()]);
    assert!(
        !parse_findings(&analyse_out).iter().any(|f| f.kind == "deadlock"),
        "the becomes-form witness must credit closed -> archived just as the subscription form does.\nFindings: {:?}",
        parse_findings(&analyse_out).iter().map(|f| f.summary.clone()).collect::<Vec<_>>()
    );
    assert!(analyse_ok, "analyse on the becomes-form pair should exit 0.\n{analyse_out}");

    let (check_ok, check_out) = run("check", &[dir.path().to_str().unwrap()]);
    assert!(check_ok, "check on the becomes-form pair should exit 0.\n{check_out}");
}

#[test]
fn t64_deadlock_when_ticket_analysed_alone() {
    let dir = TempDir::new("64-alone");
    dir.write("ticket.allium", TICKET_64);

    let (_ok, stdout) = run("analyse", &[&dir.file("ticket.allium")]);
    assert!(
        parse_findings(&stdout).iter().any(|f| f.kind == "deadlock"),
        "analysed alone, ticket.allium has no witness for closed -> archived and must deadlock.\n{stdout}"
    );
}

#[test]
fn t64_merged_control_is_clean() {
    let dir = TempDir::new("64-merged");
    dir.write("merged.allium", MERGED_64);

    let (ok, stdout) = run("analyse", &[&dir.file("merged.allium")]);
    assert!(
        !parse_findings(&stdout).iter().any(|f| f.kind == "deadlock"),
        "merged control must have no deadlock (the oracle).\n{stdout}"
    );
    assert!(ok, "merged control analyse should exit 0.\n{stdout}");
}

// ===========================================================================
// #65 — importer owns the trigger; binding typed via a qualified surface context
// ===========================================================================

const TICKET_65: &str = r#"-- allium: 3

entity Ticket {
    status: closed | archived

    transitions status {
        closed -> archived
        terminal: archived
    }
}

rule CreateTicket {
    when: CreateTicketRequested()
    ensures: Ticket.created(status: closed)
}

surface TicketDesk {
    provides:
        CreateTicketRequested()
}
"#;

const CONSOLE_65: &str = r#"-- allium: 3

use "./ticket.allium" as tickets

surface ArchiveDesk {
    context t: tickets/Ticket
    provides:
        ArchiveTicketRequested(t)
            when t.status = closed
}

rule ArchiveClosedTicket {
    when: ArchiveTicketRequested(ticket)
    requires: ticket.status = closed
    ensures: ticket.status = archived
}
"#;

#[test]
fn t65_importer_owned_trigger_credits_transition() {
    let dir = TempDir::new("65-pair");
    dir.write("ticket.allium", TICKET_65);
    dir.write("operator-console.allium", CONSOLE_65);

    let (analyse_ok, analyse_out) = run("analyse", &[dir.path().to_str().unwrap()]);
    assert!(
        !parse_findings(&analyse_out).iter().any(|f| f.kind == "deadlock"),
        "the console witnesses closed -> archived on tickets/Ticket — no deadlock.\nFindings: {:?}",
        parse_findings(&analyse_out).iter().map(|f| f.summary.clone()).collect::<Vec<_>>()
    );
    assert!(analyse_ok, "analyse on the pair should exit 0.\n{analyse_out}");

    let (check_ok, check_out) = run("check", &[dir.path().to_str().unwrap()]);
    let diags = parse_diagnostics(&check_out);
    assert!(
        !diags.iter().any(|d| d.code == "allium.status.noExit" && d.message.contains("closed")),
        "closed has a witnessed exit — no noExit.\n{check_out}"
    );
    assert!(
        !diags.iter().any(|d| d.code == "allium.status.unreachableValue" && d.message.contains("archived")),
        "archived is assigned by the witnessing rule — not unreachable.\n{check_out}"
    );
    assert!(check_ok, "check on the pair should exit 0.\n{check_out}");
}

#[test]
fn t65_deadlock_when_ticket_analysed_alone() {
    let dir = TempDir::new("65-alone");
    dir.write("ticket.allium", TICKET_65);

    let (_ok, stdout) = run("analyse", &[&dir.file("ticket.allium")]);
    assert!(
        parse_findings(&stdout).iter().any(|f| f.kind == "deadlock"),
        "analysed alone, ticket.allium has no witness for closed -> archived and must deadlock.\n{stdout}"
    );
}

// ===========================================================================
// #70 — split-invariance: a spec as one file produces the same reports as the
// same spec split across a `use` edge. The transition-trigger start state must
// be credited locally, not only across the module boundary. This is the #66
// oracle I asserted in prose but never tested, which is how the regression got
// in.
// ===========================================================================

const T70_SINGLE: &str = r#"-- allium: 3

entity Ticket {
    status: closed | archived
    transitions status { closed -> archived  terminal: archived }
}

rule CreateTicket {
    when: CreateTicketRequested()
    ensures: Ticket.created(status: closed)
}

rule ArchiveClosedTicket {
    when: t: Ticket.status becomes closed
    ensures: t.status = archived
}

surface TicketDesk {
    provides:
        CreateTicketRequested()
}
"#;

const T70_DOMAIN: &str = r#"-- allium: 3

entity Ticket {
    status: closed | archived
    transitions status { closed -> archived  terminal: archived }
}

rule CreateTicket {
    when: CreateTicketRequested()
    ensures: Ticket.created(status: closed)
}

surface TicketDesk {
    provides:
        CreateTicketRequested()
}
"#;

const T70_CONSOLE: &str = r#"-- allium: 3

use "./ticket.allium" as tickets

rule ArchiveClosedTicket {
    when: t: tickets/Ticket.status becomes closed
    ensures: t.status = archived
}
"#;

/// A comparable report set (code + message for diagnostics, kind + summary for
/// findings), independent of which file a report lands in.
fn report_set(stdout: &str) -> Vec<String> {
    let mut rows: Vec<String> = parse_diagnostics(stdout)
        .into_iter()
        .map(|d| format!("D {} {}", d.code, d.message))
        .collect();
    rows.extend(
        parse_findings(stdout)
            .into_iter()
            .map(|f| format!("F {} {}", f.kind, f.summary)),
    );
    rows.sort();
    rows
}

#[test]
fn t70_single_file_matches_split_for_becomes_trigger() {
    let single = TempDir::new("70-single");
    single.write("ticket.allium", T70_SINGLE);
    let split = TempDir::new("70-split");
    split.write("ticket.allium", T70_DOMAIN);
    split.write("operator-console.allium", T70_CONSOLE);

    for cmd in ["check", "analyse"] {
        let (_o1, single_out) = run(cmd, &[&single.file("ticket.allium")]);
        let (_o2, split_out) = run(cmd, &[split.path().to_str().unwrap()]);
        assert_eq!(
            report_set(&single_out),
            report_set(&split_out),
            "{cmd}: the one-file spec and its split must produce the same reports.\nsingle: {:?}\nsplit: {:?}",
            report_set(&single_out),
            report_set(&split_out)
        );
    }
}

// A generator-driven version of the split-invariance oracle, so it covers the
// matrix (both trigger forms, varied entity/state names) rather than one cell.
// Seeded SplitMix64, pure std. The CLI is spawned per case, so the seed count is
// modest.

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
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// One logical spec in two shapes: as a single file, and split into a domain
/// module plus a console module that owns the transition-trigger rule and
/// refers to the entity by qualified name.
fn gen_single_and_split(seed: u64) -> (String, String, String) {
    let mut r = Rng::new(seed);
    let name = format!("E{}", r.below(10000));
    let s0 = format!("a{}", r.below(1000));
    let s1 = format!("b{}", r.below(1000));
    let trig = if r.below(2) == 0 { "becomes" } else { "transitions_to" };

    let entity = format!(
        "entity {name} {{\n    status: {s0} | {s1}\n    transitions status {{ {s0} -> {s1}  terminal: {s1} }}\n}}\n"
    );
    let create = format!(
        "rule Create{name} {{\n    when: Create{name}Requested()\n    ensures: {name}.created(status: {s0})\n}}\n"
    );
    let surface = format!(
        "surface {name}Desk {{\n    provides:\n        Create{name}Requested()\n}}\n"
    );
    let advance_local = format!(
        "rule Advance{name} {{\n    when: t: {name}.status {trig} {s0}\n    ensures: t.status = {s1}\n}}\n"
    );
    let advance_qualified = format!(
        "rule Advance{name} {{\n    when: t: dom/{name}.status {trig} {s0}\n    ensures: t.status = {s1}\n}}\n"
    );

    let single = format!("-- allium: 3\n\n{entity}\n{create}\n{advance_local}\n{surface}");
    let domain = format!("-- allium: 3\n\n{entity}\n{create}\n{surface}");
    let console = format!("-- allium: 3\n\nuse \"./domain.allium\" as dom\n\n{advance_qualified}");
    (single, domain, console)
}

#[test]
fn prop_single_file_matches_split_for_transition_trigger() {
    for seed in 0..16u64 {
        let (single_src, domain_src, console_src) = gen_single_and_split(seed);
        let sdir = TempDir::new(&format!("splitinv-s{seed}"));
        sdir.write("spec.allium", &single_src);
        let pdir = TempDir::new(&format!("splitinv-p{seed}"));
        pdir.write("domain.allium", &domain_src);
        pdir.write("console.allium", &console_src);

        for cmd in ["check", "analyse"] {
            let (_a, single_out) = run(cmd, &[&sdir.file("spec.allium")]);
            let (_b, split_out) = run(cmd, &[pdir.path().to_str().unwrap()]);
            assert_eq!(
                report_set(&single_out),
                report_set(&split_out),
                "seed {seed} {cmd}: one-file spec and its split diverge.\nSINGLE:\n{single_src}\n-> {:?}\n\nSPLIT domain:\n{domain_src}\nconsole:\n{console_src}\n-> {:?}",
                report_set(&single_out),
                report_set(&split_out)
            );
        }
    }
}

// ===========================================================================
// #72 — an unresolvable qualified provides entry is diagnosed at the entry,
// not only as a misleading downstream unreachableTrigger on the imported module.
// ===========================================================================

const TICKET_72: &str = r#"-- allium: 3

entity Ticket {
    status: open | closed
    transitions status { open -> closed  terminal: closed }
}

rule CreateTicket {
    when: OpenTicket()
    ensures: Ticket.created(status: open)
}

rule CloseOpenTicket {
    when: CloseTicket(ticket)
    requires: ticket.status = open
    ensures: ticket.status = closed
}
"#;

const CONSOLE_72_BAD_ALIAS: &str = r#"-- allium: 3

use "./ticket.allium" as tickets

surface TicketIntake {
    provides:
        tickets/OpenTicket()
        nosuch/CloseTicket(ticket)
}
"#;

const CONSOLE_72_BAD_TRIGGER: &str = r#"-- allium: 3

use "./ticket.allium" as tickets

surface TicketIntake {
    provides:
        tickets/OpenTicket()
        tickets/AbsentTrigger()
}
"#;

const CONSOLE_72_OK: &str = r#"-- allium: 3

use "./ticket.allium" as tickets

surface TicketIntake {
    provides:
        tickets/OpenTicket()
        tickets/CloseTicket(ticket)
}
"#;

#[test]
fn t72_unknown_provides_alias_diagnosed_at_entry() {
    let dir = TempDir::new("72-bad-alias");
    dir.write("ticket.allium", TICKET_72);
    dir.write("console.allium", CONSOLE_72_BAD_ALIAS);

    let (_ok, out) = run("check", &[dir.path().to_str().unwrap()]);
    let diags = parse_diagnostics(&out);
    assert!(
        diags.iter().any(|d| d.message.contains("nosuch")),
        "the unresolvable provides alias 'nosuch' must be diagnosed at the entry. Diags: {:?}",
        diags.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
}

#[test]
fn t72_unknown_provides_trigger_diagnosed_at_entry() {
    let dir = TempDir::new("72-bad-trigger");
    dir.write("ticket.allium", TICKET_72);
    dir.write("console.allium", CONSOLE_72_BAD_TRIGGER);

    let (_ok, out) = run("check", &[dir.path().to_str().unwrap()]);
    let diags = parse_diagnostics(&out);
    assert!(
        diags.iter().any(|d| d.message.contains("AbsentTrigger")),
        "the unknown provides trigger 'AbsentTrigger' must be diagnosed at the entry. Diags: {:?}",
        diags.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
}

#[test]
fn t72_well_formed_provides_draws_no_resolution_diagnostic() {
    let dir = TempDir::new("72-ok");
    dir.write("ticket.allium", TICKET_72);
    dir.write("console.allium", CONSOLE_72_OK);

    let (_ok, out) = run("check", &[dir.path().to_str().unwrap()]);
    let diags = parse_diagnostics(&out);
    assert!(
        !diags.iter().any(|d| d.code.contains("undefinedImportedAlias")
            || d.code.contains("unknownTrigger")),
        "well-formed qualified provides must not draw resolution diagnostics. Diags: {:?}",
        diags.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
    // And it stays credited: no rule reads unreachable.
    assert!(
        !diags.iter().any(|d| d.code == "allium.rule.unreachableTrigger"),
        "well-formed provides must keep the domain's rules reachable. Diags: {:?}",
        diags.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
}

// Anchoring property: corrupting one provides entry (its alias or its trigger
// name) draws a resolution diagnostic naming the bad token, which the
// well-formed variant does not. Generator-driven so it covers varied names.

fn gen_provides_case(seed: u64) -> (String, String, String, String, String, String) {
    let mut r = Rng::new(seed);
    let e = format!("E{}", r.below(10000));
    let t1 = format!("T{}a", r.below(10000));
    let t2 = format!("T{}b", r.below(10000));
    let alias = format!("dom{}", r.below(1000));
    let absent = format!("Absent{}", r.below(10000));

    let domain = format!(
        "-- allium: 3\n\nentity {e} {{\n    status: open | closed\n    transitions status {{ open -> closed  terminal: closed }}\n}}\n\nrule R1 {{\n    when: {t1}()\n    ensures: {e}.created(status: open)\n}}\n\nrule R2 {{\n    when: {t2}(x)\n    requires: x.status = open\n    ensures: x.status = closed\n}}\n"
    );
    let head = format!("-- allium: 3\n\nuse \"./domain.allium\" as {alias}\n\nsurface S {{\n    provides:\n");
    let ok = format!("{head}        {alias}/{t1}()\n        {alias}/{t2}(x)\n}}\n");
    let bad_alias = format!("{head}        {alias}/{t1}()\n        nosuch/{t2}(x)\n}}\n");
    let bad_trigger = format!("{head}        {alias}/{t1}()\n        {alias}/{absent}()\n}}\n");
    (domain, ok, bad_alias, bad_trigger, t2, absent)
}

#[test]
fn prop_malformed_provides_entry_is_anchored() {
    for seed in 0..12u64 {
        let (domain, ok, bad_alias, bad_trigger, _t2, absent) = gen_provides_case(seed);

        let run_case = |label: &str, console: &str| -> Vec<Diag> {
            let dir = TempDir::new(&format!("anchor-{label}-{seed}"));
            dir.write("domain.allium", &domain);
            dir.write("console.allium", console);
            let (_ok, out) = run("check", &[dir.path().to_str().unwrap()]);
            parse_diagnostics(&out)
        };

        let ok_diags = run_case("ok", &ok);
        assert!(
            !ok_diags.iter().any(|d| d.code == "allium.provides.undefinedImportedAlias"
                || d.code == "allium.provides.unknownTrigger"),
            "seed {seed}: well-formed provides drew a resolution diagnostic.\n{:?}",
            ok_diags.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
        );

        let alias_diags = run_case("badalias", &bad_alias);
        assert!(
            alias_diags.iter().any(|d| d.code == "allium.provides.undefinedImportedAlias"
                && d.message.contains("nosuch")),
            "seed {seed}: a bad provides alias must be anchored.\n{:?}",
            alias_diags.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
        );

        let trigger_diags = run_case("badtrigger", &bad_trigger);
        assert!(
            trigger_diags.iter().any(|d| d.code == "allium.provides.unknownTrigger"
                && d.message.contains(&absent)),
            "seed {seed}: a bad provides trigger must be anchored.\n{:?}",
            trigger_diags.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
        );
    }
}

// ===========================================================================
// Gate: crediting requires a real import edge, not arbitrary co-supply
// ===========================================================================

#[test]
fn gate_no_import_edge_means_no_credit() {
    // `unrelated.allium` creates a `Ticket` by bare name but does NOT `use`
    // the domain module. The two files share a directory but no import edge,
    // so the domain's `open` must remain unreachable.
    let dir = TempDir::new("gate-no-edge");
    dir.write("ticket.allium", TICKET_62);
    dir.write(
        "unrelated.allium",
        "-- allium: 3\n\nrule Make {\n    when: Go()\n    ensures: Ticket.created(status: open)\n}\n",
    );

    let (_ok, stdout) = run("check", &[dir.path().to_str().unwrap()]);
    assert!(
        parse_diagnostics(&stdout)
            .iter()
            .any(|d| d.code == "allium.status.unreachableValue" && d.message.contains("open")),
        "without a `use` edge, a co-supplied file's creation must not credit the domain.\n{stdout}"
    );
}
