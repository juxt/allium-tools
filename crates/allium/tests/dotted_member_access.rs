//! The legacy dotted reference form `alias.TypeName` is a heuristic reread of
//! `ident.UppercaseField` member access — the parser emits plain member
//! access for that shape, never a qualified name. Member access through a
//! locally-bound value name (an entity field holding a `Map<String, Any>`, a
//! trigger parameter, a `let`) must therefore not draw
//! `allium.reference.undefinedImportedAlias`, which fires at error severity
//! and can redden a gate on a module with zero `use` declarations. The
//! exemption is only for the dotted reread: a dotted qualifier bound nowhere
//! still errors, a dotted qualifier that IS a declared alias still flows to
//! the name-membership check, a `deferred` declaration's dotted path is
//! never exempt (it names a code location, not a value), and the slash form
//! `alias/Name` is never exempt (#87's case C).

use std::fs;
use std::path::Path;
use std::process::Command;

fn allium() -> Command {
    Command::new(env!("CARGO_BIN_EXE_allium"))
}

struct Diag {
    code: String,
    message: String,
    severity: String,
}

fn parse_diagnostics(stdout: &str) -> Vec<Diag> {
    let mut diags = Vec::new();
    for doc in split_json_docs(stdout) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&doc) {
            if let Some(arr) = v["diagnostics"].as_array() {
                for d in arr {
                    if let (Some(c), Some(m)) = (d["code"].as_str(), d["message"].as_str()) {
                        let severity = d["severity"].as_str().unwrap_or_default().to_string();
                        diags.push(Diag { code: c.to_string(), message: m.to_string(), severity });
                    }
                }
            }
        }
    }
    diags
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

fn run(cmd: &str, args: &[&str]) -> (bool, String) {
    let output = allium().arg(cmd).args(args).output().expect("spawn allium");
    (output.status.success(), String::from_utf8_lossy(&output.stdout).into_owned())
}

fn reference_diags(diags: &[Diag]) -> Vec<(&String, &String)> {
    diags
        .iter()
        .filter(|d| d.code.starts_with("allium.reference."))
        .map(|d| (&d.code, &d.message))
        .collect()
}

// ===========================================================================
// Headline repro: a Map field's capitalised keys, module with zero imports.
// ===========================================================================

const MAP_PAGE: &str = r#"-- allium: 3

external entity Page {
    properties: Map<String, Any>
    current_status: String? = first_non_null(
        properties.Status.status.name,
        properties.Review.status.name
    )
    lower_status: String? = properties.status.name
}
"#;

#[test]
fn map_field_member_access_draws_no_reference_diagnostic() {
    let dir = TempDir::new("map-member-access");
    dir.write("map.allium", MAP_PAGE);

    let (_ok, out) = run("check", &[dir.path().to_str().unwrap()]);
    let diags = parse_diagnostics(&out);
    assert!(
        reference_diags(&diags).is_empty(),
        "member access on a declared Map field must not be read as an import reference.\nDiags: {:?}",
        reference_diags(&diags)
    );
    // The false positive was error severity. The minimal fixture keeps its
    // incidental unused/source-hint warnings; what must be gone is any error.
    assert!(
        !diags.iter().any(|d| d.severity == "error"),
        "a module with zero imports must carry no error-severity diagnostic.\nDiags: {:?}",
        diags.iter().map(|d| (&d.code, &d.severity)).collect::<Vec<_>>()
    );
}

#[test]
fn lowercase_member_access_stays_clean() {
    let dir = TempDir::new("lowercase-member");
    dir.write(
        "map.allium",
        "-- allium: 3\n\nexternal entity Page {\n    properties: Map<String, Any>\n    lower_status: String? = properties.status.name\n}\n",
    );

    let (_ok, out) = run("check", &[dir.path().to_str().unwrap()]);
    let diags = parse_diagnostics(&out);
    assert!(
        reference_diags(&diags).is_empty(),
        "lowercase member access never matched the dotted heuristic and must stay clean.\nDiags: {:?}",
        reference_diags(&diags)
    );
}

