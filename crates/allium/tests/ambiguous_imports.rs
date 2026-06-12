//! End-to-end tests for `allium.use.ambiguousReference` (issue #15): warn
//! when an unqualified reference could resolve to more than one imported
//! module.

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

/// Parse the JSON output from `allium check` and return all diagnostics with
/// their code and message.
fn parse_diagnostics(stdout: &str) -> Vec<Diag> {
    let mut diags = Vec::new();
    for doc in split_json_docs(stdout) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&doc) {
            if let Some(arr) = v["diagnostics"].as_array() {
                for d in arr {
                    if let (Some(c), Some(m)) = (d["code"].as_str(), d["message"].as_str()) {
                        diags.push(Diag {
                            code: c.to_string(),
                            message: m.to_string(),
                        });
                    }
                }
            }
        }
    }
    diags
}

fn ambiguity_warnings(stdout: &str) -> Vec<Diag> {
    parse_diagnostics(stdout)
        .into_iter()
        .filter(|d| d.code == "allium.use.ambiguousReference")
        .collect()
}

/// Split concatenated pretty-printed JSON objects.
fn split_json_docs(s: &str) -> Vec<String> {
    let mut docs = Vec::new();
    let mut depth = 0i32;
    let mut start = None;
    for (i, ch) in s.char_indices() {
        match ch {
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
        let path = std::env::temp_dir().join(format!("allium-test-{name}-{}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn write(&self, name: &str, content: &str) {
        fs::write(self.path.join(name), content).unwrap();
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

fn check(dir: &TempDir) -> String {
    let output = allium()
        .args(["check", dir.path().to_str().unwrap()])
        .output()
        .expect("spawn allium");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

const ORDERS_INVOICE: &str = "-- allium: 3\nentity Invoice {\n  processed: Boolean\n}\n";
const BILLING_INVOICE: &str = "-- allium: 3\nentity Invoice {\n  processed: Boolean\n}\n";

// -----------------------------------------------------------------------
// Entity declarations in multiple imports
// -----------------------------------------------------------------------

#[test]
fn ambiguous_entity_reference_warns() {
    let dir = TempDir::new("ambiguous-entity");
    dir.write("orders.allium", ORDERS_INVOICE);
    dir.write("billing.allium", BILLING_INVOICE);
    dir.write(
        "main.allium",
        "-- allium: 3\nuse \"./orders.allium\" as orders\nuse \"./billing.allium\" as billing\n\nrule Process {\n  when: i: Invoice.created()\n  ensures: i.processed = true\n}\n",
    );

    let warnings = ambiguity_warnings(&check(&dir));
    assert_eq!(
        warnings.len(),
        1,
        "expected one ambiguity warning for 'Invoice'"
    );
    let msg = &warnings[0].message;
    assert!(msg.contains("'Invoice'"), "message: {msg}");
    assert!(msg.contains("'billing' and 'orders'"), "message: {msg}");
    assert!(msg.contains("billing/Invoice"), "message: {msg}");
}

#[test]
fn qualified_reference_not_ambiguous() {
    let dir = TempDir::new("qualified-ref");
    dir.write("orders.allium", ORDERS_INVOICE);
    dir.write("billing.allium", BILLING_INVOICE);
    dir.write(
        "main.allium",
        "-- allium: 3\nuse \"./orders.allium\" as orders\nuse \"./billing.allium\" as billing\n\nrule Process {\n  when: i: orders/Invoice.created()\n  ensures: i.processed = true\n}\n",
    );

    let warnings = ambiguity_warnings(&check(&dir));
    assert!(
        warnings.is_empty(),
        "qualified reference should not warn: {:?}",
        warnings.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

#[test]
fn local_declaration_shadows_imports() {
    let dir = TempDir::new("local-shadow");
    dir.write("orders.allium", ORDERS_INVOICE);
    dir.write("billing.allium", BILLING_INVOICE);
    dir.write(
        "main.allium",
        "-- allium: 3\nuse \"./orders.allium\" as orders\nuse \"./billing.allium\" as billing\n\nentity Invoice {\n  processed: Boolean\n}\n\nrule Process {\n  when: i: Invoice.created()\n  ensures: i.processed = true\n}\n",
    );

    let warnings = ambiguity_warnings(&check(&dir));
    assert!(
        warnings.is_empty(),
        "local declaration shadows imports: {:?}",
        warnings.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

#[test]
fn single_import_declaring_name_not_ambiguous() {
    let dir = TempDir::new("single-import");
    dir.write("orders.allium", ORDERS_INVOICE);
    dir.write(
        "billing.allium",
        "-- allium: 3\nentity Receipt {\n  total: Decimal\n}\n",
    );
    dir.write(
        "main.allium",
        "-- allium: 3\nuse \"./orders.allium\" as orders\nuse \"./billing.allium\" as billing\n\nrule Process {\n  when: i: Invoice.created()\n  ensures: i.processed = true\n}\n",
    );

    let warnings = ambiguity_warnings(&check(&dir));
    assert!(
        warnings.is_empty(),
        "one declaring import is not ambiguous: {:?}",
        warnings.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

#[test]
fn two_aliases_for_same_file_not_ambiguous() {
    let dir = TempDir::new("same-target");
    dir.write("orders.allium", ORDERS_INVOICE);
    dir.write(
        "main.allium",
        "-- allium: 3\nuse \"./orders.allium\" as orders\nuse \"./orders.allium\" as billing\n\nrule Process {\n  when: i: Invoice.created()\n  ensures: i.processed = true\n}\n",
    );

    let warnings = ambiguity_warnings(&check(&dir));
    assert!(
        warnings.is_empty(),
        "two aliases for one file resolve identically: {:?}",
        warnings.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

// -----------------------------------------------------------------------
// Trigger emissions in multiple imports
// -----------------------------------------------------------------------

#[test]
fn ambiguous_trigger_subscription_warns() {
    let dir = TempDir::new("ambiguous-trigger");
    dir.write(
        "a.allium",
        "-- allium: 3\nrule EmitA {\n  when: StartA(x)\n  ensures: Pinged(subject: x)\n}\n",
    );
    dir.write(
        "b.allium",
        "-- allium: 3\nrule EmitB {\n  when: StartB(x)\n  ensures: Pinged(subject: x)\n}\n",
    );
    dir.write(
        "main.allium",
        "-- allium: 3\nuse \"./a.allium\" as a\nuse \"./b.allium\" as b\n\nrule HandlePing {\n  when: Pinged(subject)\n  ensures: PingHandled(subject: subject)\n}\n",
    );

    let warnings = ambiguity_warnings(&check(&dir));
    assert_eq!(
        warnings.len(),
        1,
        "expected one ambiguity warning for trigger 'Pinged'"
    );
    let msg = &warnings[0].message;
    assert!(msg.contains("'Pinged'"), "message: {msg}");
    assert!(msg.contains("'a' and 'b'"), "message: {msg}");
    assert!(msg.contains("a/Pinged"), "message: {msg}");
}

#[test]
fn qualified_trigger_subscription_not_ambiguous() {
    let dir = TempDir::new("qualified-trigger");
    dir.write(
        "a.allium",
        "-- allium: 3\nrule EmitA {\n  when: StartA(x)\n  ensures: Pinged(subject: x)\n}\n",
    );
    dir.write(
        "b.allium",
        "-- allium: 3\nrule EmitB {\n  when: StartB(x)\n  ensures: Pinged(subject: x)\n}\n",
    );
    dir.write(
        "main.allium",
        "-- allium: 3\nuse \"./a.allium\" as a\nuse \"./b.allium\" as b\n\nrule HandlePing {\n  when: a/Pinged(subject)\n  ensures: PingHandled(subject: subject)\n}\n",
    );

    let warnings = ambiguity_warnings(&check(&dir));
    assert!(
        warnings.is_empty(),
        "qualified subscription should not warn: {:?}",
        warnings.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}
