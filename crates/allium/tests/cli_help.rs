use std::process::Command;

fn allium() -> Command {
    Command::new(env!("CARGO_BIN_EXE_allium"))
}

#[test]
fn help_flag_writes_to_stdout_and_exits_zero() {
    let output = allium().arg("--help").output().expect("spawn allium");
    assert!(output.status.success(), "expected exit 0, got {:?}", output.status);
    assert!(output.stderr.is_empty(), "expected empty stderr, got {:?}", output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage: allium"));
    assert!(stdout.contains("check"));
    assert!(stdout.contains("parse"));
    assert!(stdout.contains("plan"));
    assert!(stdout.contains("model"));
}

#[test]
fn short_help_flag_matches_long_form() {
    let long = allium().arg("--help").output().expect("spawn allium");
    let short = allium().arg("-h").output().expect("spawn allium");
    assert_eq!(long.stdout, short.stdout);
    assert!(long.status.success());
    assert!(short.status.success());
}

#[test]
fn help_subcommand_prints_top_level_help() {
    let output = allium().arg("help").output().expect("spawn allium");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage: allium"));
}

#[test]
fn help_subcommand_prints_specific_command_help() {
    for command in ["check", "parse", "plan", "model"] {
        let output = allium().args(["help", command]).output().expect("spawn allium");
        assert!(output.status.success(), "help {command} did not exit 0");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(&format!("allium {command}")),
            "expected help to mention `allium {command}`, got {stdout}"
        );
        assert!(stdout.contains("Usage:"));
    }
}

#[test]
fn subcommand_help_flag_prints_command_help() {
    for command in ["check", "parse", "plan", "model"] {
        let output = allium().args([command, "--help"]).output().expect("spawn allium");
        assert!(output.status.success(), "{command} --help did not exit 0");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains(&format!("allium {command}")));
    }
}

#[test]
fn no_arguments_exits_two_and_writes_hint_to_stderr() {
    let output = allium().output().expect("spawn allium");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("missing command"));
    assert!(stderr.contains("--help"));
}

#[test]
fn unknown_command_exits_two_and_writes_hint_to_stderr() {
    let output = allium().arg("wibble").output().expect("spawn allium");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown command"));
    assert!(stderr.contains("wibble"));
}

#[test]
fn help_for_unknown_command_exits_two() {
    let output = allium().args(["help", "wibble"]).output().expect("spawn allium");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown command"));
}

#[test]
fn version_flag_prints_version_to_stdout() {
    let output = allium().arg("--version").output().expect("spawn allium");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("allium "));
}
