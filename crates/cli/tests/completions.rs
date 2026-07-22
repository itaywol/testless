use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn completion_zsh_emits_script() {
    Command::cargo_bin("pick-a-test").unwrap()
        .args(["completion", "zsh"]).assert().success()
        .stdout(predicate::str::contains("#compdef"));
}

#[test]
fn help_has_examples() {
    Command::cargo_bin("pick-a-test").unwrap()
        .arg("--help").assert().success()
        .stdout(predicate::str::contains("Examples:"));
}
