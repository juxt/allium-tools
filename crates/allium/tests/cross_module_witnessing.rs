//! End-to-end: `allium analyse` credits a transition witnessed by a rule in
//! another module, so a lifecycle that spans modules is not reported as a false
//! deadlock / dead end. Exercises the whole pipeline (trigger-payload +
//! witnessed-transition collectors, cross-module crediting in main, and the
//! reachability/status-machine consumers) through the real binary.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn allium() -> Command {
    Command::new(env!("CARGO_BIN_EXE_allium"))
}

/// Split concatenated pretty-printed JSON objects (one per analysed file).
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

fn finding_types(stdout: &str) -> Vec<String> {
    let mut out = Vec::new();
    for doc in split_json_docs(stdout) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&doc) {
            if let Some(arr) = v["findings"].as_array() {
                for f in arr {
                    if let Some(t) = f["type"].as_str() {
                        out.push(t.to_string());
                    }
                }
            }
        }
    }
    out
}

fn diagnostic_codes(stdout: &str) -> Vec<String> {
    let mut out = Vec::new();
    for doc in split_json_docs(stdout) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&doc) {
            if let Some(arr) = v["diagnostics"].as_array() {
                for d in arr {
                    if let Some(c) = d["code"].as_str() {
                        out.push(c.to_string());
                    }
                }
            }
        }
    }
    out
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("allium-xmw-{name}-{}", std::process::id()));
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

const CORE: &str = "-- allium: 3\n\
entity Order {\n\
    id: String\n\
    status: pending | paid | shipped\n\
\n\
    transitions status {\n\
        pending -> paid\n\
        paid -> shipped\n\
        terminal: shipped\n\
    }\n\
}\n\
\n\
rule MarkPaid {\n\
    when: PayOrder(order)\n\
    requires: order.status = pending\n\
    ensures: order.status = paid\n\
    ensures: OrderPaid(order)\n\
}\n";

// Witnesses paid -> shipped in a different module via the imported chained trigger.
const SHIPPING: &str = "-- allium: 3\n\
use \"./core.allium\" as core\n\
\n\
rule ShipOnPayment {\n\
    when: core/OrderPaid(order)\n\
    requires: order.status = paid\n\
    ensures: order.status = shipped\n\
}\n";

// Subscribes to the same trigger but never establishes the exit — no witness.
const LOGGER: &str = "-- allium: 3\n\
use \"./core.allium\" as core\n\
\n\
rule LogPayment {\n\
    when: core/OrderPaid(order)\n\
    ensures: order.logged = true\n\
}\n";

#[test]
fn cross_module_witness_clears_deadlock_and_noexit() {
    let dir = TempDir::new("cleared");
    dir.write("core.allium", CORE);
    dir.write("shipping.allium", SHIPPING);

    let output = allium()
        .args(["analyse", dir.path().to_str().unwrap()])
        .output()
        .expect("spawn allium");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        !finding_types(&stdout).contains(&"deadlock".to_string()),
        "shipping witnesses paid->shipped, so core's Order must not deadlock.\nOutput: {stdout}"
    );
    assert!(
        !diagnostic_codes(&stdout).contains(&"allium.status.noExit".to_string()),
        "the witnessed exit means 'paid' is not a dead end.\nOutput: {stdout}"
    );
}

#[test]
fn no_over_suppression_when_exit_not_witnessed() {
    let dir = TempDir::new("negative");
    dir.write("core.allium", CORE);
    // logger subscribes to the trigger but never ensures shipped.
    dir.write("logger.allium", LOGGER);

    let output = allium()
        .args(["analyse", dir.path().to_str().unwrap()])
        .output()
        .expect("spawn allium");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        finding_types(&stdout).contains(&"deadlock".to_string()),
        "nothing witnesses paid->shipped, so the deadlock must still be reported.\nOutput: {stdout}"
    );
}

#[test]
fn single_file_core_still_deadlocks() {
    let dir = TempDir::new("single");
    dir.write("core.allium", CORE);

    let output = allium()
        .args(["analyse", &dir.path().join("core.allium").to_string_lossy()])
        .output()
        .expect("spawn allium");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        finding_types(&stdout).contains(&"deadlock".to_string()),
        "checked alone, core's paid->shipped has no witness and must deadlock.\nOutput: {stdout}"
    );
}