#[test]
fn optional_access_through_bound_field_exempt() {
    let dir = TempDir::new("optional-member-access");
    dir.write(
        "map.allium",
        "-- allium: 3\n\nexternal entity Page {\n    properties: Map<String, Any>\n    s: String? = properties?.Status.name\n}\n",
    );

    let (_ok, out) = run("check", &[dir.path().to_str().unwrap()]);
    let diags = parse_diagnostics(&out);
    assert!(
        reference_diags(&diags).is_empty(),
        "optional access takes the same heuristic arm and gets the same exemption.\nDiags: {:?}",
        reference_diags(&diags)
    );
}

#[test]
fn trigger_param_and_let_binding_qualifiers_exempt() {
    let dir = TempDir::new("bound-qualifiers");
    dir.write(
        "spec.allium",
        "-- allium: 3\n\nentity Order {\n    items: Map<String, Any>\n}\n\nrule Handle {\n    when: Go(payload)\n    ensures:\n        let extra = payload.Items\n        Order.created(items: extra.Raw)\n}\n",
    );

    let (_ok, out) = run("check", &[dir.path().to_str().unwrap()]);
    let diags = parse_diagnostics(&out);
    assert!(
        reference_diags(&diags).is_empty(),
        "a trigger parameter and an expression let are bound value names; member access through them is not an import reference.\nDiags: {:?}",
        reference_diags(&diags)
    );
}

#[test]
fn invariant_body_bindings_exempt() {
    // A top-level `invariant Name { ... }` body binds names like any other
    // expression: a `for` quantifier binding, a lambda parameter, a `let`.
    let dir = TempDir::new("invariant-bindings");
    dir.write(
        "spec.allium",
        "-- allium: 3\n\nexternal entity Page {\n    properties: Map<String, Any>\n}\n\ninvariant EveryPageHasStatus {\n    for page in Pages: page.Status != null\n}\n\ninvariant AllPagesTagged {\n    Pages.all(p => p.Status != null)\n}\n\ninvariant WithLet {\n    let snap = first_non_null(Pages)\n    snap.Status != null\n}\n",
    );

    let (_ok, out) = run("check", &[dir.path().to_str().unwrap()]);
    let diags = parse_diagnostics(&out);
    assert!(
        reference_diags(&diags).is_empty(),
        "for/lambda/let bindings inside a top-level invariant body are bound value names; member access through them is not an import reference.\nDiags: {:?}",
        reference_diags(&diags)
    );
}

#[test]
fn variant_field_member_access_exempt() {
    // `variant Name : Base { field: Type ... }` parses its brace body into
    // the base expression; the field declarations must still count as bound
    // value names.
    let dir = TempDir::new("variant-fields");
    dir.write(
        "spec.allium",
        "-- allium: 3\n\nexternal entity Page {\n    properties: Map<String, Any>\n}\n\nvariant ArchivedPage : Page {\n    archive_meta: Map<String, Any>\n    reason: String? = archive_meta.Reason.name\n}\n",
    );

    let (_ok, out) = run("check", &[dir.path().to_str().unwrap()]);
    let diags = parse_diagnostics(&out);
    assert!(
        reference_diags(&diags).is_empty(),
        "a variant-declared field is a bound value name; member access through it is not an import reference.\nDiags: {:?}",
        reference_diags(&diags)
    );
}

#[test]
fn default_declaration_name_exempt() {
    // `default Page home = ...` binds `home` at module level.
    let dir = TempDir::new("default-name");
    dir.write(
        "spec.allium",
        "-- allium: 3\n\nexternal entity Page {\n    properties: Map<String, Any>\n}\n\ndefault Page home = { properties: {} }\n\nentity Wrapper {\n    status: String? = home.Status.name\n}\n",
    );

    let (_ok, out) = run("check", &[dir.path().to_str().unwrap()]);
    let diags = parse_diagnostics(&out);
    assert!(
        reference_diags(&diags).is_empty(),
        "a default declaration's name is a bound value name; member access through it is not an import reference.\nDiags: {:?}",
        reference_diags(&diags)
    );
}

#[test]
fn surface_assignment_binding_exempt() {
    // A `name: expr` item in a surface block binds a projection the
    // surface's other clauses can navigate.
    let dir = TempDir::new("surface-assignment");
    dir.write(
        "spec.allium",
        "-- allium: 3\n\nexternal entity Page {\n    properties: Map<String, Any>\n}\n\nsurface Dashboard {\n    facing viewer: Admin\n    recent: Pages\n    shows: recent.Status\n}\n",
    );

    let (_ok, out) = run("check", &[dir.path().to_str().unwrap()]);
    let diags = parse_diagnostics(&out);
    assert!(
        reference_diags(&diags).is_empty(),
        "a surface-level assignment binds a value name; member access through it is not an import reference.\nDiags: {:?}",
        reference_diags(&diags)
    );
}

