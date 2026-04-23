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

/// Parse the JSON output from `allium check` / `allium analyse` and return
/// all diagnostics with their code and message.
fn parse_diagnostics(stdout: &str) -> Vec<Diag> {
    let mut diags = Vec::new();
    let docs = split_json_docs(stdout);
    for doc in &docs {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(doc) {
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

fn diagnostic_codes(stdout: &str) -> Vec<String> {
    parse_diagnostics(stdout).into_iter().map(|d| d.code).collect()
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

// -----------------------------------------------------------------------
// Core scenario: cross-module reference suppresses unused warning
// -----------------------------------------------------------------------

#[test]
fn cross_module_ref_suppresses_unused_entity() {
    let dir = TempDir::new("suppress-entity");
    dir.write("core.allium", "-- allium: 3\nentity InputEvent {\n  payload: String\n}\n");
    dir.write(
        "consumer.allium",
        "-- allium: 3\nuse \"./core.allium\" as core\n\nrule Handle {\n  when: e: core/InputEvent\n  ensures: e.payload = \"done\"\n}\n",
    );

    let output = allium()
        .args(["check", dir.path().to_str().unwrap()])
        .output()
        .expect("spawn allium");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let codes = diagnostic_codes(&stdout);

    // InputEvent is referenced by consumer.allium — should not be flagged.
    assert!(
        !codes.iter().any(|c| c == "allium.entity.unused"),
        "InputEvent should not be flagged as unused when referenced cross-module.\nDiagnostics: {codes:?}"
    );
}

#[test]
fn cross_module_ref_in_ensures_suppresses_unused() {
    let dir = TempDir::new("suppress-ensures");
    dir.write("statuses.allium", "-- allium: 3\nvalue Active {\n  since: Timestamp\n}\n");
    dir.write(
        "consumer.allium",
        "-- allium: 3\nuse \"./statuses.allium\" as statuses\n\nrule Activate {\n  when: order: Order\n  ensures: order.state = statuses/Active\n}\n",
    );

    let output = allium()
        .args(["check", dir.path().to_str().unwrap()])
        .output()
        .expect("spawn allium");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let diags = parse_diagnostics(&stdout);

    assert!(
        !diags.iter().any(|d| d.code == "allium.definition.unused" && d.message.contains("Active")),
        "Active referenced in ensures should not be flagged.\nDiagnostics: {:?}",
        diags.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
}

#[test]
fn cross_module_ref_suppresses_unused_value() {
    let dir = TempDir::new("suppress-value");
    dir.write("shared.allium", "-- allium: 3\nvalue Snapshot {\n  version: Integer\n}\n");
    dir.write(
        "consumer.allium",
        "-- allium: 3\nuse \"./shared.allium\" as shared\n\nentity Record {\n  snap: shared/Snapshot\n}\n",
    );

    let output = allium()
        .args(["check", dir.path().to_str().unwrap()])
        .output()
        .expect("spawn allium");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let codes = diagnostic_codes(&stdout);

    assert!(
        !codes.iter().any(|c| c == "allium.definition.unused"),
        "Snapshot should not be flagged as unused.\nDiagnostics: {codes:?}"
    );
}

#[test]
fn cross_module_ref_suppresses_unused_enum() {
    let dir = TempDir::new("suppress-enum");
    dir.write("shared.allium", "-- allium: 3\nenum Priority {\n  low\n  medium\n  high\n}\n");
    dir.write(
        "consumer.allium",
        "-- allium: 3\nuse \"./shared.allium\" as shared\n\nentity Task {\n  priority: shared/Priority\n}\n",
    );

    let output = allium()
        .args(["check", dir.path().to_str().unwrap()])
        .output()
        .expect("spawn allium");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let codes = diagnostic_codes(&stdout);

    assert!(
        !codes.iter().any(|c| c == "allium.definition.unused"),
        "Priority enum should not be flagged as unused.\nDiagnostics: {codes:?}"
    );
}

// -----------------------------------------------------------------------
// Truly unused declarations still warn
// -----------------------------------------------------------------------

#[test]
fn unreferenced_entity_still_warns_in_multi_file() {
    let dir = TempDir::new("still-warns");
    dir.write("core.allium", "-- allium: 3\nentity Used {\n  x: String\n}\n\nentity Orphan {\n  y: String\n}\n");
    dir.write(
        "consumer.allium",
        "-- allium: 3\nuse \"./core.allium\" as core\n\nentity Handler {\n  event: core/Used\n}\n",
    );

    let output = allium()
        .args(["check", dir.path().to_str().unwrap()])
        .output()
        .expect("spawn allium");
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Orphan is not referenced by anyone — should still warn.
    assert!(
        stdout.contains("Orphan"),
        "Orphan should still be flagged as unused.\nOutput: {stdout}"
    );
}

// -----------------------------------------------------------------------
// Single-file check is unaffected
// -----------------------------------------------------------------------

#[test]
fn single_file_check_still_warns_for_unused() {
    let dir = TempDir::new("single-file");
    dir.write("core.allium", "-- allium: 3\nentity InputEvent {\n  payload: String\n}\n");

    let output = allium()
        .args(["check", &dir.path().join("core.allium").to_string_lossy()])
        .output()
        .expect("spawn allium");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let codes = diagnostic_codes(&stdout);

    // Without a consumer in the check set, InputEvent is unused.
    assert!(
        codes.iter().any(|c| c == "allium.entity.unused"),
        "InputEvent should be flagged when checked alone.\nDiagnostics: {codes:?}"
    );
}

// -----------------------------------------------------------------------
// Multiple consumers referencing the same target
// -----------------------------------------------------------------------

#[test]
fn multiple_consumers_all_contribute_refs() {
    let dir = TempDir::new("multi-consumer");
    dir.write(
        "core.allium",
        "-- allium: 3\nentity EventA {\n  x: String\n}\n\nentity EventB {\n  y: String\n}\n",
    );
    dir.write(
        "consumer1.allium",
        "-- allium: 3\nuse \"./core.allium\" as core\n\nentity Handler1 {\n  a: core/EventA\n}\n",
    );
    dir.write(
        "consumer2.allium",
        "-- allium: 3\nuse \"./core.allium\" as core\n\nentity Handler2 {\n  b: core/EventB\n}\n",
    );

    let output = allium()
        .args(["check", dir.path().to_str().unwrap()])
        .output()
        .expect("spawn allium");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let diags = parse_diagnostics(&stdout);

    // EventA and EventB are referenced cross-module — should not be flagged.
    // Handler1 and Handler2 may be flagged (they aren't referenced by anything).
    assert!(
        !diags.iter().any(|d| d.code == "allium.entity.unused" && d.message.contains("EventA")),
        "EventA should not be flagged as unused.\nDiagnostics: {:?}",
        diags.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
    assert!(
        !diags.iter().any(|d| d.code == "allium.entity.unused" && d.message.contains("EventB")),
        "EventB should not be flagged as unused.\nDiagnostics: {:?}",
        diags.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
}

// -----------------------------------------------------------------------
// Use without alias — qualified refs are not possible
// -----------------------------------------------------------------------

#[test]
fn use_without_alias_does_not_suppress() {
    let dir = TempDir::new("no-alias");
    dir.write("core.allium", "-- allium: 3\nentity InputEvent {\n  payload: String\n}\n");
    // use without alias — no qualified references possible
    dir.write(
        "consumer.allium",
        "-- allium: 3\nuse \"./core.allium\"\n\nentity Handler {\n  x: String\n}\n",
    );

    let output = allium()
        .args(["check", dir.path().to_str().unwrap()])
        .output()
        .expect("spawn allium");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let codes = diagnostic_codes(&stdout);

    assert!(
        codes.iter().any(|c| c == "allium.entity.unused"),
        "InputEvent should still be unused — consumer has no alias to reference it.\nDiagnostics: {codes:?}"
    );
}

// -----------------------------------------------------------------------
// Alias targets a file not in the check set — no suppression
// -----------------------------------------------------------------------

#[test]
fn alias_to_file_outside_check_set_does_not_suppress() {
    let dir = TempDir::new("outside-set");
    dir.write("core.allium", "-- allium: 3\nentity InputEvent {\n  payload: String\n}\n");
    // consumer references a different file that isn't in the check set
    dir.write(
        "consumer.allium",
        "-- allium: 3\nuse \"./other.allium\" as other\n\nentity Handler {\n  event: other/InputEvent\n}\n",
    );

    let output = allium()
        .args(["check", dir.path().to_str().unwrap()])
        .output()
        .expect("spawn allium");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let codes = diagnostic_codes(&stdout);

    // core.allium's InputEvent is not referenced (consumer points at other.allium)
    assert!(
        codes.iter().any(|c| c == "allium.entity.unused"),
        "InputEvent in core.allium is not referenced by consumer's alias.\nDiagnostics: {codes:?}"
    );
}

// -----------------------------------------------------------------------
// analyse command also respects cross-module refs
// -----------------------------------------------------------------------

#[test]
fn analyse_command_respects_cross_module_refs() {
    let dir = TempDir::new("analyse-xmod");
    dir.write("core.allium", "-- allium: 3\nentity InputEvent {\n  payload: String\n}\n");
    dir.write(
        "consumer.allium",
        "-- allium: 3\nuse \"./core.allium\" as core\n\nrule Handle {\n  when: e: core/InputEvent\n  ensures: e.payload = \"done\"\n}\n",
    );

    let output = allium()
        .args(["analyse", dir.path().to_str().unwrap()])
        .output()
        .expect("spawn allium");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let codes = diagnostic_codes(&stdout);

    assert!(
        !codes.iter().any(|c| c == "allium.entity.unused"),
        "analyse should also suppress cross-module unused warnings.\nDiagnostics: {codes:?}"
    );
}

// -----------------------------------------------------------------------
// Subdirectory use paths resolve correctly
// -----------------------------------------------------------------------

#[test]
fn subdirectory_use_path_resolves() {
    let dir = TempDir::new("subdir-path");
    fs::create_dir_all(dir.path().join("shared")).unwrap();
    dir.write("shared/types.allium", "-- allium: 3\nvalue Money {\n  amount: Decimal\n  currency: String\n}\n");
    dir.write(
        "order.allium",
        "-- allium: 3\nuse \"./shared/types.allium\" as types\n\nentity Order {\n  total: types/Money\n}\n",
    );

    let output = allium()
        .args(["check", dir.path().to_str().unwrap()])
        .output()
        .expect("spawn allium");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let codes = diagnostic_codes(&stdout);

    assert!(
        !codes.iter().any(|c| c == "allium.definition.unused"),
        "Money should not be flagged — referenced via subdirectory path.\nDiagnostics: {codes:?}"
    );
}

// -----------------------------------------------------------------------
// Bare relative use paths (no leading ./)
// -----------------------------------------------------------------------

#[test]
fn bare_relative_use_path_resolves() {
    let dir = TempDir::new("bare-relative");
    dir.write("core.allium", "-- allium: 3\nentity Event {\n  x: String\n}\n");
    dir.write(
        "consumer.allium",
        "-- allium: 3\nuse \"core.allium\" as core\n\nentity Handler {\n  event: core/Event\n}\n",
    );

    let output = allium()
        .args(["check", dir.path().to_str().unwrap()])
        .output()
        .expect("spawn allium");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let diags = parse_diagnostics(&stdout);

    assert!(
        !diags.iter().any(|d| d.code == "allium.entity.unused" && d.message.contains("Event")),
        "Event should not be flagged — referenced via bare relative path.\nDiagnostics: {:?}",
        diags.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
    assert!(
        !diags.iter().any(|d| d.code == "allium.use.unresolvedPath"),
        "Bare relative path should resolve.\nDiagnostics: {:?}",
        diags.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
}

// -----------------------------------------------------------------------
// Unresolved use path diagnostics
// -----------------------------------------------------------------------

#[test]
fn single_file_check_does_not_warn_unresolved_use() {
    let dir = TempDir::new("single-no-unresolved");
    dir.write(
        "consumer.allium",
        "-- allium: 3\nuse \"./missing.allium\" as missing\n\nentity Handler {\n  x: String\n}\n",
    );

    // Check only the single file by name (not the directory).
    // Single-file invocations still go through run_multi_file with one file,
    // which means resolved_use_paths IS computed. The use target doesn't
    // resolve, so the diagnostic should fire even for a single file.
    let output = allium()
        .args(["check", &dir.path().join("consumer.allium").to_string_lossy()])
        .output()
        .expect("spawn allium");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let codes = diagnostic_codes(&stdout);

    assert!(
        codes.iter().any(|c| c == "allium.use.unresolvedPath"),
        "Unresolved use path should warn even with a single file.\nDiagnostics: {codes:?}"
    );
}

#[test]
fn unresolved_use_path_warns_when_target_missing() {
    let dir = TempDir::new("unresolved-missing");
    dir.write(
        "consumer.allium",
        "-- allium: 3\nuse \"./missing.allium\" as missing\n\nentity Handler {\n  x: String\n}\n",
    );

    let output = allium()
        .args(["check", dir.path().to_str().unwrap()])
        .output()
        .expect("spawn allium");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let codes = diagnostic_codes(&stdout);

    assert!(
        codes.iter().any(|c| c == "allium.use.unresolvedPath"),
        "Should warn about unresolved use path.\nDiagnostics: {codes:?}"
    );
}

#[test]
fn resolved_use_path_no_warning() {
    let dir = TempDir::new("resolved-ok");
    dir.write("core.allium", "-- allium: 3\nentity Event {\n  x: String\n}\n");
    dir.write(
        "consumer.allium",
        "-- allium: 3\nuse \"./core.allium\" as core\n\nentity Handler {\n  event: core/Event\n}\n",
    );

    let output = allium()
        .args(["check", dir.path().to_str().unwrap()])
        .output()
        .expect("spawn allium");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let codes = diagnostic_codes(&stdout);

    assert!(
        !codes.iter().any(|c| c == "allium.use.unresolvedPath"),
        "Should not warn — target file is in the check set.\nDiagnostics: {codes:?}"
    );
}

#[test]
fn use_target_exists_but_not_in_check_set_warns() {
    let dir = TempDir::new("not-in-set");
    dir.write("core.allium", "-- allium: 3\nentity Event {\n  x: String\n}\n");
    dir.write(
        "consumer.allium",
        "-- allium: 3\nuse \"./core.allium\" as core\n\nentity Handler {\n  event: core/Event\n}\n",
    );

    // Only check consumer.allium — core.allium exists on disk but is not in the check set.
    let output = allium()
        .args(["check", &dir.path().join("consumer.allium").to_string_lossy()])
        .output()
        .expect("spawn allium");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let codes = diagnostic_codes(&stdout);

    // Single-file mode: resolved_use_paths is empty for the file, so the check
    // is skipped. This is by design — single-file analysis can't know the full
    // check set.
    // When only one file is passed we still get a resolved_use_paths set
    // (containing nothing, since core.allium isn't in the parsed set).
    // The check fires because the resolved set is non-empty (it's computed for
    // every multi-file invocation, even with one file).
    assert!(
        codes.iter().any(|c| c == "allium.use.unresolvedPath"),
        "Should warn — core.allium exists but is not in the check set.\nDiagnostics: {codes:?}"
    );
}

#[test]
fn unresolved_use_path_message_names_file() {
    let dir = TempDir::new("unresolved-msg");
    dir.write(
        "consumer.allium",
        "-- allium: 3\nuse \"./phantom.allium\" as phantom\n\nentity Handler {\n  x: String\n}\n",
    );

    let output = allium()
        .args(["check", dir.path().to_str().unwrap()])
        .output()
        .expect("spawn allium");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let diags = parse_diagnostics(&stdout);

    let d = diags.iter().find(|d| d.code == "allium.use.unresolvedPath")
        .expect("expected allium.use.unresolvedPath diagnostic");
    assert!(
        d.message.contains("phantom.allium"),
        "message should name the path: {}", d.message
    );
}

#[test]
fn mixed_resolved_and_unresolved_use_paths() {
    let dir = TempDir::new("mixed-use");
    dir.write("found.allium", "-- allium: 3\nentity Found {\n  x: String\n}\n");
    dir.write(
        "consumer.allium",
        "-- allium: 3\nuse \"./found.allium\" as found\nuse \"./lost.allium\" as lost\n\nentity Handler {\n  f: found/Found\n}\n",
    );

    let output = allium()
        .args(["check", dir.path().to_str().unwrap()])
        .output()
        .expect("spawn allium");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let diags = parse_diagnostics(&stdout);

    let unresolved: Vec<_> = diags.iter()
        .filter(|d| d.code == "allium.use.unresolvedPath")
        .collect();
    assert_eq!(unresolved.len(), 1, "only lost.allium should be unresolved: {:?}",
        unresolved.iter().map(|d| &d.message).collect::<Vec<_>>());
    assert!(unresolved[0].message.contains("lost.allium"));
}

#[test]
fn analyse_also_reports_unresolved_use_paths() {
    let dir = TempDir::new("analyse-unresolved");
    dir.write(
        "consumer.allium",
        "-- allium: 3\nuse \"./ghost.allium\" as ghost\n\nentity Handler {\n  x: String\n}\n",
    );

    let output = allium()
        .args(["analyse", dir.path().to_str().unwrap()])
        .output()
        .expect("spawn allium");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let codes = diagnostic_codes(&stdout);

    assert!(
        codes.iter().any(|c| c == "allium.use.unresolvedPath"),
        "analyse should also report unresolved use paths.\nDiagnostics: {codes:?}"
    );
}
