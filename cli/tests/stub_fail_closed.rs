use std::process::{Command, Output};

const UNAVAILABLE_DIAGNOSTIC: &str =
    "taskveil: operational commands are unavailable in this build\n";

fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_taskveil"))
        .args(arguments)
        .output()
        .expect("run taskveil CLI")
}

#[test]
fn operational_commands_fail_without_stdout_or_input_echo() {
    for arguments in [
        &["add", "do-not-echo-title"][..],
        &["list"][..],
        &["done", "do-not-echo-id"][..],
    ] {
        let output = run(arguments);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, UNAVAILABLE_DIAGNOSTIC.as_bytes());
        assert!(!output
            .stderr
            .windows(b"do-not-echo".len())
            .any(|window| window == b"do-not-echo"));
    }
}

#[test]
fn help_and_version_succeed_while_parse_errors_fail() {
    for arguments in [&["--help"][..], &["--version"][..]] {
        let output = run(arguments);
        assert!(output.status.success());
        assert!(!output.stdout.is_empty());
    }

    let output = run(&["unknown-command"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}