// ===========================================================================
// Guards: what must keep firing.
// ===========================================================================

#[test]
fn deferred_path_qualifier_never_exempt() {
    // A deferred path names a code location, never member access on a value:
    // a dotted deferred path through an undeclared alias must keep erroring
    // even when the module binds a value name spelled the same.
    let dir = TempDir::new("deferred-path");
    dir.write(
        "spec.allium",
        "-- allium: 3\n\nexternal entity Page {\n    quiz_creation: Map<String, Any>\n}\n\ndeferred quiz_creation.QuizCreationWizard\n",
    );

    let (_ok, out) = run("check", &[dir.path().to_str().unwrap()]);
    let diags = parse_diagnostics(&out);
    assert!(
        diags.iter().any(|d| d.code == "allium.reference.undefinedImportedAlias"
            && d.message.contains("quiz_creation")),
        "a dotted deferred path is not member access; a same-named field must not silence its unknown-alias error.\nDiags: {:?}",
        diags.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
}

#[test]
fn dotted_qualifier_bound_nowhere_still_errors() {
    // `quiz_creation` is neither a use alias nor any locally-bound value
    // name: this is the real legacy dotted form with a missing use
    // declaration and must keep erroring.
    let dir = TempDir::new("dotted-unbound");
    dir.write(
        "quiz.allium",
        "-- allium: 3\n\nentity Quiz {\n    name: String\n    wizard: String? = quiz_creation.QuizCreationWizard.name\n}\n",
    );

    let (_ok, out) = run("check", &[dir.path().to_str().unwrap()]);
    let diags = parse_diagnostics(&out);
    assert!(
        diags.iter().any(|d| d.code == "allium.reference.undefinedImportedAlias"
            && d.message.contains("quiz_creation")),
        "a dotted qualifier bound nowhere must keep erroring undefinedImportedAlias.\nDiags: {:?}",
        diags.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
}

#[test]
fn slash_form_unknown_alias_still_errors() {
    // The slash form parses as a qualified name; the exemption never applies.
    let dir = TempDir::new("slash-unknown-alias");
    dir.write(
        "spec.allium",
        "-- allium: 3\n\nrule R {\n    when: p/Go(x)\n    ensures: x.done = true\n}\n",
    );

    let (_ok, out) = run("check", &[dir.path().to_str().unwrap()]);
    let diags = parse_diagnostics(&out);
    assert!(
        diags.iter().any(|d| d.code == "allium.reference.undefinedImportedAlias"
            && d.message.contains("'p'")),
        "a slash-form reference through an undeclared alias must keep erroring.\nDiags: {:?}",
        diags.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
}

#[test]
fn dotted_alias_qualifier_still_checked_for_offered_names() {
    // A dotted qualifier that IS a declared alias keeps flowing to the
    // name-membership check: `core.Missing` warns, `core.Widget` is clean.
    let dir = TempDir::new("dotted-alias-membership");
    dir.write("core.allium", "-- allium: 3\n\nentity Widget {\n    name: String\n}\n");
    dir.write(
        "consumer.allium",
        "-- allium: 3\n\nuse \"./core.allium\" as core\n\nentity Holder {\n    w: String? = core.Missing.name\n    v: String? = core.Widget.name\n}\n",
    );

    let (_ok, out) = run("check", &[dir.path().to_str().unwrap()]);
    let diags = parse_diagnostics(&out);
    assert!(
        diags.iter().any(|d| d.code == "allium.reference.unknownName"
            && d.message.contains("Missing")),
        "a dotted reference through a declared alias must still have its name checked.\nDiags: {:?}",
        diags.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
    assert!(
        !diags.iter().any(|d| d.code == "allium.reference.undefinedImportedAlias"),
        "a declared alias must not draw the unknown-alias error.\nDiags: {:?}",
        diags.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
    assert!(
        !diags.iter().any(|d| d.code == "allium.reference.unknownName"
            && d.message.contains("'Widget'")),
        "an offered name through the dotted form stays clean.\nDiags: {:?}",
        diags.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
}
